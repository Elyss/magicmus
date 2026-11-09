//! Lecteur intégré : son (cpal + changement de tonalité / vitesse en direct),
//! image (frames décodées par ffmpeg) et paroles karaoké synchronisées.
//!
//! L'horloge de référence est la position dans le son : l'image et les paroles
//! la suivent, quelle que soit la vitesse choisie.

use crate::lyrics::Lyrics;
use crate::stretch::Stretch;
use crate::{media, tools};
use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, TextureHandle, TextureOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub const TEMPO_RANGE: std::ops::RangeInclusive<f32> = 0.5..=1.5;
pub const SEMITONE_RANGE: std::ops::RangeInclusive<f32> = -12.0..=12.0;
const VIDEO_FPS: f64 = 30.0;
/// Latence approximative de la carte son (s).
const DEVICE_LATENCY: f64 = 0.03;

/// État partagé avec le thread audio.
struct Audio {
    pcm: Arc<Vec<f32>>, // karaoké : stéréo entrelacé à la fréquence de la carte son
    /// Audio d'origine (avec la voix), chargé en arrière-plan pour la comparaison.
    orig: Option<Arc<Vec<f32>>>,
    /// Part de l'original entendue (0 = karaoké, 1 = original) et cible du fondu.
    voice: f32,
    voice_target: f32,
    /// Pas du fondu par trame (≈ 40 ms pour passer d'une version à l'autre).
    fade_step: f32,
    pos: usize, // trame source courante
    frac: f64,          // reste fractionnaire (vitesse non entière)
    playing: bool,
    tempo: f32,
    semitones: f32,
    applied_semitones: f32,
    volume: f32,
    stretch: Stretch,
    inbuf: Vec<f32>,
    outbuf: Vec<f32>,
}

impl Audio {
    fn frames(&self) -> usize {
        self.pcm.len() / 2
    }

    /// Remplit le tampon de la carte son (`channels` canaux entrelacés).
    fn render(&mut self, data: &mut [f32], channels: usize) {
        data.fill(0.0);
        let frames = data.len() / channels.max(1);
        if !self.playing || frames == 0 {
            return;
        }
        let total = self.frames();
        if self.pos >= total {
            self.playing = false;
            return;
        }
        if self.applied_semitones != self.semitones {
            self.stretch.set_semitones(self.semitones);
            self.applied_semitones = self.semitones;
        }
        let want = frames as f64 * self.tempo as f64 + self.frac;
        let n_in = want.floor() as usize;
        self.frac = want - n_in as f64;

        let end = (self.pos + n_in).min(total);
        self.inbuf.clear();
        match &self.orig {
            // Mélange karaoké / original avec fondu (pas de clic au basculement).
            Some(orig) if self.voice > 0.0 || self.voice_target > 0.0 => {
                for k in self.pos..end {
                    if self.voice < self.voice_target {
                        self.voice = (self.voice + self.fade_step).min(self.voice_target);
                    } else if self.voice > self.voice_target {
                        self.voice = (self.voice - self.fade_step).max(self.voice_target);
                    }
                    let (il, ir) = (self.pcm[2 * k], self.pcm[2 * k + 1]);
                    let (ol, or) = match orig.get(2 * k..2 * k + 2) {
                        Some(o) => (o[0], o[1]),
                        None => (il, ir),
                    };
                    self.inbuf.push(il + self.voice * (ol - il));
                    self.inbuf.push(ir + self.voice * (or - ir));
                }
            }
            _ => self.inbuf.extend_from_slice(&self.pcm[self.pos * 2..end * 2]),
        }
        self.inbuf.resize(n_in * 2, 0.0);
        self.outbuf.resize(frames * 2, 0.0);
        self.stretch.process(&self.inbuf, &mut self.outbuf);
        self.pos += n_in;

        let v = self.volume;
        for (i, f) in data.chunks_exact_mut(channels).enumerate() {
            let (l, r) = (self.outbuf[2 * i] * v, self.outbuf[2 * i + 1] * v);
            if channels == 1 {
                f[0] = 0.5 * (l + r);
            } else {
                f[0] = l;
                f[1] = r;
            }
        }
    }
}

struct Frame {
    t: f64,
    rgba: Vec<u8>,
}

/// Décodage de la vidéo par ffmpeg à partir d'un instant donné.
struct VideoStream {
    child: Child,
    rx: Receiver<Frame>,
    started: Instant,
}

impl VideoStream {
    fn spawn(path: &Path, start: f64, w: u32, h: u32) -> Result<Self> {
        let mut child = tools::command("ffmpeg")
            .args(["-v", "error", "-nostdin", "-ss", &format!("{start:.3}"), "-i"])
            .arg(path)
            .args(["-an", "-sn", "-vf", &format!("fps={VIDEO_FPS},scale={w}:{h}:flags=bilinear")])
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("impossible de lancer ffmpeg pour la vidéo")?;
        let mut out = child.stdout.take().context("sortie vidéo indisponible")?;
        let (tx, rx) = mpsc::sync_channel::<Frame>(6);
        let size = (w * h * 4) as usize;
        std::thread::spawn(move || {
            for i in 0.. {
                let mut rgba = vec![0u8; size];
                if out.read_exact(&mut rgba).is_err() {
                    break;
                }
                if tx.send(Frame { t: start + i as f64 / VIDEO_FPS, rgba }).is_err() {
                    break;
                }
            }
        });
        Ok(Self { child, rx, started: Instant::now() })
    }
}

impl Drop for VideoStream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Video {
    size: (u32, u32),
    stream: Option<VideoStream>,
    pending: Option<Frame>,
    texture: Option<TextureHandle>,
    shown: f64,
}

pub struct Player {
    audio: Arc<Mutex<Audio>>,
    _stream: cpal::Stream,
    rate: u32,
    latency: f64,
    pub path: PathBuf,
    pub title: String,
    pub lyrics: Option<Lyrics>,
    video: Option<Video>,
}

impl Player {
    /// `source` : fichier d'origine (avec la voix), chargé en arrière-plan pour pouvoir
    /// basculer à tout moment entre la version karaoké et l'original.
    pub fn open(
        path: &Path,
        title: &str,
        has_video: bool,
        lyrics: Option<Lyrics>,
        source: Option<&Path>,
    ) -> Result<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| anyhow!("aucune sortie audio disponible"))?;
        let supported = device.default_output_config().map_err(|e| anyhow!("{e}"))?;
        let rate = supported.sample_rate();
        let channels = supported.channels() as usize;
        let format = supported.sample_format();
        let config = supported.config();

        let pcm = Arc::new(media::decode_pcm_at(path, rate)?);
        let stretch = Stretch::new(2, rate);
        let (lat_in, lat_out) = stretch.latency();
        let audio = Arc::new(Mutex::new(Audio {
            pcm,
            orig: None,
            voice: 0.0,
            voice_target: 0.0,
            fade_step: 1.0 / (0.04 * rate as f32),
            pos: 0,
            frac: 0.0,
            playing: false,
            tempo: 1.0,
            semitones: 0.0,
            applied_semitones: 0.0,
            volume: 1.0,
            stretch,
            inbuf: Vec::with_capacity(1 << 16),
            outbuf: Vec::with_capacity(1 << 16),
        }));

        let stream = match format {
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, config, channels, audio.clone()),
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, config, channels, audio.clone()),
            cpal::SampleFormat::I32 => build_stream::<i32>(&device, config, channels, audio.clone()),
            cpal::SampleFormat::U16 => build_stream::<u16>(&device, config, channels, audio.clone()),
            other => bail!("format audio de la carte son non pris en charge : {other:?}"),
        }?;
        stream.play().map_err(|e| anyhow!("{e}"))?;

        if let Some(src) = source.map(Path::to_path_buf) {
            let audio = audio.clone();
            std::thread::spawn(move || {
                if let Ok(orig) = media::decode_pcm_at(&src, rate) {
                    if let Ok(mut a) = audio.lock() {
                        a.orig = Some(Arc::new(orig));
                    }
                }
            });
        }

        let video = if has_video {
            media::video_size(path).map(|(w, h)| Video {
                size: display_size(w, h),
                stream: None,
                pending: None,
                texture: None,
                shown: -1.0,
            })
        } else {
            None
        };

        Ok(Self {
            audio,
            _stream: stream,
            rate,
            latency: (lat_in + lat_out) as f64 / rate as f64,
            path: path.to_path_buf(),
            title: title.to_string(),
            lyrics,
            video,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Audio> {
        self.audio.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn duration(&self) -> f64 {
        self.lock().frames() as f64 / self.rate as f64
    }

    /// Instant (dans le morceau d'origine) actuellement entendu.
    pub fn position(&self) -> f64 {
        let a = self.lock();
        let t = a.pos as f64 / self.rate as f64;
        if a.playing {
            (t - (self.latency + DEVICE_LATENCY) * a.tempo as f64).max(0.0)
        } else {
            t
        }
    }

    pub fn is_playing(&self) -> bool {
        self.lock().playing
    }

    pub fn play(&self) {
        let mut a = self.lock();
        if a.pos >= a.frames() {
            a.pos = 0;
            a.stretch.reset();
        }
        a.playing = true;
    }

    pub fn pause(&self) {
        self.lock().playing = false;
    }

    pub fn toggle(&self) {
        if self.is_playing() { self.pause() } else { self.play() }
    }

    pub fn seek(&mut self, t: f64) {
        {
            let mut a = self.lock();
            let frames = a.frames();
            a.pos = ((t.max(0.0) * self.rate as f64) as usize).min(frames);
            a.frac = 0.0;
            a.stretch.reset();
        }
        if let Some(v) = &mut self.video {
            v.stream = None;
            v.pending = None;
            v.shown = -1.0;
        }
    }

    pub fn tempo(&self) -> f32 {
        self.lock().tempo
    }
    pub fn set_tempo(&self, tempo: f32) {
        self.lock().tempo = tempo.clamp(*TEMPO_RANGE.start(), *TEMPO_RANGE.end());
    }
    pub fn semitones(&self) -> f32 {
        self.lock().semitones
    }
    pub fn set_semitones(&self, s: f32) {
        self.lock().semitones = s.clamp(*SEMITONE_RANGE.start(), *SEMITONE_RANGE.end());
    }
    pub fn volume(&self) -> f32 {
        self.lock().volume
    }
    pub fn set_volume(&self, v: f32) {
        self.lock().volume = v.clamp(0.0, 1.5);
    }

    /// L'audio d'origine est-il chargé (comparaison possible) ?
    pub fn original_available(&self) -> bool {
        self.lock().orig.is_some()
    }

    /// Écoute-t-on l'original (avec la voix) plutôt que le karaoké ?
    pub fn original(&self) -> bool {
        self.lock().voice_target > 0.5
    }

    /// Bascule entre le karaoké et l'original (fondu de 40 ms, sans interruption).
    pub fn set_original(&self, on: bool) {
        let mut a = self.lock();
        if a.orig.is_some() {
            a.voice_target = if on { 1.0 } else { 0.0 };
            if !a.playing {
                a.voice = a.voice_target;
            }
        }
    }

    /// Rapport largeur / hauteur de l'image.
    pub fn aspect(&self) -> f32 {
        self.video.as_ref().map_or(16.0 / 9.0, |v| v.size.0 as f32 / v.size.1 as f32)
    }

    pub fn has_video(&self) -> bool {
        self.video.is_some()
    }

    /// Met à jour l'image affichée pour l'instant `t`.
    pub fn update_video(&mut self, ctx: &egui::Context, t: f64) {
        let path = self.path.clone();
        let Some(v) = &mut self.video else { return };

        // (Re)lancement du décodage si nécessaire : premier affichage, retour en
        // arrière, ou décodage trop en retard.
        let behind = v.stream.as_ref().is_some_and(|s| s.started.elapsed().as_secs_f64() > 1.5)
            && t - v.shown > 1.5;
        let ahead = v.pending.as_ref().is_some_and(|f| f.t - t > 2.0);
        if v.stream.is_none() || behind || ahead {
            v.stream = None;
            v.pending = None;
            match VideoStream::spawn(&path, t, v.size.0, v.size.1) {
                Ok(s) => v.stream = Some(s),
                Err(_) => return,
            }
            v.shown = t;
        }
        let Some(stream) = &v.stream else { return };

        let mut latest = None;
        loop {
            if v.pending.is_none() {
                match stream.rx.try_recv() {
                    Ok(f) => v.pending = Some(f),
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
            match &v.pending {
                Some(f) if f.t <= t + 0.5 / VIDEO_FPS => latest = v.pending.take(),
                _ => break,
            }
        }
        if let Some(f) = latest {
            let image = egui::ColorImage::from_rgba_unmultiplied([v.size.0 as usize, v.size.1 as usize], &f.rgba);
            match &mut v.texture {
                Some(tex) => tex.set(image, TextureOptions::LINEAR),
                None => v.texture = Some(ctx.load_texture("video", image, TextureOptions::LINEAR)),
            }
            v.shown = f.t;
        }
    }

    pub fn texture(&self) -> Option<&TextureHandle> {
        self.video.as_ref().and_then(|v| v.texture.as_ref())
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    audio: Arc<Mutex<Audio>>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let mut tmp: Vec<f32> = Vec::new();
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _| {
                tmp.resize(data.len(), 0.0);
                match audio.try_lock() {
                    Ok(mut a) => a.render(&mut tmp, channels),
                    Err(_) => tmp.fill(0.0),
                }
                for (d, s) in data.iter_mut().zip(&tmp) {
                    *d = <T as cpal::Sample>::from_sample(*s);
                }
            },
            |_err| {},
            None,
        )
        .map_err(|e| anyhow!("ouverture de la sortie audio : {e}"))
}

/// Taille d'affichage (max 960×540, dimensions paires, proportions conservées).
fn display_size(w: u32, h: u32) -> (u32, u32) {
    let scale = (960.0 / w as f64).min(540.0 / h as f64).min(1.0);
    let even = |x: f64| ((x / 2.0).round() as u32 * 2).max(2);
    (even(w as f64 * scale), even(h as f64 * scale))
}

/// Apparence des paroles à l'écran.
pub struct LyricsStyle {
    /// Taille de la ligne en cours (px) ; la suivante fait 68 % de cette taille.
    pub size: f32,
    /// Couleur des mots chantés.
    pub highlight: Color32,
    /// Police de la ligne en cours et de la ligne suivante.
    pub current: egui::FontFamily,
    pub next: egui::FontFamily,
}

/// Dessine les paroles karaoké en bas de `rect` : ligne en cours (mots chantés
/// colorés progressivement) et ligne suivante, atténuée.
pub fn draw_lyrics(painter: &egui::Painter, rect: Rect, lyrics: &Lyrics, t: f64, style: &LyricsStyle) {
    let t = t + lyrics.offset;
    let Some(idx) = lyrics.line_at(t) else { return };
    let next_size = style.size * 0.68;
    let y_next = rect.bottom() - 34.0 - next_size * 0.6;
    let y_cur = y_next - next_size * 0.6 - 10.0 - style.size * 0.6;
    let line = &lyrics.lines[idx];
    let started = line.start() <= t + 0.05;
    draw_line(painter, rect, line, t, style.size, y_cur, started, style, &style.current);
    if let Some(next) = lyrics.lines.get(idx + 1) {
        if next.start() - t < 8.0 {
            draw_line(painter, rect, next, f64::NEG_INFINITY, next_size, y_next, false, style, &style.next);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_line(
    painter: &egui::Painter,
    rect: Rect,
    line: &crate::lyrics::Line,
    t: f64,
    mut size: f32,
    y: f32,
    current: bool,
    style: &LyricsStyle,
    family: &egui::FontFamily,
) {
    // Ligne en cours en blanc, ligne suivante (t = −∞) en blanc à 60 %.
    let base = if t.is_finite() { Color32::WHITE } else { Color32::from_white_alpha(153) };
    // Mise en page mot par mot, réduite si la ligne est trop large.
    let layout = |size: f32| {
        let font = FontId::new(size, family.clone());
        let space = painter.layout_no_wrap(" ".into(), font.clone(), base).size().x;
        let words: Vec<_> = line
            .words
            .iter()
            .map(|w| painter.layout_no_wrap(w.text.clone(), font.clone(), base))
            .collect();
        let width = words.iter().map(|g| g.size().x).sum::<f32>() + space * (words.len().saturating_sub(1)) as f32;
        (words, space, width)
    };
    let (mut words, mut space, mut width) = layout(size);
    let max_w = rect.width() * 0.94;
    if width > max_w {
        size *= max_w / width;
        (words, space, width) = layout(size);
    }

    let mut x = rect.center().x - width / 2.0;
    let edge = (size / 30.0).max(1.0);
    for (w, galley) in line.words.iter().zip(&words) {
        let pos = Pos2::new(x, y - galley.size().y / 2.0);
        let wr = Rect::from_min_size(pos, galley.size());
        // ombre douce portée vers le bas + léger contour, pour la lisibilité sur l'image
        for (dx, dy, a) in [(0.0, 3.0, 70u8), (0.0, 2.0, 90), (-1.0, 0.0, 110), (1.0, 0.0, 110), (0.0, -1.0, 90), (0.0, 1.0, 110)] {
            painter.galley_with_override_text_color(
                pos + egui::vec2(dx * edge, dy * edge),
                galley.clone(),
                Color32::from_black_alpha(a),
            );
        }
        painter.galley(pos, galley.clone(), base);
        if current {
            let frac = if t >= w.end { 1.0 } else if t <= w.start { 0.0 } else { ((t - w.start) / (w.end - w.start)) as f32 };
            if frac > 0.0 {
                let clip = Rect::from_min_max(wr.min, Pos2::new(wr.min.x + wr.width() * frac, wr.max.y));
                painter.with_clip_rect(clip.intersect(painter.clip_rect())).galley_with_override_text_color(
                    pos,
                    galley.clone(),
                    style.highlight,
                );
            }
        }
        x += galley.size().x + space;
    }
}
