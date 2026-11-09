//! Mises à jour de l'application : recherche de la dernière version publiée sur GitHub
//! (Releases du dépôt), téléchargement vérifié, remplacement de l'exe et relance.
//!
//! Publication d'une version : voir `CLAUDE.md` (« Publier une version »). La version
//! comparée est celle de `Cargo.toml` (`CARGO_PKG_VERSION`) et l'étiquette `vX.Y.Z`.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Dépôt GitHub public où sont publiées les versions.
pub const REPO: &str = "Elyss/magicmus";
/// Nom de l'exe joint à chaque version.
pub const ASSET: &str = "MagicMus.exe";
const USER_AGENT: &str = "MagicMus (mise a jour, https://github.com/Elyss/magicmus)";

/// Version de cet exe.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Version publiée sur GitHub.
#[derive(Clone, Debug)]
pub struct Release {
    /// Version, sans le « v » de l'étiquette.
    pub version: String,
    /// Page de la version (notes, téléchargement manuel).
    pub page: String,
    /// Notes de version (Markdown).
    pub notes: String,
    url: String,
    size: u64,
    /// Empreinte SHA-256 attendue (fournie par GitHub, ou fichier `MagicMus.exe.sha256`).
    sha256: Option<String>,
}

/// « 1.2.3 » → (1, 2, 3) ; un suffixe (« -beta ») est ignoré.
fn parse(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)))
}

/// `remote` est-elle plus récente que `local` ?
pub fn is_newer(remote: &str, local: &str) -> bool {
    matches!((parse(remote), parse(local)), (Some(r), Some(l)) if r > l)
}

/// Dernière version publiée, si elle est plus récente que cet exe. Désactivé par
/// `MAGICMUS_NO_UPDATE` (et dans les compilations `demo`, sauf avec `MAGICMUS_UPDATE_API`).
pub fn check() -> Result<Option<Release>> {
    let test_api = std::env::var_os("MAGICMUS_UPDATE_API").is_some();
    if std::env::var_os("MAGICMUS_NO_UPDATE").is_some() || (cfg!(feature = "demo") && !test_api) {
        return Ok(None);
    }
    let rel = latest()?;
    Ok(rel.filter(|r| is_newer(&r.version, current_version())))
}

/// Dernière version publiée (`None` s'il n'y en a pas encore).
pub fn latest() -> Result<Option<Release>> {
    // `MAGICMUS_UPDATE_API` : autre adresse (tests avec un serveur local).
    let url = std::env::var("MAGICMUS_UPDATE_API")
        .unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}/releases/latest"));
    let agent = ureq::Agent::new_with_defaults();
    let resp = agent
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .context("recherche de mise à jour")?;
    if resp.status() == 404 {
        return Ok(None);
    }
    if !resp.status().is_success() {
        bail!("recherche de mise à jour : réponse {}", resp.status());
    }
    let text = resp.into_body().read_to_string().context("réponse GitHub illisible")?;
    let json: serde_json::Value = serde_json::from_str(&text).context("réponse GitHub illisible")?;
    let tag = json["tag_name"].as_str().unwrap_or_default();
    let assets = json["assets"].as_array().cloned().unwrap_or_default();
    let Some(exe) = assets.iter().find(|a| a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(ASSET))) else {
        return Ok(None);
    };
    // Empreinte : champ « digest » de GitHub, sinon fichier « MagicMus.exe.sha256 ».
    let mut sha256 = exe["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_lowercase);
    if sha256.is_none() {
        let side = format!("{ASSET}.sha256");
        if let Some(u) = assets
            .iter()
            .find(|a| a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(&side)))
            .and_then(|a| a["browser_download_url"].as_str())
        {
            let text = agent
                .get(u)
                .header("User-Agent", USER_AGENT)
                .call()
                .context("empreinte de la mise à jour")?
                .into_body()
                .read_to_string()?;
            sha256 = text.split_whitespace().next().map(str::to_lowercase);
        }
    }
    Ok(Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        page: json["html_url"].as_str().unwrap_or_default().to_string(),
        notes: json["body"].as_str().unwrap_or_default().to_string(),
        url: exe["browser_download_url"].as_str().unwrap_or_default().to_string(),
        size: exe["size"].as_u64().unwrap_or(0),
        sha256,
    }))
}

/// Nom de l'ancien exe, gardé le temps de la relance.
fn old_path(exe: &Path) -> PathBuf {
    let stem = exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "MagicMus".into());
    exe.with_file_name(format!("{stem}.avant-maj.exe"))
}

/// Supprime l'ancien exe laissé par la dernière mise à jour (au démarrage).
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_path(&exe));
    }
}

/// Télécharge la version, vérifie son empreinte, puis remplace l'exe en cours (Windows
/// permet de renommer un exe en cours d'exécution). Renvoie le chemin à relancer.
pub fn install(rel: &Release, progress: &mut dyn FnMut(&str, Option<f32>)) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("emplacement de l'application")?;
    install_to(rel, &exe, progress)?;
    Ok(exe)
}

/// Comme [`install`], pour un exe donné (tests).
pub fn install_to(rel: &Release, exe: &Path, progress: &mut dyn FnMut(&str, Option<f32>)) -> Result<()> {
    let Some(expected) = &rel.sha256 else {
        bail!("la version {} n'indique pas d'empreinte SHA-256 : installation refusée", rel.version);
    };
    let new = exe.with_file_name(format!("{ASSET}.nouveau"));
    let label = format!("MagicMus {} (≈ {} Mo)", rel.version, rel.size.div_ceil(1 << 20));
    crate::setup::download(&rel.url, &new, &label, progress)?;
    let got = crate::setup::sha256_file(&new)?;
    if !got.eq_ignore_ascii_case(expected) {
        let _ = std::fs::remove_file(&new);
        bail!("mise à jour corrompue (empreinte inattendue) : réessayez plus tard");
    }
    let old = old_path(exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).with_context(|| {
        format!(
            "impossible de remplacer {} (dossier protégé ?) : téléchargez la version sur {}",
            exe.display(),
            rel.page
        )
    })?;
    if let Err(e) = std::fs::rename(&new, exe) {
        // Remise en place de l'exe d'origine.
        let _ = std::fs::rename(&old, exe);
        return Err(e).context("installation de la mise à jour");
    }
    Ok(())
}

/// Relance l'application (après [`install`]).
pub fn relaunch(exe: &Path) -> Result<()> {
    std::process::Command::new(exe).spawn().context("relance de l'application")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparaison_des_versions() {
        assert!(is_newer("v1.0.1", "1.0.0"));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2", "1.9.9"));
        assert!(!is_newer("v1.0.0", "1.0.0"));
        assert!(!is_newer("0.9.0", "1.0.0"));
        assert!(!is_newer("1.0.0-beta", "1.0.0"));
        assert!(!is_newer("n'importe quoi", "1.0.0"));
    }

    /// Remplacement d'un exe factice : l'ancien est gardé à côté, le nouveau vérifié.
    #[test]
    fn remplacement_verifie() {
        let dir = std::env::temp_dir().join(format!("magicmus-maj-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("MagicMus.exe");
        std::fs::write(&exe, b"ancien").unwrap();
        // Source locale servie par un mini serveur HTTP.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                use std::io::{Read, Write};
                let mut s = stream.unwrap();
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let body = b"nouveau";
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(body);
            }
        });
        let mut rel = Release {
            version: "9.9.9".into(),
            page: String::new(),
            notes: String::new(),
            url: format!("http://127.0.0.1:{port}/MagicMus.exe"),
            size: 7,
            // SHA-256 de « nouveau » volontairement faux d'abord.
            sha256: Some("00".repeat(32)),
        };
        assert!(install_to(&rel, &exe, &mut |_, _| {}).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"ancien");
        rel.sha256 = Some({
            use sha2::{Digest, Sha256};
            Sha256::digest(b"nouveau").iter().map(|b| format!("{b:02x}")).collect()
        });
        install_to(&rel, &exe, &mut |_, _| {}).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"nouveau");
        assert_eq!(std::fs::read(old_path(&exe)).unwrap(), b"ancien");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
