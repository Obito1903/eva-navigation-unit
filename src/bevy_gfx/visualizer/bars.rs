//! VFD-style spectrum analyzer geometry — discrete illuminated segments.
//!
//! Each segment used to be resolved per-pixel by a fullscreen fragment shader;
//! it is now emitted as emissive quads whose HDR vertex colours feed the
//! camera's bloom. Lit segments keep the `█|█` VFD sub-element shape (two
//! blocks plus a thin centre spine); unlit segments collapse to a single dim
//! quad so the whole grid silhouette still reads without tripling the vertex
//! count.

use bevy::math::Vec2;

/// Vertical segment rows the peak dot snaps to.
pub(super) struct Params<'a> {
    pub(super) size: Vec2,
    pub(super) bands: &'a [f32],
    pub(super) peaks: &'a [f32],
    pub(super) theme_id: i32,
    pub(super) bar_gap: f32,
    pub(super) seg_gap_px: f32,
    pub(super) seg_count: usize,
    pub(super) top_pad: f32,
    pub(super) bottom_pad: f32,
}

/// `(color_lo, color_hi, color_peak, color_dim)` for a theme id.
fn palette(theme_id: i32) -> ([f32; 3], [f32; 3], [f32; 3], [f32; 3]) {
    match theme_id {
        1 => (
            [0.40, 0.02, 0.01], // lo:   deep red
            [1.00, 0.30, 0.05], // hi:   bright orange-red
            [1.00, 0.70, 0.40], // peak: warm white
            [0.40, 0.05, 0.02], // dim:  dark red tint
        ),
        2 => (
            [0.00, 0.25, 0.04],
            [0.10, 1.00, 0.30],
            [0.60, 1.00, 0.70],
            [0.00, 0.30, 0.06],
        ),
        3 => (
            [0.25, 0.00, 0.30],
            [0.00, 0.90, 1.00],
            [0.80, 0.80, 1.00],
            [0.15, 0.00, 0.20],
        ),
        _ => (
            // VFD / Kenwood cyan-phosphor (theme 0)
            [0.00, 0.30, 0.42],
            [0.00, 0.88, 1.00],
            [0.55, 1.00, 1.00],
            [0.00, 0.28, 0.38],
        ),
    }
}

/// How far past 1.0 lit segments are pushed so bloom picks them up.
const ACTIVE_GAIN: f32 = 2.2;
const PEAK_GAIN: f32 = 3.2;
const DIM_GAIN: f32 = 0.30;

pub(super) fn build(params: Params) -> (Vec<[f32; 3]>, Vec<[f32; 4]>) {
    let Params {
        size,
        bands,
        peaks,
        theme_id,
        bar_gap,
        seg_gap_px,
        seg_count,
        top_pad,
        bottom_pad,
    } = params;

    let (col_lo, col_hi, col_peak, col_dim) = palette(theme_id);
    let n = bands.len().min(peaks.len());
    let segs = seg_count.max(1);
    let area_h = (size.y - top_pad - bottom_pad).max(1.0);
    let content_w = (size.x - 4.0).max(1.0);
    if n == 0 {
        return (Vec::new(), Vec::new());
    }

    let slot_w = content_w / n as f32;
    let seg_slot = area_h / segs as f32;
    let seg_body = (seg_slot - seg_gap_px).max(1.0);
    let gap_half = bar_gap.clamp(0.0, 0.9) * 0.5;

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();

    for (i, (&level, &peak)) in bands.iter().zip(peaks.iter()).enumerate() {
        let slot_x = i as f32 * slot_w;
        let bar_x0 = slot_x + slot_w * gap_half;
        let bar_w = slot_w * (1.0 - gap_half * 2.0);
        let peak_seg = (peak * segs as f32).floor();

        for s in 0..segs {
            let seg_level = (s as f32 + 0.5) / segs as f32;
            let y0 = bottom_pad + s as f32 * seg_slot;
            let is_peak = peak >= 0.015 && (s as f32 - peak_seg).abs() < 0.5;
            let active = seg_level <= level + 0.5 / segs as f32;

            if is_peak {
                let c = scale(col_peak, PEAK_GAIN);
                push_vfd_cell(&mut positions, &mut colors, bar_x0, bar_w, y0, seg_body, c, size);
            } else if active {
                let c = scale(mix(col_lo, col_hi, seg_level), ACTIVE_GAIN);
                push_vfd_cell(&mut positions, &mut colors, bar_x0, bar_w, y0, seg_body, c, size);
            } else {
                // Unlit: one flat, dim quad keeping the grid silhouette visible.
                let c = scale(col_dim, DIM_GAIN);
                push_quad(
                    &mut positions,
                    &mut colors,
                    bar_x0,
                    y0,
                    bar_w,
                    seg_body,
                    c,
                    size,
                );
            }
        }
    }

    (positions, colors)
}

/// The `█|█` VFD segment: left block, thin centre spine, right block. The
/// spine reads slightly brighter, as on real VFD wiring grids.
#[expect(clippy::too_many_arguments, reason = "plain geometry emitter")]
fn push_vfd_cell(
    positions: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
    x0: f32,
    w: f32,
    y0: f32,
    h: f32,
    color: [f32; 3],
    size: Vec2,
) {
    push_quad(positions, colors, x0, y0, w * 0.30, h, color, size);
    push_quad(positions, colors, x0 + w * 0.70, y0, w * 0.30, h, color, size);
    push_quad(
        positions,
        colors,
        x0 + w * 0.45,
        y0,
        w * 0.10,
        h,
        scale(color, 1.20),
        size,
    );
}

/// Emit one axis-aligned quad given in pixels (origin bottom-left) as two
/// triangles in the visualizer camera's 2×2 NDC world.
#[expect(clippy::too_many_arguments, reason = "plain geometry emitter")]
fn push_quad(
    positions: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 3],
    size: Vec2,
) {
    let x0 = x / size.x * 2.0 - 1.0;
    let x1 = (x + w) / size.x * 2.0 - 1.0;
    let y0 = y / size.y * 2.0 - 1.0;
    let y1 = (y + h) / size.y * 2.0 - 1.0;
    positions.extend_from_slice(&[
        [x0, y0, 0.0],
        [x1, y0, 0.0],
        [x1, y1, 0.0],
        [x0, y0, 0.0],
        [x1, y1, 0.0],
        [x0, y1, 0.0],
    ]);
    let c = [color[0], color[1], color[2], 1.0];
    colors.extend_from_slice(&[c; 6]);
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn scale(c: [f32; 3], k: f32) -> [f32; 3] {
    [c[0] * k, c[1] * k, c[2] * k]
}
