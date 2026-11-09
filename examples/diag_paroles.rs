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
    let verses = karaoke::forced::align_verses(&text_lines, &em, None, skip_setting()).expect("alignement");
    let skipped = verses.iter().filter(|v| v.is_none()).count();
    let lines: Vec<karaoke::lyrics::Line> = verses.into_iter().flatten().collect();
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
        "{} : {} mots comparés, {skipped} vers sautés · écart médian {:.3} s (signé {:+.3} s) · ≤ 0,1 s : {:.0} % · ≤ 0,2 s : {:.0} % · max {:.2} s",
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

/// Pénalité de saut de vers : `MAGICMUS_SKIP=<valeur>` ou `off` (défaut : celle de l'application).
fn skip_setting() -> Option<f32> {
    match std::env::var("MAGICMUS_SKIP") {
        Ok(v) if v == "off" => None,
        Ok(v) => v.parse().ok(),
        Err(_) => Some(karaoke::forced::SKIP_PER_LETTER),
    }
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
                karaoke::pipeline::Event::Log(w) => println!("journal : {w}"),
                karaoke::pipeline::Event::Stage(w) => println!("étape : {w}"),
                _ => {}
            }
        })
        .expect("paroles");
        ly.offset = old.map_or(0.0, |l| l.offset);
        if std::env::args().any(|a| a == "--essai") {
            println!("essai : paroles.json non modifié ({} lignes, source « {} »)", ly.lines.len(), ly.source);
            return Ok(());
        }
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
    let heard: Vec<String> = rec.iter().map(|w| w.text.clone()).collect();
    let Some(m) = lrclib::find(&title, None, None, duration, &heard)? else {
        println!("aucune paroles en ligne");
        return Ok(());
    };
    let lrclib::Found::Synced(lines) = m.lyrics else {
        println!("paroles en ligne non synchronisées");
        return Ok(());
    };
    println!("LRC : {} vers, dernier à {:.1} s", lines.len(), lines.last().map_or(0.0, |l| l.0));
    let text: Vec<align::TextLine> = lines.iter().map(|(t, s)| align::TextLine { start: Some(*t), text: s.clone() }).collect();

    // Structure chantée : pour chaque groupe de mots entendus par Whisper, vers LRC qui
    // lui ressemble le plus (numéros seulement, jamais le texte).
    if std::env::args().any(|a| a == "--structure") {
        let verses: Vec<Vec<String>> = text
            .iter()
            .filter(|t| !t.text.trim().is_empty())
            .map(|t| t.text.split_whitespace().map(align::normalize).collect())
            .collect();
        let mut groups: Vec<Vec<&lyrics::Word>> = vec![];
        for w in &rec {
            match groups.last_mut() {
                Some(g) if w.start - g.last().unwrap().end < 0.8 && g.len() < 8 => g.push(w),
                _ => groups.push(vec![w]),
            }
        }
        println!("début  mots  vers (score)  2e");
        for g in groups.iter().filter(|g| g.len() >= 2) {
            let ws: Vec<String> = g.iter().map(|w| align::normalize(&w.text)).collect();
            let mut scores: Vec<(usize, f64)> = verses
                .iter()
                .enumerate()
                .map(|(i, v)| (i, ws.iter().filter(|w| v.contains(w)).count() as f64 / ws.len() as f64))
                .collect();
            scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            let ties: Vec<String> = scores.iter().take_while(|s| s.1 == scores[0].1).map(|s| s.0.to_string()).collect();
            println!("{:>6.1} {:>4}  {:>10} ({:.2})  {}", g[0].start, g.len(), ties.join(","), scores[0].1, scores.get(ties.len()).map_or(String::new(), |s| format!("{} ({:.2})", s.0, s.1)));
        }
        return Ok(());
    }
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
        let free = std::env::args().any(|a| a == "--libre");
        let guide = karaoke::forced::guide_windows(&text, &rec).filter(|_| !free);
        let mut sung_text: Vec<align::TextLine> = text.clone();
        let mut origin: Vec<Option<usize>> = (0..text.iter().filter(|t| !t.text.trim().is_empty()).count()).map(Some).collect();
        let mut lines = karaoke::forced::align_verses(&sung_text, &em, guide.as_deref(), skip_setting())
            .expect("alignement forcé");
        let mut guide = guide;
        // Réparation par la transcription (sauf --ancien), comme pipeline::build_lyrics.
        if !std::env::args().any(|a| a == "--ancien") {
            let shown: Vec<karaoke::lyrics::Line> = lines.iter().flatten().cloned().collect();
            if let Some(rp) = karaoke::sung::repair(&shown, &text, &rec) {
                println!("réparation : {} vers répétés, {} ajouts entendus", rp.repeats, rp.extras);
                lines = karaoke::forced::align_verses(&rp.text, &em, Some(&rp.windows), skip_setting()).expect("réalignement");
                origin = vec![None; rp.text.len()];
                sung_text = rp.text;
                guide = Some(rp.windows);
            }
        }
        println!(
            "alignement forcé : {} vers, {} sautés, en {:.2} s",
            lines.len(),
            lines.iter().filter(|l| l.is_none()).count(),
            t1.elapsed().as_secs_f64()
        );
        let text_ne: Vec<&align::TextLine> = text.iter().filter(|t| !t.text.trim().is_empty()).collect();
        if std::env::args().any(|a| a == "--lignes") {
            // Par vers : heure LRC, début MMS, début méthode de secours, durée MMS, nombre de mots.
            let onsets = karaoke::vocal::load_or_compute(&dir, &output);
            let fb = align::align_with(&text, &rec, duration, &onsets);
            let heard = |i: usize| {
                guide.as_ref().and_then(|g| g.get(i)).map_or(String::new(), |w| {
                    format!("{} [{:.0}–{:.0}]", if w.heard { "entendu" } else if w.sung { "sûr" } else { "·" }, w.lo, w.hi.min(9999.0))
                })
            };
            println!("vers  orig  LRC     MMS     secours  MMS-LRC  durée  mots");
            for (i, f) in lines.iter().enumerate() {
                let (o, lrc, sec) = match origin[i] {
                    Some(o) => (o.to_string(), text_ne[o].start.unwrap_or(f64::NAN), fb.get(o).map_or(f64::NAN, |b| b.start())),
                    None => ("ajout".to_string(), f64::NAN, f64::NAN),
                };
                let Some(f) = f else {
                    println!("{i:>3} {o:>5} {lrc:>7.2}       —  {sec:>7.2}         (sauté)        {:>4} {}", sung_text[i].text.split_whitespace().count(), heard(i));
                    continue;
                };
                println!(
                    "{i:>3} {o:>5} {lrc:>7.2} {:>7.2} {sec:>7.2} {:>+8.2} {:>6.2} {:>4} {}",
                    f.start(),
                    f.start() - lrc,
                    f.words.last().map_or(0.0, |w| w.end) - f.start(),
                    f.words.len(),
                    heard(i)
                );
            }
        }
        // Mots entendus par Whisper retrouvés dans les paroles affichées (même mot à
        // ±1 s) : mesure du décrochage, indépendante de la structure du texte.
        let shown: Vec<&karaoke::lyrics::Word> = lines.iter().flatten().flat_map(|l| l.words.iter()).collect();
        let mut diffs = Vec::new();
        let mut counted = 0;
        for r in rec.iter().filter(|r| align::normalize(&r.text).chars().count() >= 2) {
            counted += 1;
            let n = align::normalize(&r.text);
            if let Some(d) = shown
                .iter()
                .filter(|w| align::normalize(&w.text) == n)
                .map(|w| w.start - r.start)
                .filter(|d| d.abs() <= 1.0)
                .min_by(|a, b| a.abs().total_cmp(&b.abs()))
            {
                diffs.push(d);
            }
        }
        let mut abs: Vec<f64> = diffs.iter().map(|d| d.abs()).collect();
        abs.sort_by(f64::total_cmp);
        let within = |x: f64| abs.iter().filter(|e| **e <= x).count() as f64 * 100.0 / abs.len().max(1) as f64;
        println!(
            "mots entendus retrouvés à ±1 s dans les paroles affichées : {}/{} ({:.0} %) · écart médian {:.2} s · ≤ 0,3 s : {:.0} %",
            diffs.len(),
            counted,
            diffs.len() as f64 * 100.0 / counted.max(1) as f64,
            abs.get(abs.len() / 2).copied().unwrap_or(f64::NAN),
            within(0.3)
        );
        // Mots entendus non retrouvés : affichés plus loin (décalage) ou jamais à ±5 s (texte).
        let (mut late, mut absent, mut late_t) = (0, 0, Vec::new());
        for r in rec.iter().filter(|r| align::normalize(&r.text).chars().count() >= 2) {
            let n = align::normalize(&r.text);
            let best = shown.iter().filter(|w| align::normalize(&w.text) == n).map(|w| w.start - r.start)
                .min_by(|a, b| a.abs().total_cmp(&b.abs()));
            match best {
                Some(d) if d.abs() <= 1.0 => {}
                Some(d) if d.abs() <= 5.0 => { late += 1; late_t.push(format!("{:.0}s:{:+.1}", r.start, d)); }
                _ => absent += 1,
            }
        }
        println!("non retrouvés : {late} affichés entre 1 et 5 s d'écart [{}] · {absent} absents à ±5 s", late_t.join(" "));
        // Arbitre : début des mots affichés − attaque de la voix la plus proche (±0,5 s).
        let onsets = karaoke::vocal::load_or_compute(&dir, &output);
        let mut sm: Vec<f64> = shown
            .iter()
            .filter_map(|w| onsets.iter().map(|o| w.start - o).filter(|d| d.abs() <= 0.5).min_by(|a, b| a.abs().total_cmp(&b.abs())))
            .collect();
        sm.sort_by(f64::total_cmp);
        println!("mot affiché − attaque la plus proche : médiane {:+.3} s ({} mots sur {})", sm.get(sm.len() / 2).copied().unwrap_or(f64::NAN), sm.len(), shown.len());
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
        let eval = |name: &str, legacy: bool, ons: &[f64]| {
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
