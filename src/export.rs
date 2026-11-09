//! Enregistrement du résultat avec la tonalité et la vitesse choisies, et les
//! paroles karaoké incrustées dans la vidéo.

use crate::lyrics::{self, Lyrics};
use crate::{media, stretch, tools};
use anyhow::{Context, Result};
use std::path::Path;

pub struct ExportSettings {
    pub tempo: f32,
    pub semitones: f32,
    /// Incruster les paroles dans l'image (vidéo seulement).
    pub burn_lyrics: bool,
}

/// `source` : fichier karaoké (sans voix) ; `dest` : fichier à créer.
pub fn export(
    source: &Path,
    has_video: bool,
    lyrics: Option<&Lyrics>,
    s: &ExportSettings,
    dest: &Path,
    progress: &mut dyn FnMut(&str, Option<f32>),
) -> Result<()> {
    let source = std::path::absolute(source)?;
    let dest = std::path::absolute(dest)?;

    progress("Préparation de l'audio", None);
    let pcm = media::decode_pcm(&source)?;
    let audio = stretch::render(&pcm, 2, media::SAMPLE_RATE, s.tempo, s.semitones);
    let duration = audio.len() as f64 / 2.0 / media::SAMPLE_RATE as f64;
    drop(pcm);

    // Dossier de travail : sous-titres et police, référencés par des chemins
    // relatifs (évite les problèmes d'échappement des chemins Windows dans les filtres).
    let tmp = std::env::temp_dir().join(format!("karaoke-export-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;

    let mut cmd = tools::command("ffmpeg");
    cmd.current_dir(&tmp)
        .args(["-v", "error", "-y", "-nostats", "-progress", "pipe:2"]);
    if has_video {
        cmd.arg("-i").arg(&source);
    }
    cmd.args(["-f", "f32le", "-ar", &media::SAMPLE_RATE.to_string(), "-ac", "2", "-i", "pipe:0"]);

    let ext = dest.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if has_video && ext != "m4a" && ext != "mp3" && ext != "wav" {
        let mut filters = Vec::new();
        if (s.tempo - 1.0).abs() > 1e-4 {
            filters.push(format!("setpts=PTS/{}", s.tempo));
        }
        if let (true, Some(ly)) = (s.burn_lyrics, lyrics) {
            let (w, h) = media::video_size(&source).unwrap_or((1920, 1080));
            std::fs::write(tmp.join("paroles.ass"), lyrics::to_ass(ly, s.tempo as f64, w, h))?;
            let fonts = Path::new(r"C:\Windows\Fonts");
            let mut filter = "ass=paroles.ass".to_string();
            if fonts.join("arial.ttf").is_file() {
                std::fs::create_dir_all(tmp.join("fonts"))?;
                for f in ["arial.ttf", "arialbd.ttf"] {
                    let _ = std::fs::copy(fonts.join(f), tmp.join("fonts").join(f));
                }
                filter.push_str(":fontsdir=fonts");
            }
            filters.push(filter);
        }
        cmd.args(["-map", "0:v:0", "-map", "1:a:0"]);
        if filters.is_empty() {
            cmd.args(["-c:v", "copy"]);
        } else {
            cmd.arg("-vf").arg(filters.join(","));
            cmd.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p"]);
        }
        cmd.args(["-c:a", "aac", "-b:a", "192k", "-shortest", "-movflags", "+faststart"]);
    } else {
        let input = if has_video { "1:a:0" } else { "0:a:0" };
        cmd.args(["-map", input, "-vn"]);
        match ext.as_str() {
            "mp3" => cmd.args(["-c:a", "libmp3lame", "-q:a", "2"]),
            "wav" => cmd.args(["-c:a", "pcm_s16le"]),
            _ => cmd.args(["-c:a", "aac", "-b:a", "192k"]),
        };
    }
    cmd.arg(&dest);

    progress("Encodage", Some(0.0));
    let result = media::feed_ffmpeg(cmd, &audio, &mut |line| {
        if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
            progress("Encodage", Some((us / 1e6 / duration).clamp(0.0, 1.0) as f32));
        }
    });
    let _ = std::fs::remove_dir_all(&tmp);
    result.with_context(|| format!("enregistrement de {}", dest.display()))
}
