//! Téléchargement via yt-dlp.

use crate::tools;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct VideoInfo {
    pub id: String,
    pub title: String,
    /// Artiste et titre du morceau, quand YouTube les connaît (clips musicaux).
    pub artist: Option<String>,
    pub track: Option<String>,
}

fn yt_dlp() -> Command {
    let mut cmd = tools::command("yt-dlp");
    // Sorties de yt-dlp en UTF-8 (titres accentués sous Windows).
    cmd.env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1").args(["--encoding", "utf-8"]);
    cmd
}

/// Récupère l'identifiant et le titre de la vidéo sans la télécharger.
pub fn fetch_info(url: &str) -> Result<VideoInfo> {
    let out = yt_dlp()
        .args(["--no-playlist", "--skip-download", "--no-warnings"])
        .args(["--print", "id", "--print", "title", "--print", "%(artist|)s", "--print", "%(track|)s"])
        .arg(url)
        .output()
        .context("impossible de lancer yt-dlp")?;
    if !out.status.success() {
        bail!(
            "yt-dlp n'a pas pu lire cette adresse :\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Une ligne par champ ; artiste et titre du morceau peuvent être vides.
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let id = lines.first().filter(|l| !l.is_empty()).context("réponse de yt-dlp vide")?.to_string();
    let title = lines.get(1).filter(|l| !l.is_empty()).map_or(id.clone(), |s| s.to_string());
    let field = |i: usize| {
        lines.get(i).map(|s| s.to_string()).filter(|s| !s.is_empty() && s != "NA")
    };
    Ok(VideoInfo { id, title, artist: field(2), track: field(3) })
}

/// Met yt-dlp à jour (`yt-dlp -U`).
pub fn self_update() -> Result<()> {
    let status = yt_dlp()
        .arg("-U")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("impossible de lancer yt-dlp")?;
    if !status.success() {
        bail!("la mise à jour de yt-dlp a échoué");
    }
    Ok(())
}

/// Télécharge la vidéo (ou l'audio seul) dans `dir/source.<ext>`.
/// Chaque ligne affichée par yt-dlp est transmise à `on_line`.
pub fn download(
    url: &str,
    dir: &Path,
    audio_only: bool,
    on_line: &mut dyn FnMut(&str),
) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    // Supprime une éventuelle source précédente (cas « forcer »).
    if let Some(old) = find_source(dir) {
        let _ = std::fs::remove_file(old);
    }

    let format = if audio_only {
        "bestaudio[ext=m4a]/bestaudio/best"
    } else {
        // H.264/AAC en MP4 de préférence (le plus compatible), 1080p max.
        "bv*[height<=1080][vcodec^=avc1]+ba[ext=m4a]/bv*[height<=1080][ext=mp4]+ba[ext=m4a]/\
         bv*[height<=1080]+ba/b[height<=1080]/b"
    };

    let mut cmd = yt_dlp();
    cmd.args(["--no-playlist", "--no-warnings", "--newline", "-f", format, "-o"])
        .arg(dir.join("source.%(ext)s"));
    if !audio_only {
        cmd.args(["--merge-output-format", "mp4"]);
    }
    cmd.arg(url);

    tools::run_streaming(cmd, "le téléchargement (yt-dlp)", on_line)?;
    find_source(dir).context("fichier téléchargé introuvable")
}

/// Cherche un fichier `source.<ext>` complet (ignore les `.part` et les fragments `source.f137.mp4`).
pub fn find_source(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            p.is_file()
                && p.file_stem().is_some_and(|s| s == "source")
                && p.extension()
                    .is_some_and(|e| !matches!(e.to_str(), Some("part" | "ytdl" | "temp")))
        })
}
