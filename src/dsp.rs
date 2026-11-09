//! Suppression de la voix par masque spectral « canal central ».
//!
//! Dans la quasi-totalité des mixages, la voix principale est placée au centre :
//! elle est identique (même amplitude, même phase) sur les canaux gauche et droit,
//! alors que la plupart des instruments sont répartis dans l'image stéréo.
//!
//! Pour chaque trame de la STFT et chaque fréquence, on mesure la ressemblance
//! entre gauche (L) et droite (R) :
//!
//! ```text
//!     sim = 2·Re(L·R̄) / (|L|² + |R|²)   ∈ [-1, 1]   (= 1 ⇔ L = R, donc « au centre »)
//! ```
//!
//! et on applique le gain `1 − force · sim^sélectivité` aux deux canaux, uniquement
//! dans la bande de fréquences de la voix (la basse et la grosse caisse, elles aussi
//! centrées, sont ainsi conservées). Contrairement à la simple soustraction L − R,
//! le résultat reste en stéréo.

use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;
use std::f32::consts::PI;

const FRAME: usize = 4096;
const HOP: usize = FRAME / 4;
/// Lissage temporel des statistiques inter-canaux (réduit les artefacts « bulles »).
const SMOOTHING: f32 = 0.5;

pub struct Params {
    pub sample_rate: u32,
    pub strength: f32,
    pub selectivity: f32,
    pub low_hz: f32,
    pub high_hz: f32,
}

/// Rapport d'énergie côté/milieu : ~0 pour un signal mono.
pub fn stereo_width(pcm: &[f32]) -> f32 {
    let (mut mid, mut side) = (0f64, 0f64);
    for f in pcm.chunks_exact(2) {
        let (l, r) = (f[0] as f64, f[1] as f64);
        mid += (l + r) * (l + r);
        side += (l - r) * (l - r);
    }
    if mid <= 0.0 { 0.0 } else { (side / mid) as f32 }
}

/// Poids de traitement d'une fréquence : 1 dans la bande, rampe douce sur les bords.
fn band_weight(f: f32, lo: f32, hi: f32) -> f32 {
    let (lo0, hi1) = (lo * 0.5, hi * 1.5);
    if f <= lo0 || f >= hi1 {
        0.0
    } else if f < lo {
        (f - lo0) / (lo - lo0)
    } else if f > hi {
        (hi1 - f) / (hi1 - hi)
    } else {
        1.0
    }
}

/// Traite un signal stéréo entrelacé et renvoie le signal sans voix (même format).
pub fn remove_vocals(pcm: &[f32], p: &Params, mut progress: impl FnMut(f32)) -> Vec<f32> {
    let n = pcm.len() / 2;
    let pad = FRAME;
    let total = n + 2 * pad;
    let total = total + (HOP - (total - FRAME) % HOP) % HOP; // aligne sur le pas

    // Désentrelacement avec marges de silence (toutes les trames sont complètes).
    let mut l = vec![0f32; total];
    let mut r = vec![0f32; total];
    for (i, f) in pcm.chunks_exact(2).enumerate() {
        l[pad + i] = f[0];
        r[pad + i] = f[1];
    }

    let window: Vec<f32> = (0..FRAME)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / FRAME as f32).cos())
        .collect();
    let half = FRAME / 2;
    let bin_hz = p.sample_rate as f32 / FRAME as f32;
    let weights: Vec<f32> = (0..=half)
        .map(|k| band_weight(k as f32 * bin_hz, p.low_hz, p.high_hz))
        .collect();

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FRAME);
    let ifft = planner.plan_fft_inverse(FRAME);
    let mut scratch = vec![
        Complex32::default();
        fft.get_inplace_scratch_len().max(ifft.get_inplace_scratch_len())
    ];
    let mut bl = vec![Complex32::default(); FRAME];
    let mut br = vec![Complex32::default(); FRAME];

    let mut s_ll = vec![0f32; half + 1];
    let mut s_rr = vec![0f32; half + 1];
    let mut s_lr = vec![0f32; half + 1];

    let mut out_l = vec![0f32; total];
    let mut out_r = vec![0f32; total];
    let mut norm = vec![0f32; total];
    let inv_n = 1.0 / FRAME as f32;
    let n_frames = (total - FRAME) / HOP + 1;

    for (frame, pos) in (0..=total - FRAME).step_by(HOP).enumerate() {
        for i in 0..FRAME {
            bl[i] = Complex32::new(l[pos + i] * window[i], 0.0);
            br[i] = Complex32::new(r[pos + i] * window[i], 0.0);
        }
        fft.process_with_scratch(&mut bl, &mut scratch);
        fft.process_with_scratch(&mut br, &mut scratch);

        for k in 0..=half {
            let w = weights[k];
            if w == 0.0 {
                continue;
            }
            let (a, b) = (bl[k], br[k]);
            s_ll[k] = SMOOTHING * s_ll[k] + (1.0 - SMOOTHING) * a.norm_sqr();
            s_rr[k] = SMOOTHING * s_rr[k] + (1.0 - SMOOTHING) * b.norm_sqr();
            s_lr[k] = SMOOTHING * s_lr[k] + (1.0 - SMOOTHING) * (a * b.conj()).re;
            let denom = s_ll[k] + s_rr[k];
            let sim = if denom > 1e-12 { (2.0 * s_lr[k] / denom).clamp(0.0, 1.0) } else { 0.0 };
            let gain = 1.0 - p.strength * w * sim.powf(p.selectivity);

            bl[k] *= gain;
            br[k] *= gain;
            // Symétrie hermitienne : le signal reconstruit reste réel.
            if k != 0 && k != half {
                bl[FRAME - k] *= gain;
                br[FRAME - k] *= gain;
            }
        }

        ifft.process_with_scratch(&mut bl, &mut scratch);
        ifft.process_with_scratch(&mut br, &mut scratch);
        for i in 0..FRAME {
            let w = window[i];
            out_l[pos + i] += bl[i].re * inv_n * w;
            out_r[pos + i] += br[i].re * inv_n * w;
            norm[pos + i] += w * w;
        }

        if frame % 64 == 0 {
            progress(frame as f32 / n_frames as f32);
        }
    }
    progress(1.0);

    // Normalisation de l'addition-recouvrement et ré-entrelacement.
    let mut out = Vec::with_capacity(n * 2);
    for i in pad..pad + n {
        let g = if norm[i] > 1e-6 { 1.0 / norm[i] } else { 0.0 };
        out.push(out_l[i] * g);
        out.push(out_r[i] * g);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    fn params() -> Params {
        Params { sample_rate: SR, strength: 1.0, selectivity: 4.0, low_hz: 150.0, high_hz: 8000.0 }
    }

    fn tone(freq: f32, secs: f32) -> Vec<f32> {
        (0..(secs * SR as f32) as usize)
            .map(|i| (2.0 * PI * freq * i as f32 / SR as f32).sin() * 0.5)
            .collect()
    }

    /// Énergie d'un canal, en ignorant les bords.
    fn energy(pcm: &[f32], ch: usize) -> f32 {
        let s: Vec<f32> = pcm.chunks_exact(2).map(|f| f[ch]).collect();
        let m = s.len() / 10;
        s[m..s.len() - m].iter().map(|x| x * x).sum::<f32>()
    }

    fn db(after: f32, before: f32) -> f32 {
        10.0 * (after / before).log10()
    }

    #[test]
    fn centre_supprime_cote_conserve() {
        // « Voix » au centre à 440 Hz + « guitare » à gauche à 1 kHz.
        let voice = tone(440.0, 2.0);
        let guitar = tone(1000.0, 2.0);
        let mut pcm = Vec::new();
        for (v, g) in voice.iter().zip(&guitar) {
            pcm.push(v + g);
            pcm.push(*v);
        }
        let out = remove_vocals(&pcm, &params(), |_| {});
        assert_eq!(out.len(), pcm.len());

        // Canal droit = voix seule : doit être fortement atténuée.
        let att_voice = db(energy(&out, 1), energy(&pcm, 1));
        assert!(att_voice < -20.0, "voix atténuée de seulement {att_voice:.1} dB");

        // Canal gauche ≈ guitare seule.
        let only_guitar: Vec<f32> = guitar.iter().flat_map(|g| [*g, 0.0]).collect();
        let diff = db(energy(&out, 0), energy(&only_guitar, 0));
        assert!(diff.abs() < 1.5, "guitare modifiée de {diff:.1} dB");
    }

    #[test]
    fn basses_centrees_conservees() {
        let bass = tone(60.0, 2.0);
        let pcm: Vec<f32> = bass.iter().flat_map(|b| [*b, *b]).collect();
        let out = remove_vocals(&pcm, &params(), |_| {});
        let d = db(energy(&out, 0), energy(&pcm, 0));
        assert!(d.abs() < 0.5, "basse modifiée de {d:.1} dB");
    }

    #[test]
    fn force_nulle_transparente() {
        let t = tone(440.0, 1.0);
        let pcm: Vec<f32> = t.iter().flat_map(|x| [*x, x * 0.3]).collect();
        let p = Params { strength: 0.0, ..params() };
        let out = remove_vocals(&pcm, &p, |_| {});
        let max_err = out.iter().zip(&pcm).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(max_err < 1e-4, "reconstruction imparfaite : {max_err}");
    }
}
