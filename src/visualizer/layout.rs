//! Where segments sit on screen.
//!
//! A [`Layout`] maps `(band index, segment index)` to a quad in normalised
//! viewport space. This is the only layer that knows about geometry, so a new
//! visualizer shape is a new [`Layout`] variant and nothing else.
//!
//! Normalised space is `(0, 0)` top-left to `(1, 1)` bottom-right. It is not
//! square, so anything radial divides x offsets by [`LayoutCtx::aspect`] to
//! stay circular.

use super::field::Segment;
use super::model::Band;

/// Per-frame context handed to a layout.
#[derive(Debug, Clone, Copy)]
pub struct LayoutCtx {
    pub aspect: f32,
    pub band_count: usize,
    pub seg_count: usize,
    // Animation clock for time-driven layouts; none exist yet.
    #[allow(dead_code)]
    pub elapsed: f32,
}

/// Quad produced for one segment. Units match [`Viewport`]-relative
/// [`super::instance::SegmentInstance`]: `center` is normalised per axis,
/// `half_size` is normalised to height on both axes.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub center: [f32; 2],
    pub half_size: [f32; 2],
    pub rotation: f32,
}

/// Which edge bars grow from.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum Baseline {
    #[default]
    Bottom,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum Layout {
    /// Vertical columns, one per band.
    Columns {
        /// Horizontal gap as a fraction of the column slot, `0..1`.
        bar_gap: f32,
        /// Vertical gap as a fraction of the segment slot, `0..1`.
        seg_gap: f32,
        baseline: Baseline,
    },
}

impl Default for Layout {
    fn default() -> Self {
        Self::Columns { bar_gap: 0.15, seg_gap: 0.25, baseline: Baseline::Bottom }
    }
}

impl Layout {
    pub fn place(&self, band: &Band, seg: &Segment, ctx: &LayoutCtx) -> Placement {
        match *self {
            Self::Columns { bar_gap, seg_gap, baseline } => {
                columns(band, seg, ctx, bar_gap, seg_gap, baseline)
            }
        }
    }
}

fn columns(
    band: &Band,
    seg: &Segment,
    ctx: &LayoutCtx,
    bar_gap: f32,
    seg_gap: f32,
    baseline: Baseline,
) -> Placement {
    let slot_w = 1.0 / ctx.band_count.max(1) as f32;
    let bar_w = slot_w * (1.0 - bar_gap.clamp(0.0, 0.95));
    let cx = (band.index as f32 + 0.5) * slot_w;
    // Width fraction to height-normalised units.
    let half_w = bar_w * 0.5 * ctx.aspect;

    let seg_slot = 1.0 / ctx.seg_count.max(1) as f32;
    let seg_h = seg_slot * (1.0 - seg_gap.clamp(0.0, 0.95));

    match baseline {
        Baseline::Bottom => Placement {
            center: [cx, 1.0 - (seg.index as f32 + 0.5) * seg_slot],
            half_size: [half_w, seg_h * 0.5],
            rotation: 0.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visualizer::field::{SegmentField, SegmentFieldConfig};

    fn ctx(bands: usize, segs: usize) -> LayoutCtx {
        LayoutCtx { aspect: 2.0, band_count: bands, seg_count: segs, elapsed: 0.0 }
    }

    fn band(index: usize, count: usize) -> Band {
        Band {
            index,
            band_t: crate::visualizer::model::normalized_index(index, count),
            ..Band::default()
        }
    }

    fn field(segs: usize) -> SegmentField {
        SegmentField::new(8, SegmentFieldConfig { seg_count: segs, ..Default::default() })
    }

    #[test]
    fn columns_stack_upward_from_the_bottom() {
        let f = field(10);
        let l = Layout::Columns {
            bar_gap: 0.0,
            seg_gap: 0.0,
            baseline: Baseline::Bottom,
        };
        let c = ctx(8, 10);
        let bottom = l.place(&band(0, 8), &f.band(0)[0], &c);
        let top = l.place(&band(0, 8), &f.band(0)[9], &c);
        assert!(bottom.center[1] > top.center[1], "segment 0 must sit lowest");
        assert!((bottom.center[1] - 0.95).abs() < 1e-6);
        assert!((top.center[1] - 0.05).abs() < 1e-6);
    }

    #[test]
    fn columns_tile_the_width_without_gaps() {
        let f = field(4);
        let l = Layout::Columns {
            bar_gap: 0.0,
            seg_gap: 0.0,
            baseline: Baseline::Bottom,
        };
        let c = ctx(4, 4);
        // half_size is height-normalised, so undo the aspect to get width back.
        let total: f32 = (0..4)
            .map(|i| l.place(&band(i, 4), &f.band(i)[0], &c).half_size[0] * 2.0 / c.aspect)
            .sum();
        assert!((total - 1.0).abs() < 1e-6);
    }

    #[test]
    fn bar_gap_shrinks_columns() {
        let f = field(4);
        let c = ctx(4, 4);
        let wide = Layout::Columns { bar_gap: 0.0, seg_gap: 0.0, baseline: Baseline::Bottom }
            .place(&band(0, 4), &f.band(0)[0], &c);
        let narrow = Layout::Columns { bar_gap: 0.5, seg_gap: 0.0, baseline: Baseline::Bottom }
            .place(&band(0, 4), &f.band(0)[0], &c);
        assert!((narrow.half_size[0] - wide.half_size[0] * 0.5).abs() < 1e-6);
        // Columns stay centred in their slot regardless of gap.
        assert_eq!(narrow.center[0], wide.center[0]);
    }





    #[test]
    fn degenerate_counts_do_not_divide_by_zero() {
        let c = ctx(0, 0);
        let seg = Segment::default();
        let p = Layout::default().place(&band(0, 1), &seg, &c);
        assert!(p.center.iter().all(|v| v.is_finite()));
        assert!(p.half_size.iter().all(|v| v.is_finite()));
    }
}
