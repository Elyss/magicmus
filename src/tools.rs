//! Lancement des outils externes (yt-dlp, ffmpeg, ffplay, python).

use anyhow::{bail, Context, Result};
use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

/// Complète le PATH du processus pour trouver les outils même si l'application est
/// lancée depuis l'Explorateur :
///   1. dossier de l'exe et son sous-dossier `tools` (version portable : on y pose
///      yt-dlp.exe, ffmpeg.exe, ffprobe.exe, ffplay.exe) ;
///   2. outils téléchargés automatiquement (`%LOCALAPPDATA%\Karaoke\tools`) ;
///   3. PATH existant ;
///   4. dossier des alias winget (`%LOCALAPPDATA%\Microsoft\WinGet\Links`).
pub fn setup_path() {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        dirs.push(dir.join("tools"));
        dirs.push(dir);
    }
    // Outils téléchargés automatiquement (voir `setup`). Le dossier est créé
    // d'avance pour qu'il soit dans le PATH dès le premier téléchargement.
    let auto = crate::setup::tools_dir();
    let _ = std::fs::create_dir_all(&auto);
    dirs.push(auto);
    let old = std::env::var_os("PATH").unwrap_or_default();
    dirs.extend(std::env::split_paths(&old));
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Microsoft").join("WinGet").join("Links"));
    }
    dirs.retain(|d| d.is_dir());
    if let Ok(joined) = std::env::join_paths(dirs) {
        // SAFETY : appelé au tout début de `main`, avant la création de tout thread.
        unsafe { std::env::set_var("PATH", joined) };
    }
}

/// Crée une commande sans fenêtre de console (important pour l'application graphique).
pub fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Lance la commande et transmet chaque ligne de sortie (stdout et stderr, découpées
/// sur `\r` et `\n` pour suivre les barres de progression) à `on_line`.
/// En cas d'échec, l'erreur contient les dernières lignes affichées par l'outil.
pub fn run_streaming(mut cmd: Command, what: &str, on_line: &mut dyn FnMut(&str)) -> Result<()> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("impossible de lancer {what}"))?;

    let (tx, rx) = mpsc::channel();
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(spawn_line_reader(out, tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(spawn_line_reader(err, tx.clone()));
    }
    drop(tx);

    let mut tail = VecDeque::new();
    for line in rx {
        on_line(&line);
        tail.push_back(line);
        if tail.len() > 12 {
            tail.pop_front();
        }
    }
    for r in readers {
        let _ = r.join();
    }
    let status = child.wait()?;
    if !status.success() {
        let tail: Vec<String> = tail.into();
        bail!("{what} a échoué (code {:?}) :\n{}", status.code(), tail.join("\n"));
    }
    Ok(())
}

/// Lit un flux dans un thread et envoie ses lignes non vides.
pub fn spawn_line_reader(mut r: impl Read + Send + 'static, tx: Sender<String>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut line = Vec::new();
        let flush = |line: &mut Vec<u8>| {
            let s = String::from_utf8_lossy(line).trim().to_string();
            line.clear();
            if !s.is_empty() {
                let _ = tx.send(s);
            }
        };
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for &b in &buf[..n] {
                        if b == b'\n' || b == b'\r' {
                            flush(&mut line);
                        } else {
                            line.push(b);
                        }
                    }
                }
            }
        }
        flush(&mut line);
    })
}

/// Extrait un pourcentage d'une ligne de progression (« 42.3% », « 7%| ») → 0.0–1.0.
pub fn parse_percent(line: &str) -> Option<f32> {
    let end = line.find('%')?;
    let start = line[..end]
        .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map_or(0, |i| i + 1);
    let v: f32 = line[start..end].parse().ok()?;
    Some((v / 100.0).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::parse_percent;

    #[test]
    fn pourcentages() {
        assert_eq!(parse_percent("[download]  42.0% of 3.2MiB"), Some(0.42));
        assert_eq!(parse_percent(" 7%|██  | 1.2/17"), Some(0.07));
        assert_eq!(parse_percent("[download] 100% of 218KiB"), Some(1.0));
        assert_eq!(parse_percent("Destination: x.mp4"), None);
    }
}
