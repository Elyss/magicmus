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
/// Log-probabilité du joker sur une trame : faible coût, pour qu'il ne grignote pas les
/// lettres bien entendues en bord de vers (gratuit, il retardait les débuts de vers),
/// tout en restant bien moins cher que le blanc sur une voix hors texte.
const STAR_LOGP: f32 = -0.1;

/// Alignement de Viterbi CTC : renvoie, pour chaque mot, (début, fin, confiance) en
/// secondes, ou `None` pour un mot sans lettre alignable.
pub fn align_words(em: &Emissions, words: &[Vec<usize>]) -> Vec<Option<(f64, f64, f32)>> {
    align_words_with_breaks(em, words, &[], &[], &[])
}

/// Pénalité (log-probabilité, par lettre) pour sauter un vers entier : un vers dont les
/// lettres s'entendent moins bien que cela en moyenne est jugé absent de la version
/// chantée (reprise raccourcie, couplet coupé, paroles d'une autre version).
pub const SKIP_PER_LETTER: f32 = 6.0;
/// Vers non entendu par Whisper (3 mots ou plus) que l'alignement a dû tasser en moins
/// de ce temps par mot : jugé absent de la version chantée.
const MIN_S_PER_WORD: f64 = 0.2;

/// Comme [`align_words`], avec un joker au début, à la fin et devant chaque mot dont
/// l'indice figure dans `breaks` (débuts de vers) : la voix chantée hors du texte entre
/// deux vers ne peut plus attirer les mots. `bounds` (vide, ou un par mot) : intervalle
/// en secondes où les lettres du mot doivent se trouver. `skip` (vide, ou un par vers) :
/// pénalité par lettre pour sauter ce vers entier (ses mots restent alors à `None`),
/// `None` pour un vers qui ne peut pas être sauté.
pub fn align_words_with_breaks(
    em: &Emissions,
    words: &[Vec<usize>],
    breaks: &[usize],
    bounds: &[(f64, f64)],
    skip: &[Option<f32>],
) -> Vec<Option<(f64, f64, f32)>> {
    let stars = !breaks.is_empty();
    let mut seq = Vec::new();
    let mut owner = Vec::new();
    for (wi, w) in words.iter().enumerate() {
        if stars && (wi == 0 || breaks.contains(&wi)) {
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
    if l == 0 || t_max < 2 {
        return vec![None; words.len()];
    }
    // États : blanc, lettre 0, blanc, lettre 1, …, blanc (2L + 1).
    let s_max = 2 * l + 1;
    let label = |s: usize| if s % 2 == 0 { BLANK } else { seq[s / 2] };
    // Saut de vers : d'un joker au joker suivant, avec la pénalité du vers sauté.
    let mut jump: Vec<Option<(usize, f32)>> = vec![None; s_max];
    if stars {
        // Un joker devant chaque vers et un à la fin : le vers v va du joker v au joker v + 1.
        let star_states: Vec<usize> = (0..l).filter(|&k| seq[k] == STAR).map(|k| 2 * k + 1).collect();
        for (v, w) in star_states.windows(2).enumerate() {
            if let Some(per_letter) = skip.get(v).copied().flatten() {
                let letters = (w[1] - w[0]) / 2 - 1;
                jump[w[1]] = Some((w[0], -per_letter * letters as f32));
            }
        }
    }
    let neg = f32::NEG_INFINITY;
    let mut prev = vec![neg; s_max];
    let mut cur = vec![neg; s_max];
    // Retour arrière : 0 = rester, 1 = depuis s-1, 2 = depuis s-2, 3 = saut de vers.
    let mut back = vec![0u8; t_max * s_max];
    prev[0] = em.at(0, BLANK);
    if allowed[0].0 == 0 {
        prev[1] = emit(0, seq[0]);
    }
    for t in 1..t_max {
        cur.fill(neg);
        for s in 0..s_max {
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
            if let Some((ps, cost)) = jump[s] {
                if prev[ps] + cost > best {
                    best = prev[ps] + cost;
                    from = 3;
                }
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
            s = match back[t * s_max + s] {
                3 => jump[s].map_or(s, |j| j.0),
                d => s - d as usize,
            };
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

/// Marge autour de la place attendue d'un vers dans le guide.
pub const GUIDE_MARGIN_S: f64 = 4.0;
/// Écart toléré entre la durée LRC et la durée entendue séparant deux vers repères
/// pour que les heures LRC servent à placer les vers intermédiaires (sinon un passage
/// a été coupé ou ajouté entre eux).
const GUIDE_CONSISTENT_S: f64 = 2.5;

/// Fenêtre de recherche d'un vers (secondes). `heard` : Whisper a entendu ce vers (au
/// moins 40 % de ses mots). `sung` : vers certainement chanté — entendu, ou situé entre
/// deux vers entendus séparés de la même durée que dans les paroles LRC (aucun passage
/// coupé entre eux) ; il ne peut pas être sauté.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub lo: f64,
    pub hi: f64,
    pub heard: bool,
    pub sung: bool,
}

/// Vers repère : au moins 40 % de ses mots (et deux, ou son mot unique) entendus par Whisper.
#[derive(Clone, Copy)]
struct Anchor {
    start: f64,
    end: f64,
}

/// Fenêtre de recherche de chaque vers non vide de `text`, d'après les mots reconnus
/// par Whisper appariés au texte sans contrainte de temps (`align::match_verses`).
///
/// - vers repère : autour des mots entendus ;
/// - entre deux repères : à la place que donnent les heures LRC si la durée LRC entre
///   les repères correspond à la durée entendue, sinon n'importe où entre eux (passage
///   coupé ou ajouté : l'alignement forcé décide, et peut sauter le vers) ;
/// - avant le premier ou après le dernier repère : heures LRC prolongées depuis le
///   repère, avec une marge qui grandit avec la distance (10 %), sinon sans borne.
///
/// `None` s'il y a moins de trois vers repères.
pub fn guide_windows(text: &[crate::align::TextLine], rec: &[crate::lyrics::Word]) -> Option<Vec<Window>> {
    let matches = crate::align::match_verses(text, rec);
    let lrc: Vec<Option<f64>> = text.iter().filter(|t| !t.text.trim().is_empty()).map(|t| t.start).collect();
    let anchors: Vec<Option<Anchor>> = matches
        .iter()
        .map(|m| {
            let hits: Vec<(usize, usize)> = m.iter().enumerate().filter_map(|(k, j)| j.map(|j| (k, j))).collect();
            // Au moins deux mots (ou le mot unique) et 40 % du vers : deux mots courants
            // appariés par hasard ne suffisent pas.
            if hits.len() < 2.min(m.len()).max((m.len() * 2).div_ceil(5)) {
                return None;
            }
            let (k0, j0) = hits[0];
            let (k1, j1) = *hits.last().unwrap();
            Some(Anchor {
                start: (rec[j0].start - 0.3 * k0 as f64).max(0.0),
                end: rec[j1].end + 0.3 * (m.len() - 1 - k1) as f64,
            })
        })
        .collect();
    if anchors.iter().flatten().count() < 3 {
        return None;
    }
    let anchors = reassign_copies(text, &lrc, anchors);
    let n = matches.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        if let Some(a) = anchors[i] {
            out.push(Window { lo: a.start - GUIDE_MARGIN_S, hi: a.end + GUIDE_MARGIN_S, heard: true, sung: true });
            continue;
        }
        let p = (0..i).rev().find(|&k| anchors[k].is_some());
        let q = (i + 1..n).find(|&k| anchors[k].is_some());
        // Durée LRC du vers (jusqu'au suivant), pour la fin de fenêtre.
        let span = |k: usize| match (lrc[k], lrc.get(k + 1).copied().flatten()) {
            (Some(a), Some(b)) if b > a => (b - a).min(12.0),
            _ => 4.0,
        };
        let mut sung = false;
        let w = match (p, q) {
            (Some(p), Some(q)) => {
                let (ap, aq) = (anchors[p].unwrap(), anchors[q].unwrap());
                match (lrc[p], lrc[i], lrc[q]) {
                    (Some(lp), Some(li), Some(lq)) if lq > lp && ((aq.start - ap.start) - (lq - lp)).abs() <= GUIDE_CONSISTENT_S => {
                        // Correspondance linéaire LRC → voix entre les deux repères.
                        let e = ap.start + (li - lp) * (aq.start - ap.start) / (lq - lp);
                        sung = true;
                        (e - GUIDE_MARGIN_S, e + span(i) + GUIDE_MARGIN_S)
                    }
                    _ => (ap.end - GUIDE_MARGIN_S, aq.start + GUIDE_MARGIN_S),
                }
            }
            (Some(p), None) => {
                let ap = anchors[p].unwrap();
                match (lrc[p], lrc[i]) {
                    (Some(lp), Some(li)) if li >= lp => {
                        let e = ap.start + (li - lp);
                        let m = GUIDE_MARGIN_S + 0.1 * (li - lp);
                        (e - m, e + span(i) + m)
                    }
                    _ => (ap.end - GUIDE_MARGIN_S, f64::INFINITY),
                }
            }
            (None, Some(q)) => {
                let aq = anchors[q].unwrap();
                match (lrc[i], lrc[q]) {
                    (Some(li), Some(lq)) if lq >= li => {
                        let e = aq.start - (lq - li);
                        let m = GUIDE_MARGIN_S + 0.1 * (lq - li);
                        (e - m, e + span(i) + m)
                    }
                    _ => (0.0, aq.start + GUIDE_MARGIN_S),
                }
            }
            (None, None) => (0.0, f64::INFINITY),
        };
        out.push(Window { lo: w.0, hi: w.1, heard: false, sung });
    }
    Some(out)
}

/// Vers identiques (refrains, couplets repris) : l'appariement mot à mot ne sait pas à
/// quelle copie attribuer ce qu'il entend. On choisit, pour chaque vers repère, la copie
/// (dans l'ordre du texte) qui garde l'écart voix − LRC le plus régulier d'un repère au
/// suivant ; sans heures LRC, celle qui saute le moins de vers.
fn reassign_copies(text: &[crate::align::TextLine], lrc: &[Option<f64>], anchors: Vec<Option<Anchor>>) -> Vec<Option<Anchor>> {
    let norm: Vec<Vec<String>> = text
        .iter()
        .filter(|t| !t.text.trim().is_empty())
        .map(|t| t.text.split_whitespace().map(crate::align::normalize).collect())
        .collect();
    let list: Vec<(usize, Anchor)> = anchors.iter().enumerate().filter_map(|(i, a)| a.map(|a| (i, a))).collect();
    // Copies : vers semblables à 80 % au moins (plus longue sous-suite commune de mots).
    let similar = |a: &[String], b: &[String]| {
        let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in 0..a.len() {
            for j in 0..b.len() {
                d[i + 1][j + 1] = if a[i] == b[j] { d[i][j] + 1 } else { d[i][j + 1].max(d[i + 1][j]) };
            }
        }
        2 * d[a.len()][b.len()] * 10 >= 8 * (a.len() + b.len())
    };
    let cands: Vec<Vec<usize>> =
        list.iter().map(|(i, _)| (0..norm.len()).filter(|&c| c == *i || similar(&norm[c], &norm[*i])).collect()).collect();
    let timed = lrc.iter().all(Option::is_some);
    // Coût d'un passage du repère kp (copie cp) au repère k (copie c).
    let trans = |kp: usize, cp: usize, k: usize, c: usize| -> f64 {
        if timed {
            let off = list[k].1.start - lrc[c].unwrap();
            let off_p = list[kp].1.start - lrc[cp].unwrap();
            (off - off_p).abs()
        } else {
            (c - cp - 1) as f64
        }
    };
    // Écarter un repère (mots appariés par erreur) coûte l'équivalent de 8 s d'écart.
    const DROP: f64 = 8.0;
    let m = list.len();
    // Programmation dynamique : repères gardés, copies strictement croissantes.
    let mut cost: Vec<Vec<f64>> = vec![];
    let mut from: Vec<Vec<Option<(usize, usize)>>> = vec![];
    for k in 0..m {
        let mut ck = vec![f64::INFINITY; cands[k].len()];
        let mut fk = vec![None; cands[k].len()];
        for (ci, &c) in cands[k].iter().enumerate() {
            ck[ci] = DROP * k as f64 + c as f64 * 1e-3;
            for kp in 0..k {
                for (pi, &cp) in cands[kp].iter().enumerate() {
                    if cp < c && cost[kp][pi].is_finite() {
                        let v = cost[kp][pi] + trans(kp, cp, k, c) + DROP * (k - kp - 1) as f64 + c as f64 * 1e-6;
                        if v < ck[ci] {
                            ck[ci] = v;
                            fk[ci] = Some((kp, pi));
                        }
                    }
                }
            }
        }
        cost.push(ck);
        from.push(fk);
    }
    let mut best: Option<(usize, usize, f64)> = None;
    for k in 0..m {
        for (ci, &v) in cost[k].iter().enumerate() {
            let total = v + DROP * (m - 1 - k) as f64;
            if best.is_none_or(|b| total < b.2) {
                best = Some((k, ci, total));
            }
        }
    }
    let Some((mut k, mut ci, _)) = best else { return anchors };
    let mut out = vec![None; anchors.len()];
    loop {
        out[cands[k][ci]] = Some(list[k].1);
        match from[k][ci] {
            Some((kp, pi)) => (k, ci) = (kp, pi),
            None => break,
        }
    }
    out
}

/// Comme [`align_verses`] avec la pénalité de saut par défaut, sans les vers sautés.
pub fn align_lines(text: &[crate::align::TextLine], em: &Emissions, guide: Option<&[Window]>) -> Option<Vec<Line>> {
    align_verses(text, em, guide, Some(SKIP_PER_LETTER)).map(|v| v.into_iter().flatten().collect())
}

/// Aligne des vers (texte exact) sur les émissions : une entrée par vers non vide de
/// `text`, `None` pour un vers que la voix ne chante pas (sauté, voir
/// [`SKIP_PER_LETTER`]). Les mots sans lettre alignable sont placés entre leurs voisins.
///
/// `guide` : fenêtre de recherche de chaque vers non vide, voir [`guide_windows`] ;
/// elle évite qu'un refrain répété ou des chœurs attirent un vers loin de sa place.
/// `skip` : pénalité de saut par lettre (les vers certainement chantés, voir
/// [`Window::sung`], ne sont jamais sautés).
pub fn align_verses(
    text: &[crate::align::TextLine],
    em: &Emissions,
    guide: Option<&[Window]>,
    skip: Option<f32>,
) -> Option<Vec<Option<Line>>> {
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
    let breaks: Vec<usize> = verses
        .iter()
        .scan(0, |n, v| {
            let i = *n;
            *n += v.len();
            Some(i)
        })
        .collect();
    let guide = guide.filter(|g| g.len() == verses.len());
    let mut bounds = Vec::new();
    if let Some(g) = guide {
        for (v, w) in verses.iter().zip(g) {
            bounds.extend(std::iter::repeat_n((w.lo, w.hi.min(1e7)), v.len()));
        }
    }
    let sure = |v: usize| guide.is_some_and(|g| g[v].sung);
    let skips: Vec<Option<f32>> = (0..verses.len()).map(|v| skip.filter(|_| !sure(v))).collect();
    let mut times = align_words_with_breaks(em, &toks, &breaks, &bounds, &skips);
    if !bounds.is_empty() && times.iter().all(Option::is_none) {
        // Guide incompatible avec l'audio : alignement libre.
        let free: Vec<Option<f32>> = vec![skip; verses.len()];
        times = align_words_with_breaks(em, &toks, &breaks, &[], &free);
    }
    if times.iter().all(Option::is_none) {
        return None;
    }
    // Vers chantés : au moins un mot placé (ou aucun mot alignable).
    let mut sung = Vec::with_capacity(verses.len());
    let mut k = 0;
    for v in &verses {
        let r = k..k + v.len();
        k += v.len();
        sung.push(r.clone().all(|i| toks[i].is_empty()) || r.clone().any(|i| times[i].is_some()));
    }
    // Mots sans lettre (chiffres, symboles…) des vers chantés : entre le mot précédent
    // et le suivant.
    let mut spans: Vec<(f64, f64)> = vec![(0.0, 0.0); flat.len()];
    let mut prev_end = 0.0;
    let mut k = 0;
    for (vi, v) in verses.iter().enumerate() {
        for _ in v {
            if sung[vi] {
                spans[k] = match times[k] {
                    Some((s, e, _)) => (s, e),
                    None => {
                        let next = times[k + 1..].iter().flatten().next().map_or(prev_end + 0.3, |n| n.0);
                        (prev_end, f64::max(prev_end, next.min(prev_end + 0.3)))
                    }
                };
                prev_end = spans[k].1;
            }
            k += 1;
        }
    }
    // Vers tassés (non entendus par Whisper) : forcés là où ils ne sont pas chantés.
    let mut k = 0;
    for (vi, v) in verses.iter().enumerate() {
        let (first, last) = (spans[k].0, spans[k + v.len() - 1].1);
        if skip.is_some() && sung[vi] && !sure(vi) && v.len() >= 3 && last - first < MIN_S_PER_WORD * v.len() as f64 {
            sung[vi] = false;
        }
        k += v.len();
    }
    let mut lines: Vec<Line> = Vec::new();
    let mut k = 0;
    for (vi, v) in verses.iter().enumerate() {
        let words: Vec<Word> = v
            .iter()
            .map(|w| {
                let (s, e) = spans[k];
                k += 1;
                Word { text: w.clone(), start: s, end: e.max(s + 0.08) }
            })
            .collect();
        if sung[vi] {
            lines.push(Line { words });
        }
    }
    crate::align::enforce_order(&mut lines);
    let mut it = lines.into_iter();
    Some(sung.into_iter().map(|k| if k { it.next() } else { None }).collect())
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

    /// Un vers absent de l'audio est sauté (sans saut, il est forcé quelque part) ; une
    /// voix hors texte (« c ») n'attire pas les vers grâce au joker.
    #[test]
    fn vers_absent_saute() {
        // Lettre absente : log-probabilité ≈ −8 par trame, comme sur une vraie voix.
        let frames = 150;
        let vocab = 31;
        let (a, b, c, d) = (4usize, 20usize, 7usize, 9usize);
        let mut logp = vec![(0.01f32).ln(); frames * vocab];
        let mut set = |t: usize, k: usize| {
            for j in 0..vocab {
                logp[t * vocab + j] = if j == k { (0.99f32).ln() } else { (0.01f32 / 30.0).ln() };
            }
        };
        for t in 0..frames {
            set(t, BLANK);
        }
        // « ab » à 0,2 s, voix hors texte « c » à 1,0 s, « ba » à 2,0 s.
        for t in 10..15 { set(t, a); }
        for t in 15..20 { set(t, b); }
        for t in 50..60 { set(t, c); }
        for t in 100..105 { set(t, b); }
        for t in 105..110 { set(t, a); }
        let em = Emissions { frames, vocab, logp };
        // Vers : « ab », « ddd » (jamais chanté), « ba ».
        let words = [vec![a, b], vec![d, d, d], vec![b, a]];
        let r = align_words_with_breaks(&em, &words, &[0, 1, 2], &[], &[Some(SKIP_PER_LETTER); 3]);
        assert!(r[1].is_none(), "{:?}", r[1]);
        assert!((r[0].unwrap().0 - 0.20).abs() < 0.03, "{r:?}");
        assert!((r[2].unwrap().0 - 2.00).abs() < 0.03);
        // Sans saut possible, le vers est placé de force.
        let r = align_words_with_breaks(&em, &words, &[0, 1, 2], &[], &[]);
        assert!(r[1].is_some());
    }
}
