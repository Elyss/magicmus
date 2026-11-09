//! Installation automatique des dépendances, sans action de l'utilisateur :
//! yt-dlp, ffmpeg (avec ffprobe et ffplay), Deno (requis par yt-dlp pour YouTube)
//! et le modèle d'IA de séparation voix/musique.
//!
//! Tout est placé dans `%LOCALAPPDATA%\Karaoke` et téléchargé une seule fois.

use anyhow::{bail, Context, Result};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
const FFMPEG_URL: &str =
    "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl-shared.zip";
const DENO_URL: &str =
    "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip";
pub const MODEL_URL: &str =
    "https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models/UVR-MDX-NET-Inst_HQ_3.onnx";
pub const MODEL_FILE: &str = "UVR-MDX-NET-Inst_HQ_3.onnx";
/// Moteur d'exécution des réseaux de neurones (Microsoft ONNX Runtime, CPU).
const ORT_VERSION: &str = "1.31.1";
const ORT_DLL: &str = "onnxruntime.dll";

/// Dossier des données de l'application.
pub fn app_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Karaoke")
}

/// Dossier des outils téléchargés (ajouté au PATH par `tools::setup_path`).
pub fn tools_dir() -> PathBuf {
    app_dir().join("tools")
}

pub fn models_dir() -> PathBuf {
    app_dir().join("models")
}

/// Un outil est-il utilisable (dans le PATH, à côté de l'exe ou déjà téléchargé) ?
pub fn tool_available(name: &str) -> bool {
    crate::tools::command(name)
        .arg(if name == "ffmpeg" || name == "ffprobe" || name == "ffplay" { "-version" } else { "--version" })
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Vérifie les outils nécessaires et télécharge ceux qui manquent.
/// `progress(message, fraction)` est appelé pendant les téléchargements.
pub fn ensure_tools(need_ytdlp: bool, progress: &mut dyn FnMut(&str, Option<f32>)) -> Result<()> {
    let dir = tools_dir();
    fs::create_dir_all(&dir)?;

    if !(tool_available("ffmpeg") && tool_available("ffprobe")) {
        let zip = dir.join("ffmpeg.zip");
        download(FFMPEG_URL, &zip, "ffmpeg (≈ 90 Mo, une seule fois)", progress)?;
        progress("Décompression de ffmpeg", None);
        extract_zip(&zip, &dir, |name| {
            name.contains("/bin/") && (name.ends_with(".exe") || name.ends_with(".dll"))
        })?;
        let _ = fs::remove_file(&zip);
    }
    if need_ytdlp {
        if !tool_available("yt-dlp") {
            download(YTDLP_URL, &dir.join("yt-dlp.exe"), "yt-dlp (≈ 25 Mo, une seule fois)", progress)?;
        }
        if !tool_available("deno") {
            let zip = dir.join("deno.zip");
            download(DENO_URL, &zip, "Deno (≈ 45 Mo, une seule fois)", progress)?;
            progress("Décompression de Deno", None);
            extract_zip(&zip, &dir, |name| name.ends_with("deno.exe"))?;
            let _ = fs::remove_file(&zip);
        }
    }
    for t in ["ffmpeg", "ffprobe"].into_iter().chain(need_ytdlp.then_some("yt-dlp")) {
        if !tool_available(t) {
            bail!("{t} reste introuvable après l'installation automatique");
        }
    }
    Ok(())
}

/// Chemin de `onnxruntime.dll` (à côté de l'exe, sinon téléchargé au premier usage).
pub fn ensure_onnxruntime(progress: &mut dyn FnMut(&str, Option<f32>)) -> Result<PathBuf> {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    if let Some(dir) = exe_dir {
        for p in [dir.join(ORT_DLL), dir.join("tools").join(ORT_DLL)] {
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    let dest_dir = tools_dir().join(format!("onnxruntime-{ORT_VERSION}"));
    let dll = dest_dir.join(ORT_DLL);
    if !dll.is_file() {
        fs::create_dir_all(&dest_dir)?;
        let url = format!(
            "https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/onnxruntime-win-x64-{ORT_VERSION}.zip"
        );
        let zip = dest_dir.join("onnxruntime.zip");
        download(&url, &zip, "moteur d'IA ONNX Runtime (≈ 80 Mo, une seule fois)", progress)?;
        progress("Décompression du moteur d'IA", None);
        extract_zip(&zip, &dest_dir, |name| name.contains("/lib/") && name.ends_with(".dll"))?;
        let _ = fs::remove_file(&zip);
        if !dll.is_file() {
            bail!("{ORT_DLL} introuvable dans l'archive ONNX Runtime");
        }
    }
    Ok(dll)
}

/// Reconnaissance vocale (whisper.cpp) : exécutable, modèle de transcription et
/// modèle de détection de voix (VAD), téléchargés au premier usage.
pub struct Whisper {
    pub cli: PathBuf,
    pub model: PathBuf,
    pub vad: PathBuf,
}

const WHISPER_BIN_URL: &str =
    "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip";
const WHISPER_MODEL: &str = "ggml-large-v3-turbo-q5_0.bin";
const WHISPER_MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin";
const VAD_MODEL: &str = "ggml-silero-v5.1.2.bin";
const VAD_MODEL_URL: &str = "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin";

pub fn ensure_whisper(progress: &mut dyn FnMut(&str, Option<f32>)) -> Result<Whisper> {
    let dir = tools_dir().join("whisper");
    let cli = dir.join("whisper-cli.exe");
    if !cli.is_file() {
        fs::create_dir_all(&dir)?;
        let zip = dir.join("whisper.zip");
        download(WHISPER_BIN_URL, &zip, "reconnaissance des paroles (≈ 9 Mo, une seule fois)", progress)?;
        progress("Décompression de la reconnaissance des paroles", None);
        extract_zip(&zip, &dir, |name| {
            let file = name.rsplit('/').next().unwrap_or(name);
            file == "whisper-cli.exe" || file.ends_with(".dll")
        })?;
        let _ = fs::remove_file(&zip);
        if !cli.is_file() {
            bail!("whisper-cli.exe introuvable dans l'archive whisper.cpp");
        }
    }
    fs::create_dir_all(models_dir())?;
    let model = models_dir().join(WHISPER_MODEL);
    if !model.is_file() {
        download(WHISPER_MODEL_URL, &model, "modèle de reconnaissance des paroles (≈ 550 Mo, une seule fois)", progress)?;
    }
    let vad = models_dir().join(VAD_MODEL);
    if !vad.is_file() {
        download(VAD_MODEL_URL, &vad, "détecteur de voix (≈ 1 Mo, une seule fois)", progress)?;
    }
    Ok(Whisper { cli, model, vad })
}

/// Chemin du modèle d'IA, téléchargé au premier usage.
pub fn ensure_model(progress: &mut dyn FnMut(&str, Option<f32>)) -> Result<PathBuf> {
    // Un modèle posé à côté de l'exe (version hors ligne) est prioritaire.
    if let Some(local) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("models").join(MODEL_FILE)))
        .filter(|p| p.is_file())
    {
        return Ok(local);
    }
    let path = models_dir().join(MODEL_FILE);
    if !path.is_file() {
        fs::create_dir_all(models_dir())?;
        download(MODEL_URL, &path, "modèle d'IA (≈ 65 Mo, une seule fois)", progress)?;
    }
    Ok(path)
}

/// Télécharge `url` vers `dest` (via un fichier temporaire, pour ne jamais laisser
/// un fichier incomplet).
fn download(url: &str, dest: &Path, label: &str, progress: &mut dyn FnMut(&str, Option<f32>)) -> Result<()> {
    let msg = format!("Téléchargement de {label}");
    progress(&msg, Some(0.0));
    let resp = ureq::get(url)
        .call()
        .with_context(|| format!("impossible de télécharger {url} (connexion Internet ?)"))?;
    let total: Option<u64> = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let mut reader = resp.into_body().into_reader();

    let tmp = dest.with_extension("download");
    let mut file = File::create(&tmp).with_context(|| format!("impossible de créer {}", tmp.display()))?;
    let mut buf = vec![0u8; 1 << 16];
    let (mut done, mut last) = (0u64, -1i64);
    loop {
        let n = reader.read(&mut buf).context("téléchargement interrompu")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        if let Some(t) = total {
            let pct = (done * 100 / t.max(1)) as i64;
            if pct != last {
                last = pct;
                progress(&msg, Some(done as f32 / t as f32));
            }
        }
    }
    file.flush()?;
    drop(file);
    if let Some(t) = total {
        if done != t {
            let _ = fs::remove_file(&tmp);
            bail!("téléchargement incomplet de {url} ({done} / {t} octets)");
        }
    }
    fs::rename(&tmp, dest)?;
    Ok(())
}

/// Extrait à plat (sans l'arborescence) les fichiers de l'archive retenus par `keep`.
fn extract_zip(zip: &Path, dest: &Path, keep: impl Fn(&str) -> bool) -> Result<()> {
    let mut archive = zip::ZipArchive::new(File::open(zip)?).context("archive corrompue")?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name()?.replace('\\', "/");
        if entry.is_dir() || !keep(&name) {
            continue;
        }
        let Some(file_name) = Path::new(&name).file_name() else { continue };
        let mut out = File::create(dest.join(file_name))?;
        io::copy(&mut entry, &mut out)?;
    }
    Ok(())
}
