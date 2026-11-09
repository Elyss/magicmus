//! Mise à jour : recherche sur une fausse API GitHub locale, puis installation vérifiée.

use std::io::{Read, Write};

/// Mini serveur HTTP local : `/latest` renvoie `json(port)` (format de l'API GitHub),
/// toute autre adresse renvoie `exe`. Renvoie le port.
fn serve(json: impl Fn(u16) -> String, exe: Vec<u8>) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let json = json(port);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = stream.unwrap();
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let body: Vec<u8> = if req.starts_with("GET /latest") { json.clone().into_bytes() } else { exe.clone() };
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(&body);
        }
    });
    port
}

#[test]
fn recherche_et_installation_depuis_une_fausse_api() {
    use sha2::{Digest, Sha256};
    let exe_bytes = b"MZ nouvelle version".to_vec();
    let digest: String = Sha256::digest(&exe_bytes).iter().map(|b| format!("{b:02x}")).collect();
    let size = exe_bytes.len();
    let port = serve(
        |port| {
            format!(
                r#"{{"tag_name":"v99.0.0","html_url":"https://example.invalid/v99","body":"notes",
                    "assets":[{{"name":"MagicMus.exe","size":{size},"digest":"sha256:{digest}",
                    "browser_download_url":"http://127.0.0.1:{port}/MagicMus.exe"}}]}}"#
            )
        },
        exe_bytes.clone(),
    );
    // SAFETY : un seul test modifie l'environnement dans ce fichier (processus à part).
    unsafe { std::env::set_var("MAGICMUS_UPDATE_API", format!("http://127.0.0.1:{port}/latest")) };
    let rel = karaoke::update::check().unwrap().expect("version plus récente proposée");
    assert_eq!(rel.version, "99.0.0");

    let dir = std::env::temp_dir().join(format!("magicmus-maj-it-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("MagicMus.exe");
    std::fs::write(&exe, b"MZ ancienne version").unwrap();
    karaoke::update::install_to(&rel, &exe, &mut |_, _| {}).unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), exe_bytes);
    let _ = std::fs::remove_dir_all(&dir);
}

/// La vraie API GitHub répond (aucune version publiée : rien n'est proposé).
#[test]
#[ignore]
fn api_github_reelle() {
    let r = karaoke::update::latest().unwrap();
    eprintln!("dernière version publiée : {:?}", r.map(|r| r.version));
}
