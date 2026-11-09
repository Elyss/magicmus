//! Diagnostic de synchronisation des paroles d'un karaoké déjà créé.
//! N'affiche que des minutages et des compteurs (jamais le texte des paroles).
//!
//! cargo run --release --example diag_paroles -- <dossier de la chanson>

use karaoke::{align, lrclib, lyrics, media};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).expect("dossier de la chanson"));
    karaoke::tools::setup_path();
    let title = std::fs::read_to_string(dir.join("titre.txt"))?.trim().to_string();
    let output = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("karaoke_")))
        .expect("fichier karaoké");
    let duration = media::duration(&output).unwrap_or(0.0);
    println!("durée de la vidéo : {duration:.1} s");

    let (_, rec) = lyrics::read_whisper(&dir.join(lyrics::WHISPER_JSON), "auto")?;
    println!("mots reconnus par Whisper : {} (de {:.1} s à {:.1} s)", rec.len(), rec[0].start, rec[rec.len() - 1].start);

    // Paroles actuelles : ordre, chevauchements et écart avec la voix entendue
    if let Ok(ly) = lyrics::Lyrics::load(&dir.join("paroles.json")) {
        let words: Vec<_> = ly.lines.iter().flat_map(|l| l.words.iter()).collect();
        let back = words.windows(2).filter(|w| w[1].start < w[0].start).count();
        let overlap = words.windows(2).filter(|w| w[0].end > w[1].start + 0.01).count();
        println!("paroles.json : {} lignes, {} mots, {back} débuts en arrière, {overlap} chevauchements", ly.lines.len(), words.len());
        // Premier mot de chaque ligne : instant d'allumage − instant entendu (même mot à ±3 s).
        let mut gaps: Vec<f64> = ly
            .lines
            .iter()
            .filter_map(|l| {
                let w0 = l.words.first()?;
                let target = align::normalize(&w0.text);
                rec.iter()
                    .filter(|r| align::normalize(&r.text) == target && (r.start - w0.start).abs() < 3.0)
                    .map(|r| w0.start - r.start)
                    .min_by(|a, b| a.abs().total_cmp(&b.abs()))
            })
            .collect();
        gaps.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
        if !gaps.is_empty() {
            let med = gaps[gaps.len() / 2].abs();
            let over = |x: f64| gaps.iter().filter(|g| g.abs() > x).count();
            println!(
                "écart allumage − voix (premier mot de {} lignes) : médiane {med:.2} s, > 0,3 s : {}, > 0,7 s : {}",
                gaps.len(),
                over(0.3),
                over(0.7)
            );
        }
    }
    if std::env::args().any(|a| a == "--resume") {
        return Ok(());
    }

    // LRC trouvé en ligne : écart entre l'heure du vers et l'heure entendue de son premier mot
    let Some(m) = lrclib::find(&title, None, None, duration)? else {
        println!("aucune paroles en ligne");
        return Ok(());
    };
    let lrclib::Found::Synced(lines) = m.lyrics else {
        println!("paroles en ligne non synchronisées");
        return Ok(());
    };
    println!("LRC : {} vers, dernier à {:.1} s", lines.len(), lines.last().map_or(0.0, |l| l.0));
    println!("vers | heure LRC | premier mot entendu | écart");
    for (i, (t, text)) in lines.iter().enumerate() {
        let Some(first) = text.split_whitespace().next() else { continue };
        let target = align::normalize(first);
        let hit = rec
            .iter()
            .filter(|w| align::normalize(&w.text) == target && (w.start - t).abs() < 30.0)
            .min_by(|a, b| (a.start - t).abs().total_cmp(&(b.start - t).abs()));
        match hit {
            Some(w) => println!("{i:4} | {t:8.2} | {:8.2} | {:+6.2}", w.start, w.start - t),
            None => println!("{i:4} | {t:8.2} |        – |      –"),
        }
    }
    Ok(())
}
