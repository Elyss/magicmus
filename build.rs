// Compile la bibliothèque C++ Signalsmith Stretch (MIT, intégrée dans vendor/signalsmith)
// utilisée pour changer la tonalité et la vitesse en temps réel.
fn main() {
    println!("cargo::rerun-if-changed=vendor/signalsmith");
    cc::Build::new()
        .file("vendor/signalsmith/wrapper.cpp")
        .include("vendor/signalsmith/signalsmith-stretch")
        .include("vendor/signalsmith")
        .cpp(true)
        .std("c++14")
        .opt_level(3)
        .flag_if_supported("/EHsc")
        .compile("signalsmith-stretch");
}
