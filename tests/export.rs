//! Test d'intégration de l'export (nécessite ffmpeg et un karaoké existant).
//! Lancement : `KARAOKE_TEST_FILE=<karaoke.mp4> cargo test --release --test export -- --ignored`

use karaoke::export::{export, ExportSettings};
use karaoke::lyrics::{group_lines, Lyrics, Word};
use std::path::PathBuf;

/// Paroles factices (chiffres) pour vérifier l'incrustation et la synchronisation.
fn fake_lyrics() -> Lyrics {
    let names = ["un", "deux", "trois", "quatre", "cinq", "six", "sept", "huit"];
    let words = (0..40)
        .map(|i| {
            let start = 2.0 + i as f64 * 0.5 + (i / 8) as f64 * 1.5;
            Word { text: names[i % 8].to_string(), start, end: start + 0.45 }
        })
        .collect();
    Lyrics { language: "fr".into(), lines: group_lines(words), offset: 0.0, source: String::new() }
}

#[test]
#[ignore]
fn export_tonalite_vitesse_paroles() {
    let src = PathBuf::from(std::env::var("KARAOKE_TEST_FILE").expect("KARAOKE_TEST_FILE"));
    let dest = src.with_file_name("export_test.mp4");
    let ly = fake_lyrics();
    ly.save(&src.with_file_name("paroles.json")).unwrap();
    let s = ExportSettings { tempo: 1.2, semitones: 2.0, burn_lyrics: true, highlight: [0xf7, 0xa6, 0xc9] };
    export(&src, true, Some(&ly), &s, &dest, &mut |stage, p| eprintln!("{stage} {p:?}")).unwrap();
    assert!(dest.is_file());
}
