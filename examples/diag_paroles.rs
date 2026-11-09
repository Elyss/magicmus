//! Diagnostic de synchronisation des paroles d'un karaoké déjà créé.
//! N'affiche que des minutages et des compteurs (jamais le texte des paroles).
//!
//! cargo run --release --example diag_paroles -- <dossier de la chanson>

use karaoke::{align, lrclib, lyrics, media};
use std::path::PathBuf;

/// Vérification de l'alignement forcé sur de la parole synthétique dont on connaît
/// l'heure exacte de chaque mot : `--tts <extrait.wav> <mots.json>`.
fn tts_check(wav: &str, json: &str) -> anyhow::Result<()> {
    karaoke::tools::setup_path();
    let bytes = std::fs::read(wav)?;
    // WAV PCM 16 bits mono 16 kHz : recherche du bloc « data ».
    let pos = bytes.windows(4).position(|w| w == b"data").expect("bloc data") + 8;
    let mono: Vec<f32> = bytes[pos..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect();
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(json)?)?;
    let mut truth: Vec<(String, f64)> = v["words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| (w["text"].as_str().unwrap_or("").to_string(), w["start"].as_f64().unwrap_or(0.0)))
        .collect();
    truth.sort_by(|a, b| a.1.total_cmp(&b.1));
    let text = v["text"].as_str().unwrap();
    let mut prog = |_: &str, _: Option<f32>| {};
    let runtime = karaoke::setup::ensure_onnxruntime(&mut prog)?;
    let model = karaoke::setup::ensure_aligner(&mut prog)?;
    let em = karaoke::forced::emissions(&runtime, &model, &mono, &mut |_| {})?;
    let text_lines: Vec<align::TextLine> = text.lines().map(|l| align::TextLine { start: None, text: l.into() }).collect();
    let lines = karaoke::forced::align_lines(&text_lines, &em, None).expect("alignement");
    let got: Vec<&karaoke::lyrics::Word> = lines.iter().flat_map(|l| l.words.iter()).collect();
    // Appariement dans l'ordre par mot normalisé.
    let mut diffs = Vec::new();
    let mut j = 0;
    for (w, t) in &truth {
        let n = align::normalize(w);
        if let Some(k) = got[j..].iter().position(|g| align::normalize(&g.text) == n) {
            diffs.push(got[j + k].start - t);
            j += k + 1;
        }
    }
    let mut abs: Vec<f64> = diffs.iter().map(|d| d.abs()).collect();
    abs.sort_by(f64::total_cmp);
    let mut signed = diffs.clone();
    signed.sort_by(f64::total_cmp);
    let within = |x: f64| abs.iter().filter(|e| **e <= x).count() as f64 * 100.0 / abs.len().max(1) as f64;
    println!(
        "{} : {} mots comparés · écart médian {:.3} s (signé {:+.3} s) · ≤ 0,1 s : {:.0} % · ≤ 0,2 s : {:.0} % · max {:.2} s",
        std::path::Path::new(wav).file_name().unwrap().to_string_lossy(),
        abs.len(),
        abs[abs.len() / 2],
        signed[signed.len() / 2],
        within(0.1),
        within(0.2),
        abs.last().unwrap()
    );
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "--tts") {
        return tts_check(&args[2], &args[3]);
    }
    let dir = PathBuf::from(std::env::args().nth(1).expect("dossier de la chanson"));
    karaoke::tools::setup_path();
    let title = std::fs::read_to_string(dir.join("titre.txt"))?.trim().to_string();
    let output = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("karaoke_")))
        .expect("fichier karaoké");
    let duration = media::duration(&output).unwrap_or(0.0);
    println!("durée de la vidéo : {duration:.1} s");

    // Même traitement que le bouton « Resynchroniser » (paroles.json est sauvegardé avant).
    if std::env::args().any(|a| a == "--ecrire") {
        let file = dir.join(karaoke::pipeline::LYRICS_FILE);
        let old = lyrics::Lyrics::load(&file).ok();
        if file.exists() && !dir.join("paroles.avant-mms.json").exists() {
            std::fs::copy(&file, dir.join("paroles.avant-mms.json"))?;
        }
        let language = old.as_ref().map(|l| l.language.clone()).filter(|l| !l.is_empty()).unwrap_or_else(|| "auto".into());
        let req = karaoke::pipeline::LyricsRequest {
            work_dir: &dir,
            title: &title,
            artist: None,
            track: None,
            duration,
            language: &language,
            user_text: None,
            online: true,
            karaoke: Some(&output),
        };
        let t0 = std::time::Instant::now();
        let mut ly = karaoke::pipeline::build_lyrics(&req, &mut |e| {
            match e {
                karaoke::pipeline::Event::Warning(w) => println!("avertissement : {w}"),
                karaoke::pipeline::Event::Log(w) if w.contains("secours") => println!("{w}"),
                _ => {}
            }
        })
        .expect("paroles");
        ly.offset = old.map_or(0.0, |l| l.offset);
        ly.save(&file)?;
        println!("paroles.json réécrit ({} lignes, source « {} ») en {:.1} s", ly.lines.len(), ly.source, t0.elapsed().as_secs_f64());
        return Ok(());
    }

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
    let text: Vec<align::TextLine> = lines.iter().map(|(t, s)| align::TextLine { start: Some(*t), text: s.clone() }).collect();
    let _ = align::align(&text, &rec, duration);
    let st = align::last_stats();
    println!(
        "alignement : {} mots, {} appariés, écartés ensuite : {} en arrière, {} trop tard, {} trop tôt",
        st.words, st.matched, st.rejected_backwards, st.rejected_late, st.rejected_early
    );
    if std::env::args().any(|a| a == "--stats") {
        return Ok(());
    }

    // Alignement forcé MMS : comparaison avec les mots que Whisper a reconnus.
    if std::env::args().any(|a| a == "--mms") {
        let mut prog = |m: &str, p: Option<f32>| {
            if p.is_none() || p.is_some_and(|p| (p * 100.0) as i32 % 25 == 0) {
                eprintln!("   {m} {}", p.map_or(String::new(), |p| format!("{:.0} %", p * 100.0)));
            }
        };
        let runtime = karaoke::setup::ensure_onnxruntime(&mut prog)?;
        let model = karaoke::setup::ensure_aligner(&mut prog)?;
        let t0 = std::time::Instant::now();
        let em = karaoke::forced::load_or_compute(&dir, &output, &runtime, &model, &mut |_| {})?;
        println!("émissions : {} trames ({:.1} s de calcul ou de lecture)", em.frames, t0.elapsed().as_secs_f64());
        let t1 = std::time::Instant::now();
        // Guide : méthode de secours (comme pipeline::build_lyrics), sauf avec --libre.
        let ons = karaoke::vocal::load_or_compute(&dir, &output);
        let guide = align::align_with(&text, &rec, duration, &ons);
        let free = std::env::args().any(|a| a == "--libre");
        let lines = karaoke::forced::align_lines(&text, &em, (!free).then_some(&guide[..])).expect("alignement forcé");
        println!("alignement forcé : {} lignes en {:.2} s", lines.len(), t1.elapsed().as_secs_f64());
        if std::env::args().any(|a| a == "--lignes") {
            // Par vers : heure LRC, début MMS, début méthode de secours, durée MMS, nombre de mots.
            let onsets = karaoke::vocal::load_or_compute(&dir, &output);
            let fb = align::align_with(&text, &rec, duration, &onsets);
            println!("vers  LRC     MMS     secours  MMS-LRC  durée  mots");
            for (i, ((t, f), b)) in text.iter().zip(&lines).zip(&fb).enumerate() {
                println!(
                    "{i:>3} {:>7.2} {:>7.2} {:>7.2} {:>+8.2} {:>6.2} {:>4}",
                    t.start.unwrap_or(f64::NAN),
                    f.start(),
                    b.start(),
                    f.start() - t.start.unwrap_or(f64::NAN),
                    f.words.last().map_or(0.0, |w| w.end) - f.start(),
                    f.words.len()
                );
            }
        }
        // Référence : mots dont l'heure vient directement de Whisper (méthode actuelle).
        let base = align::align(&text, &rec, duration);
        let mut diffs = Vec::new();
        for (bl, fl) in base.iter().zip(&lines) {
            for (bw, fw) in bl.words.iter().zip(&fl.words) {
                if rec.iter().any(|r| (r.start - bw.start).abs() < 1e-9) {
                    diffs.push(fw.start - bw.start);
                }
            }
        }
        let mut abs: Vec<f64> = diffs.iter().map(|d| d.abs()).collect();
        abs.sort_by(f64::total_cmp);
        let mut signed = diffs.clone();
        signed.sort_by(f64::total_cmp);
        let within = |x: f64| abs.iter().filter(|e| **e <= x).count() as f64 * 100.0 / abs.len().max(1) as f64;
        println!(
            "écart MMS − Whisper sur {} mots reconnus : médian {:.2} s (signé {:+.2} s) · ≤ 0,2 s : {:.0} % · ≤ 0,5 s : {:.0} % · > 1 s : {:.0} %",
            abs.len(),
            abs[abs.len() / 2],
            signed[signed.len() / 2],
            within(0.2),
            within(0.5),
            100.0 - within(1.0)
        );

        // Arbitre : attaque de la voix la plus proche (à ±0,5 s) de chaque mot reconnu.
        let onsets = karaoke::vocal::load_or_compute(&dir, &output);
        let near = |t: f64| onsets.iter().map(|o| o - t).filter(|d| d.abs() <= 0.5).min_by(|a, b| a.abs().total_cmp(&b.abs()));
        let (mut dw, mut dm, mut sw, mut sm) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut per_chunk: std::collections::BTreeMap<usize, Vec<f64>> = Default::default();
        for (bl, fl) in base.iter().zip(&lines) {
            for (bw, fw) in bl.words.iter().zip(&fl.words) {
                if rec.iter().any(|r| (r.start - bw.start).abs() < 1e-9) {
                    if let Some(d) = near(bw.start) { dw.push(d.abs()); sw.push(-d); }
                    if let Some(d) = near(fw.start) { dm.push(d.abs()); sm.push(-d); }
                    per_chunk.entry((bw.start / 15.0) as usize).or_default().push(fw.start - bw.start);
                }
            }
        }
        let med = |v: &mut Vec<f64>| { v.sort_by(f64::total_cmp); v.get(v.len() / 2).copied().unwrap_or(f64::NAN) };
        println!(
            "distance à l'attaque de voix la plus proche : Whisper médiane {:.3} s ({} mots) · MMS médiane {:.3} s ({} mots)",
            med(&mut dw), dw.len(), med(&mut dm), dm.len()
        );
        println!("mot − attaque la plus proche (signé) : Whisper médiane {:+.3} s · MMS médiane {:+.3} s", med(&mut sw), med(&mut sm));
        let chunks: Vec<String> = per_chunk.iter_mut().map(|(k, v)| format!("{}:{:+.2}", k, med(v))).collect();
        println!("écart signé MMS − Whisper par fenêtre de 15 s : {}", chunks.join("  "));
        return Ok(());
    }

    // Banc d'essai : mots reconnus masqués (1 sur 3) puis replacés ; écart avec leur
    // heure réelle, selon la méthode de placement.
    if std::env::args().any(|a| a == "--banc") {
        let onsets = karaoke::vocal::onsets_from_files(
            &karaoke::download::find_source(&dir).expect("source"),
            &output,
        )?;
        println!("attaques de la voix détectées : {} ({:.1} par seconde)", onsets.len(), onsets.len() as f64 / duration);
        let base = align::align_with(&text, &rec, duration, &onsets);
        // Mots dont l'heure vient directement d'un mot reconnu : (ligne, mot, heure, indice reconnu)
        let mut truth = Vec::new();
        for (li, l) in base.iter().enumerate() {
            for (wi, w) in l.words.iter().enumerate() {
                if let Some(ri) = rec.iter().position(|r| (r.start - w.start).abs() < 1e-9) {
                    truth.push((li, wi, w.start, ri));
                }
            }
        }
        // Scénarios réalistes : suites de mots non reconnus.
        let scenario = std::env::args().skip_while(|a| a != "--banc").nth(1).unwrap_or_default();
        let masked: Vec<_> = match scenario.as_str() {
            // les 3 premiers mots de chaque vers
            "debut" => truth.iter().filter(|t| t.1 < 3).copied().collect(),
            // des suites de 4 mots consécutifs (une suite sur deux)
            _ => truth.iter().enumerate().filter(|(k, _)| (k / 4) % 2 == 1).map(|(_, t)| *t).collect(),
        };
        let drop: std::collections::HashSet<usize> = masked.iter().map(|t| t.3).collect();
        let rec_masked: Vec<_> = rec.iter().enumerate().filter(|(i, _)| !drop.contains(i)).map(|(_, w)| w.clone()).collect();
        println!("scénario « {} » — mots de référence : {}, masqués : {}", if scenario.is_empty() { "suites" } else { &scenario }, truth.len(), masked.len());
        let mut eval = |name: &str, legacy: bool, ons: &[f64]| {
            align::set_legacy_placement(legacy);
            let lines = align::align_with(&text, &rec_masked, duration, ons);
            align::set_legacy_placement(false);
            let mut err: Vec<f64> = masked
                .iter()
                .filter_map(|&(li, wi, t, _)| lines.get(li).and_then(|l| l.words.get(wi)).map(|w| (w.start - t).abs()))
                .collect();
            err.sort_by(f64::total_cmp);
            let mean = err.iter().sum::<f64>() / err.len().max(1) as f64;
            let within = |x: f64| err.iter().filter(|e| **e <= x).count() as f64 * 100.0 / err.len().max(1) as f64;
            println!(
                "{name:<34} écart médian {:.2} s · moyen {:.2} s · ≤ 0,2 s : {:.0} % · ≤ 0,5 s : {:.0} %",
                err.get(err.len() / 2).copied().unwrap_or(0.0),
                mean,
                within(0.2),
                within(0.5)
            );
        };
        eval("ancien placement (tassé)", true, &[]);
        eval("répartition dans l'intervalle", false, &[]);
        eval("attaques de la voix", false, &onsets);
        if std::env::args().any(|a| a == "--seuils") {
            let vocals = karaoke::vocal::vocals_from_files(&karaoke::download::find_source(&dir).expect("source"), &output)?;
            for db in [3.0, 4.0, 5.0, 6.0, 8.0] {
                let o = karaoke::vocal::onsets_with(&vocals, media::SAMPLE_RATE, db);
                eval(&format!("attaques, seuil {db} dB ({}/s)", (o.len() as f64 / duration * 10.0).round() / 10.0), false, &o);
            }
        }
        return Ok(());
    }
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
