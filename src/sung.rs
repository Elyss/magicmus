//! Réparation des paroles par la transcription : après l'alignement des paroles
//! officielles (dans leur ordre, vers absents sautés), on cherche ce que Whisper a
//! clairement entendu alors qu'aucune parole n'est affichée à ce moment — refrain chanté
//! plus souvent que dans le texte officiel, phrase ajoutée par l'interprète d'une
//! reprise ou d'une version live. Chaque passage est rendu par le vers officiel qui lui
//! ressemble (répétition), sinon par le texte entendu (ajout), puis l'ensemble est
//! réaligné (`forced::align_verses`).
//!
//! Une première version appariait toute la transcription aux paroles avec sauts et
//! retours libres : sur un refrain présent 24 fois dans le texte, elle choisissait des
//! copies arbitraires et perdait des vers chantés. La réparation ne touche qu'aux
//! passages entendus et non couverts.

use crate::align::{normalize, TextLine};
use crate::forced::Window;
use crate::lyrics::{Line, Word};

/// Un mot entendu est « couvert » si le même mot est affiché à moins de cette durée.
const COVER_S: f64 = 1.5;
/// Mots entendus consécutifs (non couverts) à partir desquels on répare.
const MIN_RUN: usize = 3;
/// Écart maximal entre deux mots d'un même passage.
const RUN_GAP_S: f64 = 2.0;
/// Marge de la fenêtre de recherche autour des vers déjà placés et des passages.
const MARGIN_S: f64 = 1.0;
/// Part minimale des mots d'un vers officiel retrouvés dans un passage pour le reprendre.
const MIN_LINE_MATCH: f64 = 0.6;

/// Résultat de la réparation : texte et fenêtres à réaligner, nombre de vers ajoutés
/// (répétitions de vers officiels, ajouts entendus).
#[derive(Debug)]
pub struct Repair {
    pub text: Vec<TextLine>,
    pub windows: Vec<Window>,
    pub repeats: usize,
    pub extras: usize,
}

/// Passages entendus non couverts par `shown` (paroles placées), réparés. `None` s'il
/// n'y a rien à ajouter.
pub fn repair(shown: &[Line], official: &[TextLine], rec: &[Word]) -> Option<Repair> {
    let shown_words: Vec<(String, f64)> =
        shown.iter().flat_map(|l| l.words.iter()).map(|w| (normalize(&w.text), w.start)).collect();
    // Plages occupées par les vers placés.
    let occupied: Vec<(f64, f64)> = shown
        .iter()
        .filter_map(|l| Some((l.words.first()?.start - 0.5, l.words.last()?.end + 0.5)))
        .collect();
    let covered = |w: &Word| {
        let n = normalize(&w.text);
        n.chars().count() < 2 || shown_words.iter().any(|(s, t)| *s == n && (t - w.start).abs() <= COVER_S)
    };
    // Passages : mots entendus consécutifs non couverts, proches dans le temps.
    let mut runs: Vec<Vec<usize>> = Vec::new();
    for (i, w) in rec.iter().enumerate() {
        if covered(w) {
            continue;
        }
        match runs.last_mut() {
            Some(r) if *r.last().unwrap() + 1 == i && w.start - rec[i - 1].end < RUN_GAP_S => r.push(i),
            _ => runs.push(vec![i]),
        }
    }
    let free = |a: f64, b: f64| {
        let overlap: f64 = occupied.iter().map(|&(x, y)| (b.min(y) - a.max(x)).max(0.0)).sum();
        overlap <= 0.3 * (b - a).max(0.1)
    };
    let runs: Vec<Vec<usize>> = runs
        .into_iter()
        .filter(|r| r.len() >= MIN_RUN && free(rec[r[0]].start, rec[*r.last().unwrap()].end))
        .collect();
    if runs.is_empty() {
        return None;
    }

    // Vers officiels (mots normalisés) pour reconnaître les répétitions.
    let src: Vec<&TextLine> = official.iter().filter(|t| !t.text.trim().is_empty()).collect();
    let vocab: Vec<Vec<String>> = src.iter().map(|t| t.text.split_whitespace().map(normalize).collect()).collect();

    // Vers ajoutés : (début, fin, texte, répétition ?).
    let mut added: Vec<(f64, f64, TextLine, bool)> = Vec::new();
    let flush = |unknown: &mut Vec<usize>, added: &mut Vec<(f64, f64, TextLine, bool)>| {
        if unknown.len() >= MIN_RUN {
            let text = unknown.iter().map(|&i| rec[i].text.trim()).collect::<Vec<_>>().join(" ");
            let (a, b) = (rec[unknown[0]].start, rec[*unknown.last().unwrap()].end);
            added.push((a, b, TextLine { start: Some(a), text }, false));
        }
        unknown.clear();
    };
    for run in &runs {
        let words: Vec<String> = run.iter().map(|&i| normalize(&rec[i].text)).collect();
        let mut s = 0;
        let mut unknown: Vec<usize> = Vec::new();
        while s < run.len() {
            // Vers officiel qui explique le mieux les mots à partir de s : mots
            // consécutifs (un oubli toléré) présents dans le vers.
            let mut best: Option<(usize, usize, f64)> = None; // (vers, mots consommés, score)
            for (k, v) in vocab.iter().enumerate() {
                let (mut used, mut hits, mut miss) = (0, 0, 0);
                for w in &words[s..] {
                    if v.contains(w) {
                        hits += 1;
                        used += 1 + miss;
                        miss = 0;
                    } else {
                        miss += 1;
                        if miss > 1 {
                            break;
                        }
                    }
                    if used >= v.len() + 1 {
                        break;
                    }
                }
                let score = hits as f64 / v.len().max(1) as f64;
                if hits >= 2 && score >= MIN_LINE_MATCH && best.is_none_or(|b| score > b.2 || (score == b.2 && used > b.1)) {
                    best = Some((k, used, score));
                }
            }
            match best {
                Some((k, used, _)) => {
                    flush(&mut unknown, &mut added);
                    let idx = &run[s..s + used];
                    let (a, b) = (rec[idx[0]].start, rec[*idx.last().unwrap()].end);
                    added.push((a, b, (*src[k]).clone(), true));
                    s += used;
                }
                None => {
                    unknown.push(run[s]);
                    s += 1;
                }
            }
        }
        flush(&mut unknown, &mut added);
    }
    if added.is_empty() {
        return None;
    }

    // Vers déjà placés (fenêtre serrée autour de leur place) + vers ajoutés, dans
    // l'ordre du temps.
    let mut all: Vec<(f64, TextLine, Window)> = shown
        .iter()
        .filter_map(|l| {
            let (a, b) = (l.words.first()?.start, l.words.last()?.end);
            Some((a, TextLine { start: Some(a), text: l.text() }, Window { lo: a - MARGIN_S, hi: b + MARGIN_S, heard: true, sung: true }))
        })
        .collect();
    let (mut repeats, mut extras) = (0, 0);
    for (a, b, t, rep) in added {
        if rep {
            repeats += 1;
        } else {
            extras += 1;
        }
        all.push((a, t, Window { lo: a - MARGIN_S, hi: b + MARGIN_S, heard: true, sung: true }));
    }
    all.sort_by(|x, y| x.0.total_cmp(&y.0));
    Some(Repair {
        text: all.iter().map(|x| x.1.clone()).collect(),
        windows: all.iter().map(|x| x.2).collect(),
        repeats,
        extras,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(t: &str, s: f64) -> Word {
        Word { text: t.into(), start: s, end: s + 0.3 }
    }

    fn line(words: &[(&str, f64)]) -> Line {
        Line { words: words.iter().map(|&(t, s)| w(t, s)).collect() }
    }

    /// Refrain chanté une fois de plus que dans le texte officiel, plus un ajout.
    #[test]
    fn repetition_et_ajout_entendus() {
        let official: Vec<TextLine> = ["un deux trois quatre", "cinq six sept huit", "neuf dix onze douze"]
            .iter()
            .map(|t| TextLine { start: None, text: t.to_string() })
            .collect();
        // Placés : vers 0 à 0 s, vers 1 à 4 s, vers 2 à 20 s.
        let shown = vec![
            line(&[("un", 0.0), ("deux", 0.5), ("trois", 1.0), ("quatre", 1.5)]),
            line(&[("cinq", 4.0), ("six", 4.5), ("sept", 5.0), ("huit", 5.5)]),
            line(&[("neuf", 20.0), ("dix", 20.5), ("onze", 21.0), ("douze", 21.5)]),
        ];
        // Entendus : le vers 1 de nouveau à 9 s (un mot manqué), puis un ajout à 14 s.
        let rec = vec![
            w("un", 0.0), w("deux", 0.5), w("trois", 1.0), w("quatre", 1.5),
            w("cinq", 4.0), w("six", 4.5), w("sept", 5.0), w("huit", 5.5),
            w("cinq", 9.0), w("sept", 10.0), w("huit", 10.5),
            w("allez", 14.0), w("tout", 14.4), w("le", 14.8), w("monde", 15.2),
            w("neuf", 20.0), w("dix", 20.5), w("onze", 21.0), w("douze", 21.5),
        ];
        let r = repair(&shown, &official, &rec).unwrap();
        let texts: Vec<&str> = r.text.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(
            texts,
            ["un deux trois quatre", "cinq six sept huit", "cinq six sept huit", "allez tout le monde", "neuf dix onze douze"]
        );
        assert_eq!((r.repeats, r.extras), (1, 1));
        assert!((r.windows[2].lo - 8.0).abs() < 1e-6);
    }

    /// Tout est déjà couvert : rien à réparer.
    #[test]
    fn rien_a_reparer() {
        let official = vec![TextLine { start: None, text: "un deux trois".into() }];
        let shown = vec![line(&[("un", 0.0), ("deux", 0.5), ("trois", 1.0)])];
        let rec = vec![w("un", 0.1), w("deux", 0.6), w("trois", 1.2)];
        assert!(repair(&shown, &official, &rec).is_none());
    }
}
