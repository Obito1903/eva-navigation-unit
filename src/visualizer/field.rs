//! Per-segment state field.
//!
//! Turns the band levels in a [`SpectrumFrame`] into individually-stateful
//! segments. Each segment owns its own fade-out trail and tracks how long ago
//! it last lit up, which is what lets a style render comet tails and hit
//! flashes that band-level gravity alone cannot express.
//!
//! Pure CPU and free of GPU types, so it is unit-testable against synthetic
//! frames.

use super::model::{normalized_index, SpectrumFrame};

/// Upper bound on [`Segment::hit_age`], keeping it finite for style
/// expressions that divide by or interpolate over it.
pub const HIT_AGE_MAX: f32 = 10.0;

/// How a segment's lighting threshold is distributed along a band.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum ThresholdCurve {
    /// Evenly spaced: segment `i` of `n` lights at `(i + 1) / n`.
    #[default]
    Linear,
}

impl ThresholdCurve {
    fn threshold(self, index: usize, count: usize) -> f32 {
        let linear = (index + 1) as f32 / count as f32;
        match self {
            Self::Linear => linear,
        }
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct SegmentFieldConfig {
    /// Segments per band.
    pub seg_count: usize,
    /// Time constant of the per-segment fade-out trail, in milliseconds.
    /// Zero disables trailing (segments snap dark).
    pub decay_ms: f32,
    pub curve: ThresholdCurve,
}

impl Default for SegmentFieldConfig {
    fn default() -> Self {
        Self { seg_count: 32, decay_ms: 120.0, curve: ThresholdCurve::Linear }
    }
}

/// One addressable element of a visualizer.
///
/// Carries semantic values only. Mapping these to colour is the style layer's
/// job, because colour depends on the visualizer, not on the audio.
#[derive(Debug, Clone, Copy, Default)]
pub struct Segment {
    pub index: usize,
    /// `index` normalised to `0..=1` along the band axis.
    pub seg_t: f32,
    /// Band level at or above which this segment lights.
    pub threshold: f32,
    /// Whether the current band level drives this segment right now.
    pub lit: bool,
    /// `0..=1` including the fade-out trail; `1.0` while lit.
    pub activation: f32,
    /// Seconds since this segment last transitioned dark to lit, clamped to
    /// [`HIT_AGE_MAX`]. Starts at the maximum so untouched segments never
    /// read as freshly hit.
    pub hit_age: f32,
    /// This segment holds the band's peak marker.
    pub is_peak: bool,
}

/// Dense `bands * seg_count` grid of [`Segment`]s in row-major band order.
pub struct SegmentField {
    cfg: SegmentFieldConfig,
    band_count: usize,
    segments: Vec<Segment>,
}

impl SegmentField {
    pub fn new(band_count: usize, cfg: SegmentFieldConfig) -> Self {
        let seg_count = cfg.seg_count.max(1);
        let cfg = SegmentFieldConfig { seg_count, ..cfg };
        let segments = (0..band_count * seg_count)
            .map(|flat| {
                let i = flat % seg_count;
                Segment {
                    index: i,
                    seg_t: normalized_index(i, seg_count),
                    threshold: cfg.curve.threshold(i, seg_count),
                    hit_age: HIT_AGE_MAX,
                    ..Segment::default()
                }
            })
            .collect();
        Self { cfg, band_count, segments }
    }

    pub fn band_count(&self) -> usize {
        self.band_count
    }

    pub fn seg_count(&self) -> usize {
        self.cfg.seg_count
    }

    /// Segments of one band, ordered from the base of the band outwards.
    pub fn band(&self, index: usize) -> &[Segment] {
        let n = self.cfg.seg_count;
        &self.segments[index * n..(index + 1) * n]
    }

    /// Advances every segment against the frame's band levels.
    ///
    /// Silently ignores bands beyond the field's capacity, so a frame whose
    /// band count drifted from the field's cannot panic mid-render.
    pub fn update(&mut self, frame: &SpectrumFrame) {
        let n = self.cfg.seg_count;
        let dt = frame.dt.max(0.0);
        // exp decay per frame; tau <= 0 means snap dark immediately.
        let decay = if self.cfg.decay_ms > 0.0 {
            (-dt / (self.cfg.decay_ms * 1e-3)).exp()
        } else {
            0.0
        };

        for band in frame.bands.iter().take(self.band_count) {
            let peak_index = peak_segment(band.peak, n);
            for seg in &mut self.segments[band.index * n..(band.index + 1) * n] {
                let lit = band.level >= seg.threshold;
                seg.hit_age = (seg.hit_age + dt).min(HIT_AGE_MAX);
                if lit && !seg.lit {
                    seg.hit_age = 0.0;
                }
                seg.lit = lit;
                seg.activation = if lit { 1.0 } else { seg.activation * decay };
                seg.is_peak = peak_index == Some(seg.index);
            }
        }
    }
}

/// Segment index a peak level lands on, or `None` when there is no peak.
fn peak_segment(peak: f32, seg_count: usize) -> Option<usize> {
    if peak <= 0.0 {
        return None;
    }
    let scaled = (peak * seg_count as f32).ceil() as usize;
    Some(scaled.clamp(1, seg_count) - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visualizer::model::Band;

    /// Frame with every band driven to `level`, peak matching.
    fn frame_at(level: f32, band_count: usize, dt: f32) -> SpectrumFrame {
        let bands = (0..band_count)
            .map(|index| Band {
                index,
                band_t: normalized_index(index, band_count),
                level,
                peak: level,
                raw: level,
                ..Band::default()
            })
            .collect();
        SpectrumFrame { bands, dt, elapsed: 0.0 }
    }

    fn field(seg_count: usize, decay_ms: f32) -> SegmentField {
        SegmentField::new(
            2,
            SegmentFieldConfig { seg_count, decay_ms, curve: ThresholdCurve::Linear },
        )
    }

    #[test]
    fn thresholds_are_strictly_increasing() {
        let f = field(10, 0.0);
        let thresholds: Vec<f32> = f.band(0).iter().map(|s| s.threshold).collect();
        assert!(thresholds.windows(2).all(|w| w[1] > w[0]));
        assert!((thresholds[9] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn level_lights_exactly_the_segments_below_it() {
        let mut f = field(10, 0.0);
        f.update(&frame_at(0.5, 2, 0.016));
        let lit = f.band(0).iter().filter(|s| s.lit).count();
        assert_eq!(lit, 5);
        assert!(f.band(0)[4].lit);
        assert!(!f.band(0)[5].lit);
    }

    #[test]
    fn activation_trails_after_the_level_drops() {
        let mut f = field(10, 100.0);
        f.update(&frame_at(1.0, 2, 0.016));
        assert_eq!(f.band(0)[9].activation, 1.0);

        // One 100 ms step of silence decays a full segment to 1/e.
        f.update(&frame_at(0.0, 2, 0.1));
        let a = f.band(0)[9].activation;
        assert!(!f.band(0)[9].lit);
        assert!((a - std::f32::consts::E.recip()).abs() < 1e-3, "activation was {a}");
    }

    #[test]
    fn zero_decay_snaps_dark() {
        let mut f = field(10, 0.0);
        f.update(&frame_at(1.0, 2, 0.016));
        f.update(&frame_at(0.0, 2, 0.016));
        assert_eq!(f.band(0)[9].activation, 0.0);
    }

    #[test]
    fn hit_age_resets_only_on_the_dark_to_lit_edge() {
        let mut f = field(10, 0.0);
        assert_eq!(f.band(0)[0].hit_age, HIT_AGE_MAX);

        f.update(&frame_at(1.0, 2, 0.5));
        assert_eq!(f.band(0)[0].hit_age, 0.0);

        // Still lit: the age keeps running rather than re-triggering.
        f.update(&frame_at(1.0, 2, 0.25));
        assert_eq!(f.band(0)[0].hit_age, 0.25);

        // Dark, then lit again: fresh hit.
        f.update(&frame_at(0.0, 2, 0.25));
        f.update(&frame_at(1.0, 2, 0.25));
        assert_eq!(f.band(0)[0].hit_age, 0.0);
    }

    #[test]
    fn hit_age_is_bounded() {
        let mut f = field(4, 0.0);
        for _ in 0..10 {
            f.update(&frame_at(0.0, 2, 5.0));
        }
        assert_eq!(f.band(0)[0].hit_age, HIT_AGE_MAX);
    }

    #[test]
    fn peak_marks_one_segment_per_band() {
        let mut f = field(10, 0.0);
        f.update(&frame_at(0.55, 2, 0.016));
        let peaks: Vec<usize> =
            f.band(0).iter().filter(|s| s.is_peak).map(|s| s.index).collect();
        assert_eq!(peaks, vec![5]);
    }

    #[test]
    fn silence_marks_no_peak() {
        let mut f = field(10, 0.0);
        f.update(&frame_at(0.0, 2, 0.016));
        assert!(f.band(0).iter().all(|s| !s.is_peak));
    }

    #[test]
    fn full_level_marks_the_last_segment() {
        let mut f = field(10, 0.0);
        f.update(&frame_at(1.0, 2, 0.016));
        assert!(f.band(0)[9].is_peak);
    }


    #[test]
    fn extra_bands_in_the_frame_are_ignored() {
        let mut f = field(10, 0.0);
        f.update(&frame_at(1.0, 64, 0.016));
        assert_eq!(f.band_count(), 2);
    }
}
