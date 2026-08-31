//! Arc / fan spectrum analyzer geometry (Kenwood DPX-440 inspired).
//!
//! Bars radiate outward from a focal point below the bottom of the display,
//! spanning a ±60° fan, each bar perpendicular to the focal radius at its
//! angle. The mesh is rebuilt every frame; HDR vertex colours plus camera bloom
//! replace the old additive blur pass, and the dot-grid backdrop is now a tiled
//! texture rather than a fullscreen shader.

use bevy::color::LinearRgba;
use bevy::math::Vec2;

use crate::spectrum::BANDS;

/// Total angular span of the fan (radians). Centred on vertical.
const FAN_HALF_ANGLE: f32 = std::f32::consts::FRAC_PI_2 * (2.0 / 3.0); // 60°

/// How far below the visible bottom edge the focal origin sits, as a fraction
/// of the display height. Larger → more parallel-looking beams.
const FOCAL_DEPTH: f32 = 0.18;

/// How far past 1.0 bars are pushed so bloom picks them up.
const GAIN: f32 = 2.0;

pub(super) struct Params<'a> {
    pub(super) size: Vec2,
    pub(super) bands: &'a [f32],
    pub(super) peaks: &'a [f32],
    pub(super) theme_id: i32,
}

/// `(base, tip, peak, grid)` for a theme id. The grid colour is returned as a
/// `LinearRgba` because it tints the backdrop material rather than vertices.
pub(super) fn palette(theme_id: i32) -> ([f32; 3], [f32; 3], [f32; 3], LinearRgba) {
    let (base, tip, peak, grid) = match theme_id {
        1 => (
            [0.35, 0.03, 0.02],
            [0.90, 0.14, 0.07],
            [1.00, 0.45, 0.30],
            [0.50, 0.05, 0.03],
        ),
        2 => (
            [0.00, 0.22, 0.05],
            [0.00, 1.00, 0.25],
            [0.50, 1.00, 0.60],
            [0.00, 0.35, 0.08],
        ),
        3 => (
            [0.30, 0.00, 0.35],
            [0.00, 0.85, 1.00],
            [1.00, 0.90, 1.00],
            [0.20, 0.00, 0.25],
        ),
        _ => (
            [0.00, 0.25, 0.38],
            [0.00, 0.85, 1.00],
            [0.50, 1.00, 1.00],
            [0.00, 0.30, 0.45],
        ),
    };
    (base, tip, peak, LinearRgba::rgb(grid[0], grid[1], grid[2]))
}

/// NEON (theme 3) sweeps the tip colour across the spectrum instead of using a
/// single hue.
fn neon_tip(band_index: usize) -> [f32; 3] {
    let t = band_index as f32 / BANDS as f32;
    [1.0 - t, 0.0, t]
}

pub(super) fn build(params: Params) -> (Vec<[f32; 3]>, Vec<[f32; 4]>) {
    let Params { size, bands, peaks, theme_id } = params;
    let (base_rgb, tip_rgb, peak_rgb, _) = palette(theme_id);

    let (wf, hf) = (size.x, size.y);
    // Focal origin in pixels, measured from the top-left of the target.
    let cx = wf * 0.5;
    let cy = hf * (1.0 + FOCAL_DEPTH);

    let max_len = hf * (1.0 + FOCAL_DEPTH) - hf * 0.06;
    let min_len = hf * 0.10; // short base even at zero magnitude
    let bar_half_w = (wf / BANDS as f32) * 0.28;
    let peak_half_h = 3.0_f32;

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(bands.len() * 12);
    let mut colors: Vec<[f32; 4]> = Vec::with_capacity(bands.len() * 12);

    for (i, (&level, &peak)) in bands.iter().zip(peaks.iter()).enumerate() {
        let frac = i as f32 / (BANDS - 1) as f32;
        let angle = -FAN_HALF_ANGLE + frac * FAN_HALF_ANGLE * 2.0;
        let (sin_a, cos_a) = angle.sin_cos();
        // Positive y is down in this pixel space, so the bar axis points up.
        let dir = [sin_a, -cos_a];
        let perp = [cos_a, sin_a];

        let bar_len = min_len + level * (max_len - min_len);
        let peak_len = min_len + peak * (max_len - min_len);

        let (t_rgb, p_rgb) = if theme_id == 3 {
            (neon_tip(i), [1.0_f32, 0.9, 1.0])
        } else {
            (tip_rgb, peak_rgb)
        };
        let tip = mix(base_rgb, t_rgb, level);

        push_rotated_quad(
            &mut positions,
            &mut colors,
            Quad {
                from: [cx, cy],
                to: [cx + dir[0] * bar_len, cy + dir[1] * bar_len],
                half_width: bar_half_w,
                perp,
                from_rgb: base_rgb,
                to_rgb: tip,
                size,
            },
        );

        if peak > 0.02 {
            let start = peak_len - peak_half_h;
            let end = peak_len + peak_half_h;
            push_rotated_quad(
                &mut positions,
                &mut colors,
                Quad {
                    from: [cx + dir[0] * start, cy + dir[1] * start],
                    to: [cx + dir[0] * end, cy + dir[1] * end],
                    half_width: bar_half_w,
                    perp,
                    from_rgb: p_rgb,
                    to_rgb: p_rgb,
                    size,
                },
            );
        }
    }

    (positions, colors)
}

struct Quad {
    from: [f32; 2],
    to: [f32; 2],
    half_width: f32,
    perp: [f32; 2],
    from_rgb: [f32; 3],
    to_rgb: [f32; 3],
    size: Vec2,
}

/// Push a bar quad whose long axis runs `from` → `to` and whose short axis
/// half-width is `half_width` along `perp`.
fn push_rotated_quad(positions: &mut Vec<[f32; 3]>, colors: &mut Vec<[f32; 4]>, quad: Quad) {
    let Quad { from, to, half_width, perp, from_rgb, to_rgb, size } = quad;
    let off = [perp[0] * half_width, perp[1] * half_width];
    let bl = ndc([from[0] - off[0], from[1] - off[1]], size);
    let br = ndc([from[0] + off[0], from[1] + off[1]], size);
    let tl = ndc([to[0] - off[0], to[1] - off[1]], size);
    let tr = ndc([to[0] + off[0], to[1] + off[1]], size);

    positions.extend_from_slice(&[bl, br, tr, bl, tr, tl]);
    let b = rgba(from_rgb);
    let t = rgba(to_rgb);
    colors.extend_from_slice(&[b, b, t, b, t, t]);
}

/// Pixels (origin top-left) → the visualizer camera's 2×2 NDC world.
fn ndc(p: [f32; 2], size: Vec2) -> [f32; 3] {
    [p[0] / size.x * 2.0 - 1.0, 1.0 - p[1] / size.y * 2.0, 0.0]
}

fn rgba(c: [f32; 3]) -> [f32; 4] {
    [c[0] * GAIN, c[1] * GAIN, c[2] * GAIN, 1.0]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}
