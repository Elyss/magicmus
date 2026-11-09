//! Attaques de la voix : instants où l'énergie de la voix isolée monte brusquement,
//! ce qui correspond en général au début d'une syllabe chantée. Elles servent à placer
//! les mots que la reconnaissance vocale n'a pas transcrits.

use crate::media;
use anyhow::Result;
use std::path::Path;

/// Pas d'analyse : 10 ms.
const HOP_S: f64 = 0.01;

/// Détecte les attaques dans la voix isolée (`vocals` : stéréo entrelacé à `sample_rate`).
/// Renvoie les instants (s), croissants, espacés d'au moins 120 ms.
pub fn onsets(vocals: &[f32], sample_rate: u32) -> Vec<f64> {
    onsets_with(vocals, sample_rate, RISE_DB)
}

/// Montée d'énergie minimale (dB sur 30 ms) pour une attaque. Réglée au banc d'essai
/// (`examples/diag_paroles.rs --banc debut --seuils`) : 4 dB donnait le plus petit
/// écart moyen sur les mots non reconnus.
pub const RISE_DB: f64 = 4.0;

/// Comme [`onsets`], avec un seuil de montée choisi (réglage, banc d'essai).
pub fn onsets_with(vocals: &[f32], sample_rate: u32, rise_db: f64) -> Vec<f64> {
    let hop = (sample_rate as f64 * HOP_S) as usize;
    if hop == 0 {
        return Vec::new();
    }
    // Énergie (dB) par tranche de 10 ms, voix en mono.
    let frames = vocals.len() / 2 / hop;
    let mut db = Vec::with_capacity(frames);
    for f in 0..frames {
        let chunk = &vocals[f * hop * 2..(f + 1) * hop * 2];
        let e: f64 = chunk.chunks_exact(2).map(|s| {
            let m = 0.5 * (s[0] + s[1]) as f64;
            m * m
        }).sum::<f64>() / hop as f64;
        db.push(10.0 * (e + 1e-12).log10());
    }
    if db.len() < 10 {
        return Vec::new();
    }
    // Lissage léger (3 tranches).
    let smooth: Vec<f64> = (0..db.len())
        .map(|i| {
            let a = i.saturating_sub(1);
            let b = (i + 2).min(db.len());
            db[a..b].iter().sum::<f64>() / (b - a) as f64
        })
        .collect();
    // Seuil de présence : 35 dB sous le niveau fort de la voix (95e centile).
    let mut sorted = smooth.clone();
    sorted.sort_by(f64::total_cmp);
    let loud = sorted[(sorted.len() as f64 * 0.95) as usize];
    let floor = loud - 35.0;

    // Montée d'énergie sur 30 ms ; maxima locaux au-dessus de 6 dB.
    let rise: Vec<f64> = (0..smooth.len())
        .map(|i| if i >= 3 { (smooth[i] - smooth[i - 3]).max(0.0) } else { 0.0 })
        .collect();
    let mut out: Vec<f64> = Vec::new();
    for i in 1..rise.len() - 1 {
        let r = rise[i];
        if r < rise_db || smooth[i] < floor || r < rise[i - 1] || r < rise[i + 1] {
            continue;
        }
        let t = (i as f64 - 2.0) * HOP_S; // début de la montée
        match out.last() {
            Some(&last) if t - last < 0.12 => {}
            _ => out.push(t.max(0.0)),
        }
    }
    out
}

/// Attaques calculées à partir des fichiers (original − karaoké = voix isolée).
pub fn onsets_from_files(source: &Path, karaoke: &Path) -> Result<Vec<f64>> {
    Ok(onsets(&vocals_from_files(source, karaoke)?, media::SAMPLE_RATE))
}

/// Voix isolée (original − karaoké), stéréo entrelacé à `media::SAMPLE_RATE`.
pub fn vocals_from_files(source: &Path, karaoke: &Path) -> Result<Vec<f32>> {
    let mix = media::decode_pcm(source)?;
    let inst = media::decode_pcm(karaoke)?;
    let n = mix.len().min(inst.len());
    Ok(mix[..n].iter().zip(&inst[..n]).map(|(m, i)| m - i).collect())
}

/// Fichier où les attaques sont conservées, dans le dossier de la chanson.
pub const ONSETS_FILE: &str = "attaques.json";

pub fn save(dir: &Path, onsets: &[f64]) {
    let rounded: Vec<f64> = onsets.iter().map(|t| (t * 100.0).round() / 100.0).collect();
    let _ = crate::library::save_json(&dir.join(ONSETS_FILE), &rounded);
}

/// Attaques d'une chanson : relues si déjà calculées, sinon calculées puis conservées.
pub fn load_or_compute(dir: &Path, karaoke: &Path) -> Vec<f64> {
    let cached: Vec<f64> = crate::library::load_json(&dir.join(ONSETS_FILE));
    if !cached.is_empty() {
        return cached;
    }
    let Some(source) = crate::download::find_source(dir).or_else(|| {
        std::fs::read_to_string(dir.join("source.txt")).ok().map(|s| s.trim().into()).filter(|p: &std::path::PathBuf| p.is_file())
    }) else {
        return Vec::new();
    };
    match onsets_from_files(&source, karaoke) {
        Ok(o) => {
            save(dir, &o);
            o
        }
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attaques_de_notes() {
        // Trois « syllabes » (salves de sinus) séparées de silences, à 0,5 / 1,2 / 2,0 s.
        let sr = 44_100;
        let mut pcm = vec![0f32; sr * 3 * 2];
        for &(start, dur) in &[(0.5, 0.4), (1.2, 0.5), (2.0, 0.6)] {
            let a = (start * sr as f64) as usize;
            let b = ((start + dur) * sr as f64) as usize;
            for i in a..b {
                let v = (2.0 * std::f32::consts::PI * 300.0 * i as f32 / sr as f32).sin() * 0.3;
                pcm[2 * i] = v;
                pcm[2 * i + 1] = v;
            }
        }
        let o = onsets(&pcm, sr as u32);
        assert_eq!(o.len(), 3, "{o:?}");
        for (got, want) in o.iter().zip([0.5, 1.2, 2.0]) {
            assert!((got - want).abs() < 0.04, "{got} au lieu de {want}");
        }
    }
}
