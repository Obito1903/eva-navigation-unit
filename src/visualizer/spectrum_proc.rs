//! CAVA-style spectrum processor — runs entirely on the render thread.
//!
//! Called once per frame from `BeforeRendering`. Drains audio samples from the
//! capture ring buffer, slides the Hann-windowed FFT analysis window forward,
//! maps FFT bins to bands with CAVA's logarithmic EQ weighting, then applies
//! gravity fall-off, leaky-integral smoothing, and autosens — all calibrated
//! to the actual display frame rate.
//!
//! Reference: <https://github.com/karlstav/cava/blob/master/cavacore.c>

use std::f32::consts::PI;
use std::time::Instant;

use rustfft::{num_complex::Complex, FftPlanner};

use crate::config::VizConfig;
use crate::spectrum::AudioConsumer;

pub struct SpectrumProcessor {
    // FFT
    fft:      std::sync::Arc<dyn rustfft::Fft<f32>>,
    fft_buf:  Vec<Complex<f32>>,
    scratch:  Vec<Complex<f32>>,
    hann:     Vec<f32>,
    fft_size: usize,

    /// Sliding input window; [0]=newest sample, [N-1]=oldest (CAVA layout).
    window: Vec<f32>,

    // Band configuration
    n_bands:  usize,
    band_lo:  Vec<usize>,
    band_hi:  Vec<usize>,
    eq:       Vec<f32>,

    // CAVA per-band smoothing state
    cava_peak: Vec<f32>,
    cava_fall: Vec<f32>,
    cava_mem:  Vec<f32>,
    prev_out:  Vec<f32>,
    /// Fast-attack / slower-release EMA applied to raw FFT values before the
    /// gravity trigger.  Prevents per-frame FFT noise from resetting the fall
    /// counter (which would cause jitter on rapid volume drops).
    prev_raw:  Vec<f32>,

    // Autosens
    sens:      f32,
    sens_init: bool,

    // Config knobs
    noise_reduction: f32,
    gravity_scale:   f32,

    // Outputs (read by renderers after each `process()` call)
    pub bands: Vec<f32>,
    pub peaks: Vec<f32>,

    consumer: AudioConsumer,
    last:     Instant,
}

impl SpectrumProcessor {
    pub fn new(consumer: AudioConsumer, cfg: &VizConfig) -> Self {
        let fft_size = cfg.fft_size;
        let n_bands  = cfg.bands;

        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(fft_size);
        let scratch_len = fft.get_inplace_scratch_len();

        let hann: Vec<f32> = (0..fft_size)
            .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / (fft_size - 1) as f32).cos()))
            .collect();

        // ── CAVA logarithmic band mapping ─────────────────────────────────
        // frequency_constant = log10(lower/upper) / (1/(N+1) - 1)
        // cut_off[n] = upper * 10^( fc * ((n+1)/(N+1) - 1) )
        let lower  = cfg.freq_min.max(1.0);
        let upper  = cfg.freq_max.min(24_000.0);
        let bin_hz = 48_000.0_f32 / fft_size as f32;
        let fc     = (lower / upper).log10()
            / (1.0 / (n_bands as f32 + 1.0) - 1.0);

        let mut cut_off = Vec::with_capacity(n_bands + 1);
        for n in 0..=(n_bands) {
            let coeff = fc * ((n as f32 + 1.0) / (n_bands as f32 + 1.0) - 1.0);
            let freq  = upper * 10.0_f32.powf(coeff);
            // Strictly increasing (CAVA enforces this too).
            let freq = if n > 0 && freq <= cut_off[n - 1] {
                cut_off[n - 1] + bin_hz
            } else {
                freq
            };
            cut_off.push(freq.min(upper));
        }

        let mut band_lo = vec![0usize; n_bands];
        let mut band_hi = vec![0usize; n_bands];
        for n in 0..n_bands {
            let lo = (cut_off[n]     / bin_hz) as usize;
            let hi = (cut_off[n + 1] / bin_hz) as usize;
            band_lo[n] = lo.max(1).min(fft_size / 2);
            band_hi[n] = hi.max(band_lo[n]).min(fft_size / 2);
        }
        // Non-overlapping adjacent bands (CAVA: upper[n] = lower[n+1] - 1).
        for n in 0..n_bands.saturating_sub(1) {
            if band_hi[n] >= band_lo[n + 1] {
                band_hi[n] = band_lo[n + 1].saturating_sub(1).max(band_lo[n]);
            }
        }

        // ── CAVA EQ ───────────────────────────────────────────────────────
        // eq[n] = (1/2^28) * cut_off[n+1]^0.85 / (log2(N) * bin_count)
        // Scaled for f32 input vs CAVA's int16: 2^28/32768 = 2^13 = 8192.
        let fft_log2 = (fft_size as f32).log2();
        let eq: Vec<f32> = (0..n_bands).map(|n| {
            let bin_count = (band_hi[n] - band_lo[n] + 1) as f32;
            let f_upper   = cut_off[n + 1].max(1.0);
            f_upper.powf(0.85) / (8192.0 * fft_log2 * bin_count)
        }).collect();

        Self {
            fft,
            fft_buf:  vec![Complex::ZERO; fft_size],
            scratch:  vec![Complex::ZERO; scratch_len],
            hann,
            fft_size,
            window:   vec![0.0; fft_size],
            n_bands,
            band_lo,
            band_hi,
            eq,
            cava_peak: vec![0.0; n_bands],
            cava_fall: vec![0.0; n_bands],
            cava_mem:  vec![0.0; n_bands],
            prev_out:  vec![0.0; n_bands],
            sens:      1.0,
            sens_init: true,
            noise_reduction: cfg.noise_reduction,
            gravity_scale:   cfg.gravity,
            bands: vec![0.0; n_bands],
            peaks: vec![0.0; n_bands],
            prev_raw: vec![0.0; n_bands],
            consumer,
            last: Instant::now(),
        }
    }

    /// Run one frame of the CAVA pipeline.  Call once per rendered frame from
    /// `BeforeRendering`.  Results are in `self.bands` and `self.peaks`.
    pub fn process(&mut self) {
        let dt = self.last.elapsed().as_secs_f32().clamp(1e-4, 0.1);
        self.last = Instant::now();

        // ── Drain ring buffer, slide analysis window ──────────────────────
        // CAVA drains all pending audio each display frame and slides the
        // FFT window (newest at [0], oldest at [N-1]).
        use ringbuf::traits::Consumer;
        use ringbuf::traits::Observer;
        let available = self.consumer.occupied_len();
        let mut tmp   = vec![0.0f32; available];
        let n         = self.consumer.pop_slice(&mut tmp);

        let silence;
        if n >= 2 {
            silence = false;
            let n_frames = n / 2; // stereo → mono
            let shift    = n_frames.min(self.fft_size);
            // Shift old samples toward the back.
            self.window.rotate_right(shift);
            // Fill front with the newest `shift` mono-downmixed frames,
            // reversed so index 0 = newest (CAVA's convention).
            let start = if n_frames > self.fft_size { n_frames - self.fft_size } else { 0 };
            for i in 0..shift {
                let src = start + (shift - 1 - i);
                let l = tmp.get(src * 2    ).copied().unwrap_or(0.0);
                let r = tmp.get(src * 2 + 1).copied().unwrap_or(0.0);
                self.window[i] = (l + r) * 0.5;
            }
        } else {
            silence = self.window.iter().all(|&s| s.abs() < 1e-6);
        }

        // ── Hann-windowed FFT ─────────────────────────────────────────────
        // window[0]=newest; FFT expects oldest-first.  Hann is symmetric so
        // reversing does not change the window shape.
        for i in 0..self.fft_size {
            let s = self.window[self.fft_size - 1 - i];
            self.fft_buf[i] = Complex::new(s * self.hann[i], 0.0);
        }
        self.fft.process_with_scratch(&mut self.fft_buf, &mut self.scratch);

        // ── Band extraction: Σ|FFT| per band × EQ × sens ───────────
        let mut raw = vec![0.0f32; self.n_bands];
        for n in 0..self.n_bands {
            let sum: f32 = self.fft_buf[self.band_lo[n]..=self.band_hi[n]]
                .iter().map(|c| c.norm()).sum();
            raw[n] = (sum * self.eq[n] * self.sens).max(0.0);
        }

        // ── Pre-smooth raw values for the gravity trigger ──────────────
        // Per-frame FFT output fluctuates due to window-phase changes even for
        // a steady signal.  Without smoothing, noise spikes on the falling edge
        // reset cava_fall to 0 and cava_peak to a high value, producing visible
        // steps.  Asymmetric time constants: fast attack to not miss peaks;
        // slower release to damp transient spikes during the fall.
        let alpha_rise = 1.0 - (-dt / 0.008_f32).exp(); // τ = 8 ms
        let alpha_fall = 1.0 - (-dt / 0.035_f32).exp(); // τ = 35 ms
        for n in 0..self.n_bands {
            let target = raw[n];
            let alpha = if target > self.prev_raw[n] { alpha_rise } else { alpha_fall };
            self.prev_raw[n] += (target - self.prev_raw[n]) * alpha;
            raw[n] = self.prev_raw[n];
        }

        // ── CAVA smoothing ────────────────────────────────────────────────
        let fps           = (1.0 / dt).clamp(20.0, 200.0);
        let framerate_mod = 66.0 / fps;

        let nr = if self.noise_reduction > 0.0 {
            self.noise_reduction.clamp(0.1, 0.99)
        } else {
            // CAVA Android calibration: pow(fps/130, 0.75)
            (fps / 130.0_f32).powf(0.75).clamp(0.1, 0.99)
        };

        let gravity_mod    = framerate_mod.powf(2.5) * 2.0 / nr * self.gravity_scale;
        let integral_factor = nr / framerate_mod.powf(0.1);
        let alpha_dot_fall  = 1.0 - (-dt / 0.80_f32).exp();

        let mut overshoot = false;

        for n in 0..self.n_bands {
            // Gravity: parabolic fall when bar is descending.
            let mut out = if raw[n] < self.prev_out[n] {
                let v = self.cava_peak[n]
                    * (1.0 - self.cava_fall[n] * self.cava_fall[n] * gravity_mod);
                self.cava_fall[n] += 0.028;
                v.max(0.0)
            } else {
                self.cava_peak[n] = raw[n];
                self.cava_fall[n] = 0.0;
                raw[n]
            };
            self.prev_out[n] = out;

            // Integral (leaky integrator): "heavy" bar feel.
            out = self.cava_mem[n] * integral_factor + out;
            let final_out   = out.clamp(0.0, 1.0);
            self.cava_mem[n] = final_out; // clamp prevents runaway

            if final_out >= 1.0 { overshoot = true; }
            self.bands[n] = final_out;

            // Floating dot: instant rise, slow fall.
            if final_out >= self.peaks[n] {
                self.peaks[n] = final_out;
            } else {
                self.peaks[n] = (self.peaks[n] * (1.0 - alpha_dot_fall)).max(0.0);
            }
        }

        // Autosens: back off fast on clip; ramp up slowly otherwise.
        if overshoot {
            self.sens     *= 1.0 - 0.02 * framerate_mod;
            self.sens_init = false;
        } else if !silence {
            self.sens *= 1.0 + 0.001 * framerate_mod;
            if self.sens_init { self.sens *= 1.0 + 0.1 * framerate_mod; }
        }
        self.sens = self.sens.clamp(0.01, 50.0);
    }
}
