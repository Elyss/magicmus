//! Séparation voix / musique par réseau de neurones MDX-Net (modèle UVR « Inst HQ 3 »),
//! exécuté directement en Rust avec ONNX Runtime : aucune installation de Python.
//!
//! Le modèle prend un spectrogramme stéréo [1, 4, dim_f, dim_t] (canaux : L réel,
//! L imaginaire, R réel, R imaginaire) et renvoie celui de l'accompagnement.
//! Le découpage en segments, la STFT et la STFT inverse reproduisent ceux d'UVR
//! (`torch.stft(center=True)`, fenêtre de Hann périodique).

use anyhow::{anyhow, Context, Result};
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::f32::consts::PI;
use std::path::Path;
use std::sync::Arc;

/// Paramètres du modèle UVR-MDX-NET-Inst_HQ_3 (fichier model_data.json d'UVR).
#[derive(Clone, Copy)]
pub struct MdxParams {
    pub n_fft: usize,
    pub hop: usize,
    pub dim_f: usize,
    pub dim_t: usize,
    pub compensate: f32,
}

pub const INST_HQ_3: MdxParams =
    MdxParams { n_fft: 6144, hop: 1024, dim_f: 3072, dim_t: 256, compensate: 1.022 };

/// Sépare l'accompagnement d'un signal stéréo entrelacé (44,1 kHz).
/// `runtime` est le chemin de `onnxruntime.dll`.
pub fn separate(
    runtime: &Path,
    model: &Path,
    pcm: &[f32],
    progress: &mut dyn FnMut(f32),
) -> Result<Vec<f32>> {
    let p = INST_HQ_3;
    let env = ort::init_from(runtime)
        .map_err(|e| anyhow!("{e}"))
        .with_context(|| format!("chargement de {}", runtime.display()))?
        .with_name("karaoke")
        .build()
        .map_err(|e| anyhow!("{e}"))
        .context("initialisation d'ONNX Runtime")?;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut builder = ort::session::Session::builder(&env)
        .map_err(|e| anyhow!("{e}"))?
        .with_intra_threads(threads)
        .map_err(|e| anyhow!("{e}"))?;
    let mut session: ort::session::Session = builder
        .commit_from_file(model)
        .map_err(|e| anyhow!("{e}"))
        .with_context(|| format!("chargement du modèle {}", model.display()))?;

    let shape = [1usize, 4, p.dim_f, p.dim_t];
    separate_with(pcm, &p, progress, |spec| {
        let input = ort::value::Tensor::from_array((shape, spec.to_vec())).map_err(|e| anyhow!("{e}"))?;
        let outputs = session.run(ort::inputs![input]).map_err(|e| anyhow!("{e}"))?;
        let (_, out) = outputs[0].try_extract_tensor::<f32>().map_err(|e| anyhow!("{e}"))?;
        if out.len() != spec.len() {
            return Err(anyhow!("sortie du modèle inattendue ({} valeurs)", out.len()));
        }
        spec.copy_from_slice(out);
        Ok(())
    })
}

/// Découpe, STFT, `model` (modifie le spectrogramme en place), STFT inverse, recollage.
pub fn separate_with(
    pcm: &[f32],
    p: &MdxParams,
    progress: &mut dyn FnMut(f32),
    mut model: impl FnMut(&mut [f32]) -> Result<()>,
) -> Result<Vec<f32>> {
    let n = pcm.len() / 2;
    let trim = p.n_fft / 2;
    let chunk = p.hop * (p.dim_t - 1);
    let seg = chunk - 2 * trim; // échantillons utiles par segment
    let pad = seg - n % seg;

    // Signal complété : `trim` zéros, signal, `pad` zéros, `trim` zéros.
    let total = trim + n + pad + trim;
    let mut mix = [vec![0f32; total], vec![0f32; total]];
    for (i, f) in pcm.chunks_exact(2).enumerate() {
        mix[0][trim + i] = f[0];
        mix[1][trim + i] = f[1];
    }

    let stft = Stft::new(p.n_fft, p.hop);
    let mut out = [Vec::with_capacity(n + pad), Vec::with_capacity(n + pad)];
    let mut spec = vec![0f32; 4 * p.dim_f * p.dim_t];
    let segments = (n + pad) / seg;

    for (s, start) in (0..n + pad).step_by(seg).enumerate() {
        for ch in 0..2 {
            stft.forward(&mix[ch][start..start + chunk], p, ch, &mut spec);
        }
        model(&mut spec)?;
        for ch in 0..2 {
            let wave = stft.inverse(&spec, p, ch, chunk);
            out[ch].extend_from_slice(&wave[trim..chunk - trim]);
        }
        progress((s + 1) as f32 / segments as f32);
    }

    let mut res = Vec::with_capacity(n * 2);
    for i in 0..n {
        res.push(out[0][i] * p.compensate);
        res.push(out[1][i] * p.compensate);
    }
    Ok(res)
}

struct Stft {
    n_fft: usize,
    hop: usize,
    window: Vec<f32>,
    fwd: Arc<dyn Fft<f32>>,
    inv: Arc<dyn Fft<f32>>,
}

impl Stft {
    fn new(n_fft: usize, hop: usize) -> Self {
        let mut planner = FftPlanner::new();
        Self {
            n_fft,
            hop,
            // Hann périodique (torch.hann_window(periodic=True)).
            window: (0..n_fft).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / n_fft as f32).cos()).collect(),
            fwd: planner.plan_fft_forward(n_fft),
            inv: planner.plan_fft_inverse(n_fft),
        }
    }

    /// STFT d'un canal (centrée, remplissage par réflexion) écrite dans `spec`
    /// aux canaux `2·ch` (réel) et `2·ch + 1` (imaginaire).
    fn forward(&self, x: &[f32], p: &MdxParams, ch: usize, spec: &mut [f32]) {
        let half = self.n_fft / 2;
        let len = x.len() as isize;
        let reflect = |i: isize| -> f32 {
            let j = if i < 0 { -i } else if i >= len { 2 * (len - 1) - i } else { i };
            x[j as usize]
        };
        let mut buf = vec![Complex32::default(); self.n_fft];
        let (re_off, im_off) = ((2 * ch) * p.dim_f * p.dim_t, (2 * ch + 1) * p.dim_f * p.dim_t);
        for t in 0..p.dim_t {
            let start = (t * self.hop) as isize - half as isize;
            for (k, b) in buf.iter_mut().enumerate() {
                *b = Complex32::new(reflect(start + k as isize) * self.window[k], 0.0);
            }
            self.fwd.process(&mut buf);
            for f in 0..p.dim_f {
                spec[re_off + f * p.dim_t + t] = buf[f].re;
                spec[im_off + f * p.dim_t + t] = buf[f].im;
            }
        }
    }

    /// STFT inverse d'un canal (bandes au-delà de `dim_f` mises à zéro), longueur `len`.
    fn inverse(&self, spec: &[f32], p: &MdxParams, ch: usize, len: usize) -> Vec<f32> {
        let half = self.n_fft / 2;
        let full = len + self.n_fft;
        let mut acc = vec![0f32; full];
        let mut wsum = vec![0f32; full];
        let mut buf = vec![Complex32::default(); self.n_fft];
        let (re_off, im_off) = ((2 * ch) * p.dim_f * p.dim_t, (2 * ch + 1) * p.dim_f * p.dim_t);
        let inv_n = 1.0 / self.n_fft as f32;
        for t in 0..p.dim_t {
            buf.fill(Complex32::default());
            for f in 0..p.dim_f.min(half + 1) {
                let mut c = Complex32::new(spec[re_off + f * p.dim_t + t], spec[im_off + f * p.dim_t + t]);
                if f == 0 || f == half {
                    c.im = 0.0; // composantes réelles pour un signal réel
                }
                buf[f] = c;
                if f != 0 && f != half {
                    buf[self.n_fft - f] = c.conj();
                }
            }
            self.inv.process(&mut buf);
            let pos = t * self.hop;
            for k in 0..self.n_fft {
                let w = self.window[k];
                acc[pos + k] += buf[k].re * inv_n * w;
                wsum[pos + k] += w * w;
            }
        }
        (half..half + len)
            .map(|i| if wsum[i] > 1e-11 { acc[i] / wsum[i] } else { 0.0 })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Avec un « modèle » identité, le signal doit être reconstruit à l'identique.
    #[test]
    fn reconstruction_identite() {
        let sr = 44_100.0;
        let n = 300_000; // plus d'un segment
        let pcm: Vec<f32> = (0..n)
            .flat_map(|i| {
                let t = i as f32 / sr;
                [(2.0 * PI * 220.0 * t).sin() * 0.5, (2.0 * PI * 3000.0 * t).sin() * 0.3]
            })
            .collect();
        let p = MdxParams { compensate: 1.0, ..INST_HQ_3 };
        let mut calls = 0;
        let out = separate_with(&pcm, &p, &mut |_| {}, |_| {
            calls += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(out.len(), pcm.len());
        assert_eq!(calls, 2);
        let err = out.iter().zip(&pcm).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(err < 1e-3, "erreur de reconstruction {err}");
    }
}
