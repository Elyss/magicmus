//! Un seul environnement ONNX Runtime par processus : la séparation de la voix puis
//! l'alignement des paroles doivent pouvoir l'utiliser tour à tour (création d'un
//! karaoké dans l'application). Nécessite `onnxruntime.dll` (téléchargée au premier usage).

#[test]
#[ignore]
fn environnement_partage() {
    let runtime = karaoke::setup::ensure_onnxruntime(&mut |_, _| {}).unwrap();
    let a = karaoke::setup::onnx_env(&runtime).unwrap();
    drop(a);
    // Sans partage, ce second appel échouait : « only one environment is allowed per process ».
    let b = karaoke::setup::onnx_env(&runtime).unwrap();
    let _ = ort::session::Session::builder(&b).unwrap();
}
