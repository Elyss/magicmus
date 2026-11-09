//! Bibliothèque Visual C++ requise par ONNX Runtime et whisper.cpp.

/// Sur ce PC de développement, la bibliothèque est installée et assez récente : rien
/// n'est téléchargé ni installé.
#[test]
fn bibliotheque_visual_cpp_detectee() {
    let v = karaoke::setup::vc_runtime_version();
    eprintln!("Visual C++ : {v:?}");
    if let Some((major, minor)) = v {
        assert_eq!(major, 14);
        assert!(minor >= 40, "version trop ancienne : 14.{minor}");
        karaoke::setup::ensure_vc_runtime(&mut |m, _| panic!("rien ne devrait être installé : {m}")).unwrap();
    }
}
