// Compile la bibliothèque C++ Signalsmith Stretch (MIT, intégrée dans vendor/signalsmith)
// utilisée pour changer la tonalité et la vitesse en temps réel, et intègre l'icône
// MagicMus dans les exécutables Windows.
fn main() {
    println!("cargo::rerun-if-changed=vendor/signalsmith");
    println!("cargo::rerun-if-changed=assets/magicmus.ico");
    cc::Build::new()
        .file("vendor/signalsmith/wrapper.cpp")
        .include("vendor/signalsmith/signalsmith-stretch")
        .include("vendor/signalsmith")
        .cpp(true)
        .std("c++14")
        .opt_level(3)
        .flag_if_supported("/EHsc")
        .compile("signalsmith-stretch");

    // Pile du fil principal : 16 Mo au lieu de 1 Mo par défaut sous Windows. L'interface
    // (egui, arborescence récursive de la bibliothèque) dépasse sinon 1 Mo, surtout en
    // build de débogage, et le programme s'arrête net (débordement de pile).
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo::rustc-link-arg-bins=/STACK:16777216");
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/magicmus.ico")
            .set("ProductName", "MagicMus")
            .set("FileDescription", "MagicMus — le karaoké qui fait disparaître la voix");
        if let Err(e) = res.compile() {
            println!("cargo::warning=icône non intégrée : {e}");
        }
    }
}
