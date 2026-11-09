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

/// Installateur officiel de la bibliothèque Microsoft Visual C++ 2015-2022 (x64),
/// requise par ONNX Runtime et whisper.cpp (MagicMus.exe, lui, l'intègre : voir
/// `.cargo/config.toml`).
const VC_REDIST_URL: &str = "https://aka.ms/vs/17/release/vc_redist.x64.exe";
/// Version minimale : ONNX Runtime 1.31 est compilé avec Visual Studio 2022 17.10+,
/// qui plante avec une bibliothèque antérieure à 14.40.
const VC_MIN_MINOR: u32 = 40;

/// Version installée (majeure, mineure) de la bibliothèque Visual C++ x64, d'après le
/// registre (`None` si absente).
pub fn vc_runtime_version() -> Option<(u32, u32)> {
    let out = crate::tools::command("reg")
        .args(["query", r"HKLM\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let dword = |name: &str| {
        text.lines().find_map(|l| {
            let mut it = l.split_whitespace();
            (it.next() == Some(name) && it.next() == Some("REG_DWORD"))
                .then(|| it.next().and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok()))
                .flatten()
        })
    };
    (dword("Installed") == Some(1)).then_some(())?;
    Some((dword("Major")?, dword("Minor")?))
}

/// Installe la bibliothèque Visual C++ si elle manque ou est trop ancienne : installateur
/// officiel Microsoft, signature vérifiée, lancé avec la demande d'autorisation de
/// Windows (une seule fois, sans autre intervention).
pub fn ensure_vc_runtime(progress: &mut dyn FnMut(&str, Option<f32>)) -> Result<()> {
    if vc_runtime_version().is_some_and(|(major, minor)| major > 14 || (major == 14 && minor >= VC_MIN_MINOR)) {
        return Ok(());
    }
    fs::create_dir_all(app_dir())?;
    let exe = app_dir().join("vc_redist.x64.exe");
    download(VC_REDIST_URL, &exe, "composant Windows « Visual C++ » (≈ 25 Mo, une seule fois)", progress)?;
    check_microsoft_signature(&exe)?;
    progress("Installation du composant Windows « Visual C++ » : acceptez la demande de Windows", None);
    // L'installateur exige les droits d'administrateur : lancement via ShellExecute
    // (« runas »), qui affiche la demande d'autorisation de Windows.
    let script = format!(
        "$p = Start-Process -FilePath '{}' -ArgumentList '/install','/passive','/norestart' -Verb RunAs -Wait -PassThru; exit $p.ExitCode",
        exe.display().to_string().replace('\'', "''")
    );
    let status = crate::tools::command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .status()
        .context("lancement de l'installateur Visual C++")?;
    let _ = fs::remove_file(&exe);
    // 0 : installé ; 3010 : installé, redémarrage conseillé ; 1638 : version plus récente déjà là.
    let code = status.code().unwrap_or(-1);
    if ![0, 3010, 1638].contains(&code) && vc_runtime_version().is_none() {
        bail!(
            "le composant Windows « Visual C++ » n'a pas pu être installé (code {code}). Installez \
             « Microsoft Visual C++ Redistributable x64 » depuis le site de Microsoft, puis relancez."
        );
    }
    Ok(())
}

/// Vérifie que l'exécutable téléchargé est signé par Microsoft (signature Authenticode valide).
fn check_microsoft_signature(path: &Path) -> Result<()> {
    let script = format!(
        "$s = Get-AuthenticodeSignature -LiteralPath '{}'; $s.Status.ToString() + '|' + $s.SignerCertificate.Subject",
        path.display().to_string().replace('\'', "''")
    );
    let out = crate::tools::command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .context("vérification de la signature")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (status, subject) = text.trim().split_once('|').unwrap_or(("", ""));
    if status != "Valid" || !subject.contains("O=Microsoft Corporation") {
        let _ = fs::remove_file(path);
        bail!("l'installateur Visual C++ téléchargé n'est pas signé par Microsoft ({status})");
    }
    Ok(())
}

/// Chemin de `onnxruntime.dll` (à côté de l'exe, sinon téléchargé au premier usage).
pub fn ensure_onnxruntime(progress: &mut dyn FnMut(&str, Option<f32>)) -> Result<PathBuf> {
    ensure_vc_runtime(progress)?;
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
    ensure_vc_runtime(progress)?;
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

/// Modèle d'alignement forcé (Meta MMS-300M, conversion ONNX fp16 de
/// MahmoudAshraf/mms-300m-1130-forced-aligner). Licence CC-BY-NC 4.0 : usage non commercial.
const ALIGNER_URL: &str = "https://huggingface.co/tatsu020/mms-300m-fa-onnx/resolve/main/mms_300m_fa_fp16.onnx";
const ALIGNER_FILE: &str = "mms_300m_fa_fp16.onnx";
const ALIGNER_SHA256: &str = "9823aa1e22dfc91fe1b1185ead5db5cab4512ce604ed18459b17a9eb92a00cd2";

/// Chemin du modèle d'alignement forcé, téléchargé et vérifié au premier usage.
pub fn ensure_aligner(progress: &mut dyn FnMut(&str, Option<f32>)) -> Result<PathBuf> {
    let path = models_dir().join(ALIGNER_FILE);
    if path.is_file() {
        return Ok(path);
    }
    fs::create_dir_all(models_dir())?;
    let tmp = models_dir().join(format!("{ALIGNER_FILE}.verif"));
    download(ALIGNER_URL, &tmp, "modèle d'alignement des paroles (≈ 630 Mo, une seule fois)", progress)?;
    progress("Vérification du modèle d'alignement", None);
    let digest = sha256_file(&tmp)?;
    if digest != ALIGNER_SHA256 {
        let _ = fs::remove_file(&tmp);
        bail!("modèle d'alignement corrompu (empreinte inattendue) : réessayez");
    }
    fs::rename(&tmp, &path)?;
    Ok(path)
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Le modèle d'alignement est-il déjà présent (pas de téléchargement) ?
pub fn aligner_present() -> bool {
    models_dir().join(ALIGNER_FILE).is_file()
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
pub(crate) fn download(url: &str, dest: &Path, label: &str, progress: &mut dyn FnMut(&str, Option<f32>)) -> Result<()> {
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
