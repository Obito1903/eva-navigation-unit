//! How segments look.
//!
//! Colour is a property of the visualizer, not of the audio, so it lives here
//! rather than on [`Segment`]. A style reads the semantic values a segment
//! carries — position, activation, peak, time since hit — and returns a colour
//! and glow amount.

use super::field::Segment;
use super::instance::SegmentShape;
use super::model::Band;

/// One entry in a [`Gradient`].
#[derive(Debug, Clone, Copy)]
pub struct ColorStop {
    pub at: f32,
    pub color: [f32; 4],
}

/// Piecewise-linear colour ramp sampled by a [`GradientAxis`].
#[derive(Debug, Clone, Default)]
pub struct Gradient {
    stops: Vec<ColorStop>,
}

impl Gradient {
    /// Sorts stops on construction so `sample` can assume ascending order.
    pub fn new(mut stops: Vec<ColorStop>) -> Self {
        stops.sort_by(|a, b| a.at.total_cmp(&b.at));
        Self { stops }
    }

    /// Two-stop ramp, the common case.
    pub fn duotone(lo: [f32; 3], hi: [f32; 3]) -> Self {
        Self::new(vec![
            ColorStop { at: 0.0, color: [lo[0], lo[1], lo[2], 1.0] },
            ColorStop { at: 1.0, color: [hi[0], hi[1], hi[2], 1.0] },
        ])
    }

    pub fn sample(&self, t: f32) -> [f32; 4] {
        match self.stops.len() {
            0 => [1.0; 4],
            1 => self.stops[0].color,
            _ => {
                let t = t.clamp(self.stops[0].at, self.stops[self.stops.len() - 1].at);
                let hi = self
                    .stops
                    .iter()
                    .position(|s| s.at >= t)
                    .unwrap_or(self.stops.len() - 1)
                    .max(1);
                let (a, b) = (&self.stops[hi - 1], &self.stops[hi]);
                let span = b.at - a.at;
                let k = if span > 0.0 { (t - a.at) / span } else { 0.0 };
                lerp4(a.color, b.color, k)
            }
        }
    }
}

/// Which segment value drives the gradient lookup.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum GradientAxis {
    /// Position along the band. Reproduces the classic bottom-to-top ramp.
    #[default]
    SegT,
}

impl GradientAxis {
    fn value(self, seg: &Segment, _band: &Band) -> f32 {
        match self {
            Self::SegT => seg.seg_t,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Style {
    pub gradient: Gradient,
    pub axis: GradientAxis,
    /// Colour of a segment with zero activation. Keeps the full grid
    /// silhouette visible instead of leaving holes.
    pub unlit: [f32; 4],
    pub peak: [f32; 4],
    /// Glow applied to a fully lit segment.
    pub glow: f32,
    /// Extra brightness multiplier the instant a segment lights up.
    pub hit_flash: f32,
    /// How long the hit flash takes to fall off, in milliseconds.
    pub hit_flash_ms: f32,
    pub shape: SegmentShape,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            gradient: Gradient::duotone([0.0, 0.30, 0.42], [0.0, 0.88, 1.0]),
            axis: GradientAxis::SegT,
            unlit: [0.0, 0.28, 0.38, 0.24],
            peak: [0.55, 1.0, 1.0, 1.0],
            glow: 1.0,
            hit_flash: 0.0,
            hit_flash_ms: 80.0,
            shape: SegmentShape::Rect,
        }
    }
}

impl Style {
    /// Resolves a segment to its final colour and glow for this frame.
    pub fn shade(&self, seg: &Segment, band: &Band) -> ([f32; 4], f32) {
        if seg.is_peak {
            return (self.peak, self.glow);
        }
        if seg.activation <= 0.0 {
            return (self.unlit, 0.0);
        }

        let mut color = self.gradient.sample(self.axis.value(seg, band));
        // The trail fades by alpha, so a decaying segment dims without
        // shifting hue.
        color[3] *= seg.activation;

        let flash = self.hit_flash_factor(seg.hit_age);
        if flash > 0.0 {
            let boost = 1.0 + self.hit_flash * flash;
            for c in &mut color[..3] {
                *c = (*c * boost).min(1.0);
            }
        }

        (color, self.glow * seg.activation)
    }

    /// `1.0` at the instant of the hit, falling linearly to `0.0`.
    fn hit_flash_factor(&self, hit_age: f32) -> f32 {
        if self.hit_flash <= 0.0 || self.hit_flash_ms <= 0.0 {
            return 0.0;
        }
        (1.0 - hit_age / (self.hit_flash_ms * 1e-3)).clamp(0.0, 1.0)
    }
}

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut out = [0.0; 4];
    for i in 0..4 {
        out[i] = a[i] + (b[i] - a[i]) * t;
    }
    out
}

/// The four palettes carried over from the original VFD/ARC renderers.
pub fn builtin_style(theme_id: i32) -> Style {
    let (lo, hi, peak, dim): ([f32; 3], [f32; 3], [f32; 3], [f32; 3]) = match theme_id {
        // NERV
        1 => ([0.40, 0.02, 0.01], [1.00, 0.30, 0.05], [1.00, 0.70, 0.40], [0.40, 0.05, 0.02]),
        // MATRIX
        2 => ([0.00, 0.25, 0.04], [0.10, 1.00, 0.30], [0.60, 1.00, 0.70], [0.00, 0.30, 0.06]),
        // NEON
        3 => ([0.25, 0.00, 0.30], [0.00, 0.90, 1.00], [0.80, 0.80, 1.00], [0.15, 0.00, 0.20]),
        // VFD cyan phosphor
        _ => ([0.00, 0.30, 0.42], [0.00, 0.88, 1.00], [0.55, 1.00, 1.00], [0.00, 0.28, 0.38]),
    };
    Style {
        gradient: Gradient::duotone(lo, hi),
        axis: GradientAxis::SegT,
        unlit: [dim[0], dim[1], dim[2], 0.24],
        peak: [peak[0], peak[1], peak[2], 1.0],
        glow: 1.0,
        hit_flash: 0.0,
        hit_flash_ms: 80.0,
        shape: SegmentShape::Vfd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(activation: f32, seg_t: f32) -> Segment {
        Segment {
            seg_t,
            activation,
            lit: activation >= 1.0,
            hit_age: super::super::field::HIT_AGE_MAX,
            ..Segment::default()
        }
    }

    #[test]
    fn gradient_interpolates_between_stops() {
        let g = Gradient::duotone([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        assert_eq!(g.sample(0.0)[0], 0.0);
        assert_eq!(g.sample(1.0)[0], 1.0);
        assert!((g.sample(0.5)[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn gradient_clamps_outside_its_range() {
        let g = Gradient::duotone([0.2, 0.0, 0.0], [0.8, 0.0, 0.0]);
        assert_eq!(g.sample(-5.0)[0], 0.2);
        assert_eq!(g.sample(5.0)[0], 0.8);
    }

    #[test]
    fn gradient_accepts_unsorted_stops() {
        let g = Gradient::new(vec![
            ColorStop { at: 1.0, color: [1.0, 0.0, 0.0, 1.0] },
            ColorStop { at: 0.0, color: [0.0, 0.0, 0.0, 1.0] },
        ]);
        assert_eq!(g.sample(0.0)[0], 0.0);
        assert_eq!(g.sample(1.0)[0], 1.0);
    }

    #[test]
    fn empty_gradient_does_not_panic() {
        assert_eq!(Gradient::default().sample(0.5), [1.0; 4]);
    }

    #[test]
    fn peak_overrides_everything() {
        let style = builtin_style(0);
        let mut s = seg(0.0, 0.5);
        s.is_peak = true;
        assert_eq!(style.shade(&s, &Band::default()).0, style.peak);
    }

    #[test]
    fn dark_segments_use_the_unlit_colour() {
        let style = builtin_style(0);
        let (color, glow) = style.shade(&seg(0.0, 0.5), &Band::default());
        assert_eq!(color, style.unlit);
        assert_eq!(glow, 0.0);
    }

    #[test]
    fn trailing_segments_fade_by_alpha_not_hue() {
        let style = builtin_style(0);
        let full = style.shade(&seg(1.0, 0.5), &Band::default()).0;
        let half = style.shade(&seg(0.5, 0.5), &Band::default()).0;
        assert_eq!(full[..3], half[..3]);
        assert!((half[3] - full[3] * 0.5).abs() < 1e-6);
    }

    #[test]
    fn hit_flash_brightens_only_while_fresh() {
        let style = Style { hit_flash: 1.0, hit_flash_ms: 100.0, ..builtin_style(0) };
        let mut fresh = seg(1.0, 1.0);
        fresh.hit_age = 0.0;
        let mut stale = seg(1.0, 1.0);
        stale.hit_age = 0.2;
        let a = style.shade(&fresh, &Band::default()).0;
        let b = style.shade(&stale, &Band::default()).0;
        assert!(a[1] > b[1], "fresh hit should be brighter: {a:?} vs {b:?}");
    }

    #[test]
    fn hit_flash_is_off_by_default() {
        let style = builtin_style(0);
        let mut fresh = seg(1.0, 0.5);
        fresh.hit_age = 0.0;
        let mut stale = seg(1.0, 0.5);
        stale.hit_age = 1.0;
        assert_eq!(
            style.shade(&fresh, &Band::default()).0,
            style.shade(&stale, &Band::default()).0
        );
    }


    #[test]
    fn all_builtin_themes_are_distinct() {
        let s: Vec<[f32; 4]> = (0..4).map(|i| builtin_style(i).gradient.sample(1.0)).collect();
        for (i, a) in s.iter().enumerate() {
            for b in &s[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
