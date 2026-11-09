//! Décodage / encodage via ffmpeg, lecture via ffplay.

use crate::tools;
use anyhow::{bail, Context, Result};
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;

pub const SAMPLE_RATE: u32 = 44_100;

/// Vérifie qu'un outil externe est installé et accessible.
pub fn require_tool(name: &str, version_arg: &str) -> Result<()> {
    match tools::command(name)
        .arg(version_arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => bail!(
            "« {name} » est introuvable. Installez-le (Windows : `winget install {}`) \
             ou placez {name}.exe à côté de l'application.",
            match name {
                "yt-dlp" => "yt-dlp.yt-dlp",
                _ => "Gyan.FFmpeg",
            }
        ),
        Err(e) => Err(e).with_context(|| format!("impossible de lancer {name}")),
    }
}

/// Le fichier contient-il une vraie piste vidéo (hors pochette d'album) ?
pub fn has_video(path: &Path) -> bool {
    tools::command("ffprobe")
        .args(["-v", "error", "-select_streams", "V", "-show_entries", "stream=index"])
        .args(["-of", "csv=p=0"])
        .arg(path)
        .output()
        .map(|o| o.status.success() && !o.stdout.iter().all(u8::is_ascii_whitespace))
        // Sans ffprobe, on se fie à l'extension.
        .unwrap_or_else(|_| {
            matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("mp4" | "mkv" | "webm" | "mov" | "avi")
            )
        })
}

/// Décode la piste audio en PCM f32 stéréo entrelacé (L, R, L, R…) à 44,1 kHz.
pub fn decode_pcm(path: &Path) -> Result<Vec<f32>> {
    decode_pcm_at(path, SAMPLE_RATE)
}

/// Décode la piste audio en PCM f32 stéréo entrelacé à la fréquence demandée.
pub fn decode_pcm_at(path: &Path, sample_rate: u32) -> Result<Vec<f32>> {
    let out = tools::command("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-vn", "-ac", "2", "-ar", &sample_rate.to_string(), "-f", "f32le", "pipe:1"])
        .output()
        .context("impossible de lancer ffmpeg")?;
    if !out.status.success() {
        bail!(
            "ffmpeg n'a pas pu décoder {} :\n{}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect())
}

/// Options de sortie communes : audio AAC, et vidéo copiée telle quelle si présente.
fn output_args(cmd: &mut Command, with_video: bool, output: &Path) {
    if with_video {
        cmd.args(["-map", "0:v:0", "-map", "1:a:0", "-c:v", "copy"]);
    } else {
        cmd.args(["-map", "1:a:0", "-vn"]);
    }
    cmd.args(["-c:a", "aac", "-b:a", "192k", "-shortest", "-movflags", "+faststart"])
        .arg(output);
}

/// Encode le PCM traité et le remultiplexe avec la vidéo d'origine.
pub fn encode_pcm(source: &Path, pcm: &[f32], output: &Path, with_video: bool) -> Result<()> {
    let mut cmd = tools::command("ffmpeg");
    cmd.args(["-v", "error", "-y", "-i"]).arg(source);
    cmd.args(["-f", "f32le", "-ar", &SAMPLE_RATE.to_string(), "-ac", "2", "-i", "pipe:0"]);
    output_args(&mut cmd, with_video, output);
    feed_ffmpeg(cmd, pcm, &mut |_| {})
        .with_context(|| format!("ffmpeg n'a pas pu encoder {}", output.display()))
}

/// Lance ffmpeg (dont une entrée est `pipe:0` en f32le) en lui fournissant `pcm`.
/// Les lignes de stderr sont transmises à `on_line` (progression avec `-progress pipe:2`).
pub fn feed_ffmpeg(mut cmd: Command, pcm: &[f32], on_line: &mut dyn FnMut(&str)) -> Result<()> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("impossible de lancer ffmpeg")?;

    // stderr est lu dans un thread pour qu'ffmpeg ne se bloque jamais en écriture ;
    // l'écriture de l'audio se fait aussi dans un thread pour pouvoir suivre la progression.
    let (tx, rx) = mpsc::channel();
    let reader = child.stderr.take().map(|e| tools::spawn_line_reader(e, tx));
    let mut stdin = child.stdin.take().context("stdin ffmpeg indisponible")?;
    let mut tail = std::collections::VecDeque::new();
    let write_result = std::thread::scope(|scope| {
        let writer = scope.spawn(move || -> std::io::Result<()> {
            let mut buf = Vec::with_capacity(1 << 18);
            for chunk in pcm.chunks(1 << 16) {
                buf.clear();
                buf.extend(chunk.iter().flat_map(|s| s.to_le_bytes()));
                stdin.write_all(&buf)?;
            }
            Ok(())
        }); // stdin est fermé à la fin du thread => fin de flux pour ffmpeg
        for line in rx.iter() {
            on_line(&line);
            tail.push_back(line);
            if tail.len() > 12 {
                tail.pop_front();
            }
        }
        writer.join().unwrap_or(Ok(()))
    });
    let status = child.wait()?;
    if let Some(r) = reader {
        let _ = r.join();
    }
    if !status.success() || write_result.is_err() {
        let tail: Vec<String> = tail.into();
        bail!("ffmpeg a échoué :\n{}", tail.join("\n"));
    }
    Ok(())
}

/// Écrit un WAV mono 16 kHz (format attendu par Whisper) à partir de PCM stéréo.
pub fn write_wav_16k_mono(pcm: &[f32], sample_rate: u32, wav: &Path) -> Result<()> {
    let mut cmd = tools::command("ffmpeg");
    cmd.args(["-v", "error", "-y", "-f", "f32le", "-ar", &sample_rate.to_string(), "-ac", "2", "-i", "pipe:0"])
        .args(["-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .arg(wav);
    feed_ffmpeg(cmd, pcm, &mut |_| {}).context("création du WAV pour la reconnaissance des paroles")
}

/// Convertit du PCM stéréo entrelacé en mono 16 kHz (rééchantillonnage ffmpeg).
pub fn to_mono_16k(pcm: &[f32], sample_rate: u32) -> Result<Vec<f32>> {
    let tmp = std::env::temp_dir().join(format!("magicmus-16k-{}-{}.raw", std::process::id(), crate::library::now()));
    let mut cmd = tools::command("ffmpeg");
    cmd.args(["-v", "error", "-y", "-f", "f32le", "-ar", &sample_rate.to_string(), "-ac", "2", "-i", "pipe:0"])
        .args(["-ac", "1", "-ar", "16000", "-f", "f32le"])
        .arg(&tmp);
    let res = feed_ffmpeg(cmd, pcm, &mut |_| {}).context("conversion en 16 kHz mono");
    let bytes = std::fs::read(&tmp);
    let _ = std::fs::remove_file(&tmp);
    res?;
    Ok(bytes?.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

/// Durée du fichier en secondes.
pub fn duration(path: &Path) -> Option<f64> {
    let out = tools::command("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Dimensions (largeur, hauteur) de la vidéo, si le fichier en contient une.
pub fn video_size(path: &Path) -> Option<(u32, u32)> {
    let out = tools::command("ffprobe")
        .args(["-v", "error", "-select_streams", "V:0", "-show_entries", "stream=width,height"])
        .args(["-of", "csv=p=0:s=x"])
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (w, h) = text.trim().lines().next()?.split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// Remplace la piste audio de `source` par le fichier `audio`.
pub fn mux_audio_file(source: &Path, audio: &Path, output: &Path, with_video: bool) -> Result<()> {
    let mut cmd = tools::command("ffmpeg");
    cmd.args(["-v", "error", "-y", "-nostdin", "-i"])
        .arg(source)
        .arg("-i")
        .arg(audio);
    output_args(&mut cmd, with_video, output);
    tools::run_streaming(cmd, "l'encodage (ffmpeg)", &mut |_| {})
}

/// Lit le fichier avec ffplay (ou, à défaut, avec le lecteur par défaut du système).
/// Si `wait` est faux, la lecture se fait en arrière-plan.
pub fn play(path: &Path, title: &str, wait: bool) -> Result<()> {
    let res = tools::command("ffplay")
        .args(["-autoexit", "-loglevel", "quiet", "-window_title", title])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match res {
        Ok(mut child) => {
            if wait {
                child.wait()?;
            }
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::NotFound => open_with_default_app(path),
        Err(e) => Err(e).context("impossible de lancer ffplay"),
    }
}

/// Ouvre un fichier avec l'application associée par le système.
pub fn open_with_default_app(path: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = tools::command("cmd");
        c.args(["/C", "start", ""]).arg(path);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(path);
        c
    };
    cmd.spawn().context("impossible d'ouvrir le fichier")?;
    Ok(())
}

/// Montre le fichier dans l'Explorateur (ou ouvre son dossier).
pub fn reveal(path: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn()
            .context("impossible d'ouvrir l'Explorateur")?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    open_with_default_app(path.parent().unwrap_or(path))
}
