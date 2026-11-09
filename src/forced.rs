//! Alignement forcé des paroles (CTC) avec le modèle multilingue Meta MMS-300M.
//!
//! Contrairement à la reconnaissance vocale, on ne demande pas au modèle ce qu'il
//! entend : on lui donne le texte exact et on cherche, toutes les 20 ms, le chemin le
//! plus probable qui fait défiler toutes ses lettres dans l'ordre (algorithme de
//! Viterbi sur la sortie CTC). Chaque mot reçoit ainsi un début et une fin mesurés.

use crate::lyrics::{Line, Word};
use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;

/// Durée d'une trame de sortie du modèle (320 échantillons à 16 kHz).
pub const FRAME_S: f64 = 0.02;
const SR: usize = 16_000;
const HOP: usize = 320;
const BLANK: usize = 0;
/// Fenêtres d'analyse : 15 s, avec 1 s de contexte de chaque côté.
const WIN: usize = 15 * SR;
const CTX: usize = SR;

/// Probabilités (log) par trame et par symbole.
pub struct Emissions {
    pub frames: usize,
    pub vocab: usize,
    pub logp: Vec<f32>,
}

impl Emissions {
    fn at(&self, t: usize, k: usize) -> f32 {
        self.logp[t * self.vocab + k]
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let mut bytes = Vec::with_capacity(8 + self.logp.len() * 4);
        bytes.extend((self.frames as u32).to_le_bytes());
        bytes.extend((self.vocab as u32).to_le_bytes());
        for v in &self.logp {
            bytes.extend(v.to_le_bytes());
        }
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let b = std::fs::read(path)?;
        if b.len() < 8 {
            bail!("fichier d'émissions tronqué");
        }
        let frames = u32::from_le_bytes(b[0..4].try_into()?) as usize;
        let vocab = u32::from_le_bytes(b[4..8].try_into()?) as usize;
        let logp: Vec<f32> = b[8..].chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        if logp.len() != frames * vocab {
            bail!("fichier d'émissions incohérent");
        }
        Ok(Self { frames, vocab, logp })
    }
}

/// Fichier des émissions conservées dans le dossier de la chanson.
pub const EMISSIONS_FILE: &str = "emissions.bin";

/// Calcule les émissions du modèle sur la voix (`mono16k` : 16 kHz mono).
pub fn emissions(runtime: &Path, model: &Path, mono16k: &[f32], progress: &mut dyn FnMut(f32)) -> Result<Emissions> {
    let env = ort::init_from(runtime)
        .map_err(|e| anyhow!("{e}"))
        .with_context(|| format!("chargement de {}", runtime.display()))?
        .with_name("magicmus-alignement")
        .build()
        .map_err(|e| anyhow!("{e}"))?;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut builder = ort::session::Session::builder(&env)
        .map_err(|e| anyhow!("{e}"))?
        .with_intra_threads(threads)
        .map_err(|e| anyhow!("{e}"))?;
    let mut session: ort::session::Session = builder
        .commit_from_file(model)
        .map_err(|e| anyhow!("{e}"))
        .with_context(|| format!("chargement du modèle {}", model.display()))?;

    let total_frames = mono16k.len() / HOP;
    let mut logp: Vec<f32> = Vec::with_capacity(total_frames * 31);
    let mut vocab = 0;
    let chunks = mono16k.len().div_ceil(WIN).max(1);
    for c in 0..chunks {
        let start = c * WIN;
        let a = start.saturating_sub(CTX);
        let b = (start + WIN + CTX).min(mono16k.len());
        if b <= a + 400 {
            break;
        }
        // Normalisation (moyenne nulle, variance unité) de la fenêtre.
        let seg = &mono16k[a..b];
        let mean = seg.iter().map(|&x| x as f64).sum::<f64>() / seg.len() as f64;
        let var = seg.iter().map(|&x| (x as f64 - mean).powi(2)).sum::<f64>() / seg.len() as f64;
        let inv = 1.0 / (var + 1e-7).sqrt();
        let input: Vec<f32> = seg.iter().map(|&x| ((x as f64 - mean) * inv) as f32).collect();
        let tensor = ort::value::Tensor::from_array(([1usize, input.len()], input)).map_err(|e| anyhow!("{e}"))?;
        let outputs = session.run(ort::inputs![tensor]).map_err(|e| anyhow!("{e}"))?;
        let (shape, data) = outputs[0].try_extract_tensor::<f32>().map_err(|e| anyhow!("{e}"))?;
        let (f, v) = (shape[1] as usize, shape[2] as usize);
        vocab = v;
        // Trames correspondant à la partie centrale de la fenêtre.
        let skip = (start - a) / HOP;
        let keep = (WIN / HOP).min(f.saturating_sub(skip)).min(total_frames.saturating_sub(logp.len() / v.max(1)));
        for t in skip..skip + keep {
            let row = &data[t * v..(t + 1) * v];
            let m = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let lse = m + row.iter().map(|x| (x - m).exp()).sum::<f32>().ln();
            logp.extend(row.iter().map(|x| x - lse));
        }
        progress((c + 1) as f32 / chunks as f32);
    }
    if vocab == 0 {
        bail!("audio trop court pour l'alignement");
    }
    Ok(Emissions { frames: logp.len() / vocab, vocab, logp })
}

/// Lettres du modèle : a-z et apostrophe (identifiants du fichier vocab.json).
fn token_of(c: char) -> Option<usize> {
    const ORDER: &str = "aienoutsrmkldghybpwcvjzf'qx";
    ORDER.find(c).map(|i| i + 4)
}

/// Mot → suite de symboles (minuscules, sans accents ; le reste est ignoré).
pub fn tokens(word: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for c in word.chars().flat_map(char::to_lowercase) {
        let s: &str = match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => "a",
            'é' | 'è' | 'ê' | 'ë' => "e",
            'î' | 'ï' | 'í' | 'ì' => "i",
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' | 'ø' => "o",
            'ù' | 'û' | 'ü' | 'ú' => "u",
            'ç' => "c",
            'ñ' => "n",
            'ÿ' | 'ý' => "y",
            'œ' => "oe",
            'æ' => "ae",
            'ß' => "ss",
            '’' | '‘' | '`' => "'",
            _ => {
                if let Some(t) = token_of(c) {
                    out.push(t);
                }
                continue;
            }
        };
        out.extend(s.chars().filter_map(token_of));
    }
    out
}

/// Symbole « joker » : absorbe sans pénalité tout ce qui n'est pas dans le texte
/// (chœurs, rires, répétitions absentes des paroles, introduction parlée…).
const STAR: usize = usize::MAX;
/// Log-probabilité du joker sur une trame (0 : gratuit, comme ctc-forced-aligner).
const STAR_LOGP: f32 = 0.0;

/// Alignement de Viterbi CTC : renvoie, pour chaque mot, (début, fin, confiance) en
/// secondes, ou `None` pour un mot sans lettre alignable.
pub fn align_words(em: &Emissions, words: &[Vec<usize>]) -> Vec<Option<(f64, f64, f32)>> {
    align_words_with_breaks(em, words, &[], &[])
}

/// Comme [`align_words`], avec un joker au début, à la fin et devant chaque mot dont
/// l'indice figure dans `breaks` (débuts de vers) : la voix chantée hors du texte entre
/// deux vers ne peut plus attirer les mots. `bounds` (vide, ou un par mot) : intervalle
/// en secondes où les lettres du mot doivent se trouver.
pub fn align_words_with_breaks(
    em: &Emissions,
    words: &[Vec<usize>],
    breaks: &[usize],
    bounds: &[(f64, f64)],
) -> Vec<Option<(f64, f64, f32)>> {
    let stars = !breaks.is_empty();
    let mut seq = Vec::new();
    let mut owner = Vec::new();
    for (wi, w) in words.iter().enumerate() {
        if stars && (wi == 0 || breaks.contains(&wi)) && seq.last() != Some(&STAR) {
            seq.push(STAR);
            owner.push(usize::MAX);
        }
        for &t in w {
            seq.push(t);
            owner.push(wi);
        }
    }
    if stars {
        seq.push(STAR);
        owner.push(usize::MAX);
    }
    let emit = |t: usize, k: usize| if k == STAR { STAR_LOGP } else { em.at(t, k) };
    // Trames permises pour chaque symbole (le joker n'est pas borné).
    let allowed: Vec<(usize, usize)> = owner
        .iter()
        .map(|&wi| match bounds.get(wi) {
            Some(&(a, b)) => ((a.max(0.0) / FRAME_S) as usize, (b.max(0.0) / FRAME_S) as usize),
            None => (0, usize::MAX),
        })
        .collect();
    let l = seq.len();
    let t_max = em.frames;
    if l == 0 || t_max < l {
        return vec![None; words.len()];
    }
    // États : blanc, lettre 0, blanc, lettre 1, …, blanc (2L + 1).
    let s_max = 2 * l + 1;
    let label = |s: usize| if s % 2 == 0 { BLANK } else { seq[s / 2] };
    let neg = f32::NEG_INFINITY;
    let mut prev = vec![neg; s_max];
    let mut cur = vec![neg; s_max];
    // Retour arrière : 0 = rester, 1 = depuis s-1, 2 = depuis s-2.
    let mut back = vec![0u8; t_max * s_max];
    prev[0] = em.at(0, BLANK);
    if allowed[0].0 == 0 {
        prev[1] = emit(0, seq[0]);
    }
    for t in 1..t_max {
        // États atteignables : ni trop en avance, ni trop en retard pour finir.
        let s_lo = (2 * l + 1).saturating_sub(2 * (t_max - t));
        let s_hi = (2 * t + 2).min(s_max);
        cur.fill(neg);
        for s in s_lo..s_hi {
            if s % 2 == 1 && (t < allowed[s / 2].0 || t > allowed[s / 2].1) {
                continue;
            }
            let mut best = prev[s];
            let mut from = 0u8;
            if s >= 1 && prev[s - 1] > best {
                best = prev[s - 1];
                from = 1;
            }
            if s >= 2 && s % 2 == 1 && seq[s / 2] != seq[s / 2 - 1] && prev[s - 2] > best {
                best = prev[s - 2];
                from = 2;
            }
            if best > neg {
                cur[s] = best + emit(t, label(s));
                back[t * s_max + s] = from;
            }
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let mut s = if prev[s_max - 1] >= prev[s_max - 2] { s_max - 1 } else { s_max - 2 };
    if prev[s] == neg {
        return vec![None; words.len()];
    }
    // Remontée : trames de chaque lettre.
    let mut first = vec![usize::MAX; l];
    let mut last = vec![usize::MAX; l];
    let mut score = vec![0f32; l];
    for t in (0..t_max).rev() {
        if s % 2 == 1 {
            let k = s / 2;
            // À rebours : la première rencontre d'une lettre est sa dernière trame.
            if last[k] == usize::MAX {
                last[k] = t;
            }
            first[k] = t;
            score[k] += emit(t, seq[k]).exp();
        }
        if t > 0 {
            s -= back[t * s_max + s] as usize;
        }
    }
    let mut out = vec![None; words.len()];
    for (k, &wi) in owner.iter().enumerate() {
        if first[k] == usize::MAX || wi == usize::MAX {
            continue;
        }
        let (s0, e0) = (first[k] as f64 * FRAME_S, (last[k] + 1) as f64 * FRAME_S);
        let frames = (last[k] + 1 - first[k]).max(1) as f32;
        let conf = score[k] / frames;
        out[wi] = Some(match out[wi] {
            None => (s0, e0, conf),
            Some((a, b, c)) => (f64::min(a, s0), f64::max(b, e0), (c + conf) / 2.0),
        });
    }
    out
}

/// Marge autour du placement approximatif d'un vers (voir [`align_lines`]).
pub const GUIDE_MARGIN_S: f64 = 4.0;

/// Aligne des vers (texte exact) sur les émissions et renvoie les lignes horodatées.
/// Les mots sans lettre alignable sont placés entre leurs voisins.
///
/// `guide` : placement approximatif des mêmes vers (heures LRC recalées sur les mots
/// reconnus). Chaque vers est alors cherché entre le début de son vers guide moins
/// [`GUIDE_MARGIN_S`] et le début du vers guide suivant plus cette marge, ce qui évite
/// qu'un refrain répété ou des chœurs l'attirent loin de sa place.
pub fn align_lines(text: &[crate::align::TextLine], em: &Emissions, guide: Option<&[Line]>) -> Option<Vec<Line>> {
    let verses: Vec<Vec<String>> = text
        .iter()
        .map(|l| l.text.split_whitespace().map(String::from).collect::<Vec<_>>())
        .filter(|w| !w.is_empty())
        .collect();
    let flat: Vec<&String> = verses.iter().flatten().collect();
    let toks: Vec<Vec<usize>> = flat.iter().map(|w| tokens(w)).collect();
    if toks.iter().all(|t| t.is_empty()) {
        return None;
    }
    // Débuts de vers : un joker les précède.
    let breaks: Vec<usize> = verses.iter().scan(0, |n, v| { let i = *n; *n += v.len(); Some(i) }).collect();
    let mut bounds = Vec::new();
    if let Some(g) = guide.filter(|g| g.len() == verses.len()) {
        for (i, v) in verses.iter().enumerate() {
            let lo = g[i].start() - GUIDE_MARGIN_S;
            let end = g[i].words.last().map_or(g[i].start(), |w| w.end);
            let hi = g.get(i + 1).map_or(end, |n| n.start().max(end)) + GUIDE_MARGIN_S;
            bounds.extend(std::iter::repeat_n((lo, hi), v.len()));
        }
    }
    let mut times = align_words_with_breaks(em, &toks, &breaks, &bounds);
    if !bounds.is_empty() && times.iter().all(Option::is_none) {
        // Guide incompatible avec l'audio : alignement libre.
        times = align_words_with_breaks(em, &toks, &breaks, &[]);
    }
    if times.iter().all(Option::is_none) {
        return None;
    }
    // Mots sans lettre (chiffres, symboles…) : entre le mot précédent et le suivant.
    let mut spans: Vec<(f64, f64)> = Vec::with_capacity(flat.len());
    for (i, t) in times.iter().enumerate() {
        match t {
            Some((s, e, _)) => spans.push((*s, *e)),
            None => {
                let prev_end = spans.last().map_or(0.0, |p| p.1);
                let next = times[i + 1..].iter().flatten().next().map_or(prev_end + 0.3, |n| n.0);
                spans.push((prev_end, prev_end.max(next.min(prev_end + 0.3))));
            }
        }
    }
    let mut lines = Vec::with_capacity(verses.len());
    let mut k = 0;
    for v in &verses {
        let words = v
            .iter()
            .map(|w| {
                let (s, e) = spans[k];
                k += 1;
                Word { text: w.clone(), start: s, end: e.max(s + 0.08) }
            })
            .collect();
        lines.push(Line { words });
    }
    crate::align::enforce_order(&mut lines);
    Some(lines)
}

/// Émissions d'une chanson : relues si déjà calculées, sinon calculées depuis la voix
/// isolée (original − karaoké) puis conservées.
pub fn load_or_compute(
    work_dir: &Path,
    karaoke: &Path,
    runtime: &Path,
    model: &Path,
    progress: &mut dyn FnMut(f32),
) -> Result<Emissions> {
    let path = work_dir.join(EMISSIONS_FILE);
    if let Ok(e) = Emissions::load(&path) {
        return Ok(e);
    }
    let source = crate::download::find_source(work_dir)
        .or_else(|| {
            std::fs::read_to_string(work_dir.join("source.txt"))
                .ok()
                .map(|s| std::path::PathBuf::from(s.trim()))
                .filter(|p| p.is_file())
        })
        .context("fichier d'origine introuvable (voix nécessaire à l'alignement)")?;
    let vocals = crate::vocal::vocals_from_files(&source, karaoke)?;
    let mono = crate::media::to_mono_16k(&vocals, crate::media::SAMPLE_RATE)?;
    drop(vocals);
    let em = emissions(runtime, model, &mono, progress)?;
    let _ = em.save(&path);
    Ok(em)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lettres() {
        assert_eq!(tokens("Été"), vec![6, 10, 6]);
        assert_eq!(tokens("l'œuf"), vec![15, 28, 8, 6, 9, 27]);
        assert!(tokens("♪ 42").is_empty());
    }

    /// Émissions synthétiques : chaque lettre est très probable sur ses trames.
    #[test]
    fn viterbi_sur_emissions_synthetiques() {
        // « ab » de 0,2 à 0,4 s, « ba » de 1,0 à 1,2 s, blancs ailleurs (2 s au total).
        let frames = 100;
        let vocab = 31;
        let (a, b) = (4usize, 20usize);
        let mut logp = vec![(0.01f32).ln(); frames * vocab];
        let mut set = |t: usize, k: usize| {
            for j in 0..vocab {
                logp[t * vocab + j] = if j == k { (0.9f32).ln() } else { (0.1f32 / 30.0).ln() };
            }
        };
        for t in 0..frames {
            set(t, BLANK);
        }
        for t in 10..15 { set(t, a); }
        for t in 15..20 { set(t, b); }
        for t in 50..55 { set(t, b); }
        for t in 55..60 { set(t, a); }
        let em = Emissions { frames, vocab, logp };
        let r = align_words(&em, &[vec![a, b], vec![b, a]]);
        let (s0, e0, _) = r[0].unwrap();
        let (s1, e1, _) = r[1].unwrap();
        assert!((s0 - 0.20).abs() < 0.03 && (e0 - 0.40).abs() < 0.03, "{s0} {e0}");
        assert!((s1 - 1.00).abs() < 0.03 && (e1 - 1.20).abs() < 0.03, "{s1} {e1}");
    }
}
