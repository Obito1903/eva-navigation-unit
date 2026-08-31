//! Backend-agnostic spectrum data model.
//!
//! Produced by [`crate::visualizer::SpectrumProcessor`] and consumed by the
//! segment field, layout and style layers. Deliberately contains no colours,
//! no geometry and no GPU types, so it can be unit-tested without a context.

/// One frequency band within a single frame.
// Frequency fields are the schema for frequency-driven styles in the
// declarative scene format; nothing reads them yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Band {
    pub index: usize,
    /// `index` normalised to `0..=1` across the band count.
    pub band_t: f32,
    pub lo_hz: f32,
    pub center_hz: f32,
    pub hi_hz: f32,
    /// Magnitude after gravity + integral smoothing, `0..=1`.
    pub level: f32,
    /// Peak-hold marker, `0..=1`.
    pub peak: f32,
    /// Magnitude before gravity/integral smoothing, `0..=1`.
    pub raw: f32,
}

/// Per-frame snapshot of the whole spectrum.
#[derive(Debug, Clone, Default)]
pub struct SpectrumFrame {
    pub bands: Vec<Band>,
    /// Seconds since the previous frame.
    pub dt: f32,
    /// Seconds since capture started; drives time-based animation.
    pub elapsed: f32,
}

impl SpectrumFrame {
    /// Builds a zeroed frame from band edge frequencies.
    ///
    /// `cut_off_hz` holds `bands + 1` strictly increasing edges, matching
    /// CAVA's band layout: band `n` spans `cut_off_hz[n]..cut_off_hz[n + 1]`.
    pub fn with_layout(cut_off_hz: &[f32]) -> Self {
        let n = cut_off_hz.len().saturating_sub(1);
        let bands = (0..n)
            .map(|i| {
                let lo_hz = cut_off_hz[i];
                let hi_hz = cut_off_hz[i + 1];
                Band {
                    index: i,
                    band_t: normalized_index(i, n),
                    lo_hz,
                    center_hz: (lo_hz * hi_hz).sqrt(),
                    hi_hz,
                    ..Band::default()
                }
            })
            .collect();
        Self { bands, dt: 0.0, elapsed: 0.0 }
    }

    pub fn len(&self) -> usize {
        self.bands.len()
    }

    // Companion to `len`, required by clippy::len_without_is_empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }
}

/// Maps `index` into `0..=1` over `count` items; a lone item sits at 0.
pub fn normalized_index(index: usize, count: usize) -> f32 {
    if count <= 1 {
        0.0
    } else {
        index as f32 / (count - 1) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_derives_band_metadata() {
        let frame = SpectrumFrame::with_layout(&[20.0, 200.0, 2_000.0, 20_000.0]);
        assert_eq!(frame.len(), 3);
        assert_eq!(frame.bands[0].lo_hz, 20.0);
        assert_eq!(frame.bands[2].hi_hz, 20_000.0);
        // Geometric centre, not arithmetic: log-spaced bands.
        assert!((frame.bands[0].center_hz - 63.245).abs() < 0.01);
    }

    #[test]
    fn band_t_spans_the_full_range() {
        let frame = SpectrumFrame::with_layout(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(frame.bands[0].band_t, 0.0);
        assert_eq!(frame.bands[3].band_t, 1.0);
    }

    #[test]
    fn single_band_does_not_divide_by_zero() {
        let frame = SpectrumFrame::with_layout(&[20.0, 20_000.0]);
        assert_eq!(frame.len(), 1);
        assert_eq!(frame.bands[0].band_t, 0.0);
    }

    #[test]
    fn empty_layout_yields_empty_frame() {
        assert!(SpectrumFrame::with_layout(&[]).is_empty());
        assert!(SpectrumFrame::with_layout(&[100.0]).is_empty());
    }
}
