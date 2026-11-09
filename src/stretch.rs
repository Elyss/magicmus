//! Changement de tonalité et de vitesse indépendants (Signalsmith Stretch, C++ intégré).
//! Liaisons écrites à la main d'après `vendor/signalsmith/wrapper.h`.

use std::ffi::c_int;

#[repr(C)]
struct RawStretch {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn signalsmith_stretch_create_preset_default(channel_count: c_int, sample_rate: f32) -> *mut RawStretch;
    fn signalsmith_stretch_destroy(handle: *mut RawStretch);
    fn signalsmith_stretch_reset(handle: *mut RawStretch);
    fn signalsmith_stretch_input_latency(handle: *mut RawStretch) -> usize;
    fn signalsmith_stretch_output_latency(handle: *mut RawStretch) -> usize;
    fn signalsmith_stretch_set_transpose_factor_semitones(handle: *mut RawStretch, semitones: f32, tonality_limit: f32);
    fn signalsmith_stretch_process(
        handle: *mut RawStretch,
        input: *mut f32,
        input_length: usize,
        output: *mut f32,
        output_length: usize,
    );
    fn signalsmith_stretch_exact(
        handle: *mut RawStretch,
        input: *mut f32,
        input_length: usize,
        output: *mut f32,
        output_length: usize,
    ) -> bool;
}

/// Processeur stéréo (ou N canaux) entrelacé.
pub struct Stretch {
    raw: *mut RawStretch,
    channels: usize,
}

// SAFETY : l'objet C++ n'a pas d'état partagé ; il n'est utilisé que par un thread à la fois.
unsafe impl Send for Stretch {}

impl Stretch {
    pub fn new(channels: usize, sample_rate: u32) -> Self {
        let raw = unsafe { signalsmith_stretch_create_preset_default(channels as c_int, sample_rate as f32) };
        assert!(!raw.is_null(), "création de Signalsmith Stretch impossible");
        Self { raw, channels }
    }

    pub fn reset(&mut self) {
        unsafe { signalsmith_stretch_reset(self.raw) }
    }

    /// Latence totale (entrée, sortie) en trames.
    pub fn latency(&self) -> (usize, usize) {
        unsafe { (signalsmith_stretch_input_latency(self.raw), signalsmith_stretch_output_latency(self.raw)) }
    }

    /// Transposition en demi-tons (sans changer la vitesse).
    pub fn set_semitones(&mut self, semitones: f32) {
        unsafe { signalsmith_stretch_set_transpose_factor_semitones(self.raw, semitones, 0.0) }
    }

    /// Traite un bloc : la durée de sortie peut différer de celle d'entrée (étirement).
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len() % self.channels, 0);
        debug_assert_eq!(output.len() % self.channels, 0);
        unsafe {
            signalsmith_stretch_process(
                self.raw,
                input.as_ptr() as *mut f32, // non modifié par la bibliothèque
                input.len() / self.channels,
                output.as_mut_ptr(),
                output.len() / self.channels,
            )
        }
    }

    /// Traite un fichier entier d'un coup (latence compensée).
    pub fn exact(&mut self, input: &[f32], output: &mut [f32]) -> bool {
        unsafe {
            signalsmith_stretch_exact(
                self.raw,
                input.as_ptr() as *mut f32,
                input.len() / self.channels,
                output.as_mut_ptr(),
                output.len() / self.channels,
            )
        }
    }
}

impl Drop for Stretch {
    fn drop(&mut self) {
        unsafe { signalsmith_stretch_destroy(self.raw) }
    }
}

/// Applique vitesse (`tempo`, 1.0 = normal) et tonalité (demi-tons) à un signal entier.
pub fn render(pcm: &[f32], channels: usize, sample_rate: u32, tempo: f32, semitones: f32) -> Vec<f32> {
    if (tempo - 1.0).abs() < 1e-4 && semitones.abs() < 1e-4 {
        return pcm.to_vec();
    }
    let frames_in = pcm.len() / channels;
    let frames_out = ((frames_in as f64) / tempo as f64).round() as usize;
    let mut out = vec![0f32; frames_out * channels];
    let mut s = Stretch::new(channels, sample_rate);
    s.set_semitones(semitones);
    s.exact(pcm, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// Fréquence dominante estimée par passages à zéro.
    fn freq(x: &[f32], sr: f32) -> f32 {
        let mono: Vec<f32> = x.chunks_exact(2).map(|f| f[0]).collect();
        let m = &mono[mono.len() / 4..mono.len() * 3 / 4];
        let crossings = m.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        crossings as f32 * sr / m.len() as f32
    }

    #[test]
    fn tonalite_et_vitesse() {
        let sr = 44_100;
        let pcm: Vec<f32> = (0..sr * 2)
            .flat_map(|i| {
                let v = (2.0 * PI * 440.0 * i as f32 / sr as f32).sin() * 0.5;
                [v, v]
            })
            .collect();
        // +12 demi-tons : 880 Hz, même durée.
        let up = render(&pcm, 2, sr as u32, 1.0, 12.0);
        assert_eq!(up.len(), pcm.len());
        let f = freq(&up, sr as f32);
        assert!((f - 880.0).abs() < 15.0, "fréquence {f}");
        // Vitesse ×1.25 : durée / 1.25, hauteur inchangée.
        let fast = render(&pcm, 2, sr as u32, 1.25, 0.0);
        assert_eq!(fast.len() / 2, (sr as f32 * 2.0 / 1.25).round() as usize);
        let f = freq(&fast, sr as f32);
        assert!((f - 440.0).abs() < 10.0, "fréquence {f}");
    }
}
