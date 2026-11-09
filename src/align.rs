//! Alignement des paroles exactes (base en ligne ou texte collé) sur les mots
//! horodatés reconnus par Whisper.
//!
//! Les deux suites de mots sont alignées par programmation dynamique (Needleman-Wunsch)
//! avec une similarité tolérante (accents, fautes) : chaque mot exact apparié prend
//! l'horodatage du mot entendu ; les autres sont répartis entre leurs voisins.

use crate::lyrics::{Line, Word};

/// Paroles exactes en entrée : vers, avec éventuellement l'heure de début (LRC).
#[derive(Clone, Debug)]
pub struct TextLine {
    pub start: Option<f64>,
    pub text: String,
}

/// Construit les lignes horodatées mot à mot.
/// `recognized` : mots reconnus (peut être vide) ; `end_of_song` : durée du morceau.
pub fn align(text: &[TextLine], recognized: &[Word], end_of_song: f64) -> Vec<Line> {
    // Les heures LRC sont d'abord recalées sur la voix réellement entendue.
    let recalibrated;
    let text = if !recognized.is_empty() && text.iter().any(|l| l.start.is_some()) {
        recalibrated = recalibrate(text, recognized);
        &recalibrated[..]
    } else {
        text
    };
    let mut lines = align_inner(text, recognized, end_of_song);
    enforce_order(&mut lines);
    lines
}

fn align_inner(text: &[TextLine], recognized: &[Word], end_of_song: f64) -> Vec<Line> {
    // Vers non vides ; un vers vide horodaté marque la fin du précédent (pause).
    struct V {
        start: Option<f64>,
        limit: Option<f64>,
        words: Vec<String>,
    }
    let mut verses: Vec<V> = Vec::new();
    for (i, l) in text.iter().enumerate() {
        let words: Vec<String> = l.text.split_whitespace().map(String::from).collect();
        if words.is_empty() {
            continue;
        }
        let limit = text[i + 1..].iter().find_map(|n| n.start);
        verses.push(V { start: l.start, limit, words });
    }
    if verses.is_empty() {
        return Vec::new();
    }

    // Suite à plat des mots exacts, avec la fenêtre temporelle autorisée (vers LRC).
    let mut flat: Vec<(usize, String, Option<(f64, f64)>)> = Vec::new();
    for (vi, v) in verses.iter().enumerate() {
        let window = v.start.map(|s| (s - 1.5, v.limit.unwrap_or(end_of_song) + 1.5));
        for w in &v.words {
            flat.push((vi, w.clone(), window));
        }
    }
    let rec_norm: Vec<String> = recognized.iter().map(|w| normalize(&w.text)).collect();
    let pairs = needleman_wunsch(&flat, recognized, &rec_norm);

    // Horodatages connus (mots appariés).
    let mut times: Vec<Option<(f64, f64)>> = vec![None; flat.len()];
    for (i, j) in pairs {
        times[i] = Some((recognized[j].start, recognized[j].end));
    }

    // Construction vers par vers, en complétant les mots non appariés.
    let mut lines = Vec::new();
    let mut idx = 0;
    let mut prev_end = 0.0f64;
    for (vi, v) in verses.iter().enumerate() {
        let n = v.words.len();
        let known: Vec<Option<(f64, f64)>> = times[idx..idx + n].to_vec();
        idx += n;

        // Bornes du vers : début LRC sinon fin du vers précédent ; fin : vers suivant.
        let next_start = verses[vi + 1..]
            .iter()
            .zip(vi + 1..)
            .find_map(|(nv, _)| nv.start)
            .or_else(|| {
                // premier mot apparié des vers suivants
                let off = idx;
                times[off..].iter().flatten().next().map(|t| t.0)
            })
            .unwrap_or(end_of_song);
        let lo = v.start.unwrap_or(prev_end).max(prev_end);
        let hi = v.limit.unwrap_or(next_start).min(next_start).max(lo + 0.3);

        let words = fill(&v.words, &known, lo, hi, prev_end, v.start.is_some());
        prev_end = words.last().map_or(prev_end, |w| w.end);
        lines.push(Line { words });
    }
    lines
}

/// Recale les heures des vers (LRC) sur la voix entendue.
///
/// Les paroles en ligne correspondent souvent à une autre version du morceau (album
/// plutôt que clip) : décalage global, parfois variable. On aligne d'abord le texte
/// seul (sans contrainte de temps), on mesure pour chaque vers l'écart entre l'heure
/// entendue de son premier mot apparié et l'heure LRC, puis on lisse ces écarts.
fn recalibrate(text: &[TextLine], rec: &[Word]) -> Vec<TextLine> {
    let mut flat: Vec<(usize, String, Option<(f64, f64)>)> = Vec::new();
    let mut pos_in_line = Vec::new();
    for (li, l) in text.iter().enumerate() {
        for (k, w) in l.text.split_whitespace().enumerate() {
            flat.push((li, w.to_string(), None));
            pos_in_line.push(k);
        }
    }
    let rec_norm: Vec<String> = rec.iter().map(|w| normalize(&w.text)).collect();
    let pairs = needleman_wunsch(&flat, rec, &rec_norm);

    // Écart brut par vers, d'après son premier mot apparié.
    let mut raw: Vec<(usize, f64)> = Vec::new();
    for (i, j) in pairs {
        let li = flat[i].0;
        if raw.last().is_some_and(|r| r.0 == li) {
            continue;
        }
        if let Some(s) = text[li].start {
            let d = rec[j].start - s - pos_in_line[i] as f64 * 0.3;
            if d.abs() < 20.0 {
                raw.push((li, d));
            }
        }
    }
    if raw.is_empty() {
        return text.to_vec();
    }
    let global = median(raw.iter().map(|r| r.1).collect());
    // Les écarts aberrants (mot répété ailleurs dans la chanson) sont écartés.
    let good: Vec<(usize, f64)> = raw.into_iter().filter(|(_, d)| (d - global).abs() <= 3.0).collect();

    text.iter()
        .enumerate()
        .map(|(li, l)| {
            // Écart local : médiane des 5 vers mesurés les plus proches.
            let mut near: Vec<(usize, f64)> = good.iter().map(|&(gi, d)| (gi.abs_diff(li), d)).collect();
            near.sort_by_key(|x| x.0);
            let off = if near.is_empty() { global } else { median(near.iter().take(5).map(|x| x.1).collect()) };
            TextLine { start: l.start.map(|s| (s + off).max(0.0)), text: l.text.clone() }
        })
        .collect()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Garantit que les mots se suivent : chaque mot commence après le précédent et se
/// termine au plus tard au début du suivant (sinon plusieurs mots « s'allument » en
/// même temps).
fn enforce_order(lines: &mut [Line]) {
    const MIN: f64 = 0.08;
    let mut words: Vec<&mut Word> = lines.iter_mut().flat_map(|l| l.words.iter_mut()).collect();
    for k in 1..words.len() {
        let prev = words[k - 1].start;
        if words[k].start < prev + MIN {
            words[k].start = prev + MIN;
        }
    }
    for k in 0..words.len() {
        let next = words.get(k + 1).map(|w| w.start);
        let w = &mut words[k];
        let mut end = w.end.max(w.start + MIN);
        if let Some(n) = next {
            end = end.min(n);
        }
        w.end = end;
    }
}

/// Complète les horodatages manquants d'un vers dans [lo, hi].
/// `anchored_start` : le début du vers est fiable (LRC).
/// `floor` : fin du vers précédent (aucun mot ne peut commencer avant).
fn fill(
    words: &[String],
    known: &[Option<(f64, f64)>],
    lo: f64,
    hi: f64,
    floor: f64,
    anchored_start: bool,
) -> Vec<Word> {
    let n = words.len();
    let weight: Vec<f64> = words.iter().map(|w| (w.chars().filter(|c| c.is_alphanumeric()).count() as f64).max(1.0)).collect();
    let est = |a: usize, b: usize| -> f64 { weight[a..b].iter().map(|w| (w * 0.075).clamp(0.22, 0.9)).sum() };

    // Corrige les horodatages connus incohérents (hors du vers ou non croissants).
    let mut t: Vec<Option<(f64, f64)>> = known.to_vec();
    // Un mot entendu peut précéder l'heure estimée du vers (jusqu'à la largeur de la
    // fenêtre d'alignement), mais jamais la fin du vers précédent.
    let mut last = floor - 0.01;
    for x in t.iter_mut() {
        if let Some((s, e)) = *x {
            if s < last - 0.05 || s > hi + 0.5 || (anchored_start && s < lo - 1.6) {
                *x = None;
            } else {
                last = e.max(s);
            }
        }
    }
    if anchored_start && t.first().is_some_and(|x| x.is_none()) {
        // le premier mot commence au début LRC
        let e = lo + est(0, 1);
        t[0] = Some((lo, e.min(hi)));
    }

    let mut out: Vec<(f64, f64)> = vec![(0.0, 0.0); n];
    let mut i = 0;
    while i < n {
        if let Some(se) = t[i] {
            out[i] = se;
            i += 1;
            continue;
        }
        // série de mots inconnus [i, j)
        let mut j = i;
        while j < n && t[j].is_none() {
            j += 1;
        }
        let right = if j < n { t[j].unwrap().0 } else { hi };
        // Le mot précédent peut finir après le début du suivant (fin de mot reconnue
        // trop longue) : on repart alors juste après son début.
        let left = if i == 0 { lo } else { out[i - 1].1.min(right).max(out[i - 1].0 + 0.08) };
        let gap = (right - left).max(0.05);
        let need = est(i, j);
        // Début de vers sans repère : les mots mènent au mot suivant (alignés à droite) ;
        // sinon ils suivent le mot précédent (alignés à gauche).
        let (start, span) = if need >= gap {
            (left, gap)
        } else if i == 0 && j < n && !anchored_start {
            (right - need, need)
        } else {
            (left, need)
        };
        let total: f64 = weight[i..j].iter().sum();
        let mut cur = start;
        for k in i..j {
            let d = span * weight[k] / total;
            out[k] = (cur, cur + d * 0.92);
            cur += d;
        }
        i = j;
    }
    words
        .iter()
        .zip(out)
        .map(|(w, (s, e))| Word { text: w.clone(), start: s, end: e.max(s + 0.08) })
        .collect()
}

/// Alignement global ; renvoie les paires (indice exact, indice reconnu).
fn needleman_wunsch(
    flat: &[(usize, String, Option<(f64, f64)>)],
    rec: &[Word],
    rec_norm: &[String],
) -> Vec<(usize, usize)> {
    let (n, m) = (flat.len(), rec.len());
    if n == 0 || m == 0 {
        return Vec::new();
    }
    const GAP: f32 = -0.4;
    let norm: Vec<String> = flat.iter().map(|(_, w, _)| normalize(w)).collect();
    let score = |i: usize, j: usize| -> f32 {
        if let Some((a, b)) = flat[i].2 {
            if rec[j].start < a || rec[j].start > b {
                return -1.0;
            }
        }
        let s = similarity(&norm[i], &rec_norm[j]);
        if s >= 0.6 { 1.0 + s } else { -1.0 }
    };
    let w = m + 1;
    let mut dp = vec![0f32; (n + 1) * w];
    let mut dir = vec![0u8; (n + 1) * w]; // 0 diag, 1 haut, 2 gauche
    for i in 1..=n {
        dp[i * w] = GAP * i as f32;
        dir[i * w] = 1;
    }
    for j in 1..=m {
        dp[j] = GAP * j as f32;
        dir[j] = 2;
    }
    for i in 1..=n {
        for j in 1..=m {
            let d = dp[(i - 1) * w + j - 1] + score(i - 1, j - 1);
            let u = dp[(i - 1) * w + j] + GAP;
            let l = dp[i * w + j - 1] + GAP;
            let (best, k) = if d >= u && d >= l { (d, 0) } else if u >= l { (u, 1) } else { (l, 2) };
            dp[i * w + j] = best;
            dir[i * w + j] = k;
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        match dir[i * w + j] {
            0 => {
                if score(i - 1, j - 1) > 0.0 {
                    pairs.push((i - 1, j - 1));
                }
                i -= 1;
                j -= 1;
            }
            1 => i -= 1,
            _ => j -= 1,
        }
    }
    pairs.reverse();
    pairs
}

/// Minuscules, sans accents ni ponctuation.
pub fn normalize(w: &str) -> String {
    w.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| {
            Some(match c {
                'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => 'a',
                'é' | 'è' | 'ê' | 'ë' => 'e',
                'î' | 'ï' | 'í' | 'ì' => 'i',
                'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
                'ù' | 'û' | 'ü' | 'ú' => 'u',
                'ç' => 'c',
                'ñ' => 'n',
                'ÿ' => 'y',
                c if c.is_alphanumeric() => c,
                _ => return None,
            })
        })
        .collect()
}

/// Similarité de 0 à 1 (distance de Levenshtein normalisée).
fn similarity(a: &str, b: &str) -> f32 {
    if a.is_empty() || b.is_empty() {
        return if a == b { 1.0 } else { 0.0 };
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let c = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j - 1] + c).min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        prev = cur;
    }
    1.0 - prev[b.len()] as f32 / a.len().max(b.len()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(t: &str, s: f64) -> Word {
        Word { text: t.into(), start: s, end: s + 0.4 }
    }

    fn line(start: Option<f64>, text: &str) -> TextLine {
        TextLine { start, text: text.into() }
    }

    #[test]
    fn normalisation_similarite() {
        assert_eq!(normalize("Été,"), "ete");
        assert!(similarity("bonjour", "bonjours") > 0.8);
        assert!(similarity("chat", "maison") < 0.5);
    }

    #[test]
    fn mots_mal_reconnus_recales() {
        // Texte exact : « le soleil brille sur la mer » ; Whisper a entendu
        // « le sommeil brille sûr la » (un mot faux, un accent, un mot manquant).
        let rec = [w("le", 1.0), w("sommeil", 1.5), w("brille", 2.2), w("sûr", 2.9), w("la", 3.3)];
        let lines = align(&[line(None, "le soleil brille sur la mer")], &rec, 10.0);
        let l = &lines[0];
        assert_eq!(l.text(), "le soleil brille sur la mer");
        let starts: Vec<f64> = l.words.iter().map(|w| w.start).collect();
        assert_eq!(&starts[..5], &[1.0, 1.5, 2.2, 2.9, 3.3]); // « soleil » ≈ « sommeil »
        assert!(starts[5] >= 3.69 && starts[5] < 4.5, "« mer » placé à {}", starts[5]);
    }

    #[test]
    fn lrc_sans_reconnaissance() {
        // Vers horodatés (LRC), aucune reconnaissance : répartition dans chaque vers.
        let text = [line(Some(5.0), "un deux trois"), line(Some(9.0), "quatre cinq"), line(Some(15.0), "")];
        let lines = align(&text, &[], 60.0);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].words[0].start, 5.0);
        assert!(lines[0].end() <= 9.0);
        assert_eq!(lines[1].words[0].start, 9.0);
        assert!(lines[1].end() <= 15.0);
        for l in &lines {
            for p in l.words.windows(2) {
                assert!(p[0].start < p[1].start);
            }
        }
    }

    #[test]
    fn lrc_en_retard_recale_sur_la_voix() {
        // Les heures LRC sont 1,2 à 2 s en retard sur la voix entendue (autre version du
        // morceau) : les mots doivent suivre la voix, pas le LRC.
        let rec = [
            w("un", 3.8), w("deux", 4.3), w("trois", 4.8),
            w("quatre", 7.9), w("cinq", 8.4),
            w("six", 11.0), w("sept", 11.5), w("huit", 12.0),
        ];
        let text = [
            line(Some(5.0), "un deux trois"),
            line(Some(9.6), "quatre cinq"),
            line(Some(12.2), "six sept huit"),
        ];
        let lines = align(&text, &rec, 60.0);
        let starts: Vec<f64> = lines.iter().flat_map(|l| l.words.iter().map(|w| w.start)).collect();
        assert_eq!(starts, [3.8, 4.3, 4.8, 7.9, 8.4, 11.0, 11.5, 12.0]);
    }

    #[test]
    fn mots_dans_l_ordre_sans_chevauchement() {
        // Fins de mots reconnues trop longues et un mot reconnu en avance.
        let mut rec = vec![w("un", 1.0), w("deux", 1.4), w("quatre", 2.0)];
        rec[0].end = 3.0;
        rec[1].end = 2.6;
        let lines = align(&[line(None, "un deux trois quatre cinq")], &rec, 30.0);
        let ws = &lines[0].words;
        for p in ws.windows(2) {
            assert!(p[0].start < p[1].start, "ordre : {:?}", ws.iter().map(|w| w.start).collect::<Vec<_>>());
            assert!(p[0].end <= p[1].start + 1e-9, "chevauchement");
        }
    }

    #[test]
    fn lrc_avec_reconnaissance() {
        // Le mot reconnu hors de la fenêtre du vers n'est pas utilisé.
        let rec = [w("un", 5.2), w("deux", 5.8), w("trois", 6.5), w("quatre", 9.3), w("un", 40.0)];
        let text = [line(Some(5.0), "un deux trois"), line(Some(9.0), "quatre cinq")];
        let lines = align(&text, &rec, 60.0);
        let s0: Vec<f64> = lines[0].words.iter().map(|w| w.start).collect();
        assert_eq!(s0, [5.2, 5.8, 6.5]);
        assert_eq!(lines[1].words[0].start, 9.3);
        assert!(lines[1].words[1].start > 9.3 && lines[1].words[1].start < 11.0);
    }
}
