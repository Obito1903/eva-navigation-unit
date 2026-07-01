//! Arc / fan spectrum analyzer (Kenwood DPX-440 inspired).
//!
//! Bars radiate outward from a focal point below the bottom of the display,
//! spanning a ±60° fan. Each bar direction is perpendicular to the focal radius
//! at that angle — pure 2D, no 3D projection. The result mimics the classic
//! head-unit stadium / sunrise look.
//!
//! The render pipeline is identical to `BarsRenderer`:
//!   1. Render grid + fan bars to an offscreen FBO.
//!   2. Additive bloom pass → original framebuffer.
//!   3. Sharp copy pass on top.

use std::num::NonZeroU32;

use glow::HasContext;

use crate::spectrum::BANDS;
use crate::visualizer::{
    as_u8_slice, build_program, BAR_FRAG, BAR_VERT, BLOOM_FRAG, COPY_FRAG, GRID_FRAG, QUAD_VERT,
    SIDEBAR_W,
};
use crate::visualizer::SpectrumRenderer;

// ── Fan geometry constants ────────────────────────────────────────────────────

/// Total angular span of the fan (radians). Centred on vertical (π/2).
const FAN_HALF_ANGLE: f32 = std::f32::consts::FRAC_PI_2 * (2.0 / 3.0); // 60°

/// How far below the visible bottom edge the focal origin sits, expressed as a
/// fraction of the display height. Larger → more parallel-looking beams.
const FOCAL_DEPTH: f32 = 0.18;

// ── Theme (same palette as bars for consistency) ──────────────────────────────

fn theme_palette(theme_id: i32) -> ([f32; 3], [f32; 3], [f32; 3], [f32; 3]) {
    match theme_id {
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
    }
}

fn neon_tip(band_index: usize) -> [f32; 3] {
    let t = band_index as f32 / BANDS as f32;
    [1.0 - t, 0.0, t]
}

// ── GL state ──────────────────────────────────────────────────────────────────

struct GlState {
    bar_program: glow::Program,
    bar_pos: u32,
    bar_color: u32,
    bar_vbo: glow::Buffer,

    bloom_program: glow::Program,
    bloom_u_tex: glow::UniformLocation,
    bloom_u_texel: glow::UniformLocation,
    bloom_u_radius: glow::UniformLocation,
    bloom_u_strength: glow::UniformLocation,

    copy_program: glow::Program,
    copy_u_tex: glow::UniformLocation,
    quad_pos: u32,

    grid_program: glow::Program,
    grid_u_color: glow::UniformLocation,
    grid_u_width: glow::UniformLocation,
    grid_u_height: glow::UniformLocation,

    quad_vbo: glow::Buffer,

    fbo: glow::Framebuffer,
    fbo_tex: glow::Texture,
    fbo_w: u32,
    fbo_h: u32,
}

impl GlState {
    unsafe fn new(gl: &glow::Context) -> Self {
        let bar_program = build_program(gl, BAR_VERT, BAR_FRAG);
        let bar_pos = gl.get_attrib_location(bar_program, "a_pos").unwrap();
        let bar_color = gl.get_attrib_location(bar_program, "a_color").unwrap();

        let bloom_program = build_program(gl, QUAD_VERT, BLOOM_FRAG);
        let bloom_u_tex = gl.get_uniform_location(bloom_program, "u_tex").unwrap();
        let bloom_u_texel = gl.get_uniform_location(bloom_program, "u_texel").unwrap();
        let bloom_u_radius = gl.get_uniform_location(bloom_program, "u_radius").unwrap();
        let bloom_u_strength = gl.get_uniform_location(bloom_program, "u_strength").unwrap();

        let copy_program = build_program(gl, QUAD_VERT, COPY_FRAG);
        let copy_u_tex = gl.get_uniform_location(copy_program, "u_tex").unwrap();
        let quad_pos = gl.get_attrib_location(copy_program, "pos").unwrap();

        let grid_program = build_program(gl, QUAD_VERT, GRID_FRAG);
        let grid_u_color = gl.get_uniform_location(grid_program, "u_grid_color").unwrap();
        let grid_u_width = gl.get_uniform_location(grid_program, "u_width").unwrap();
        let grid_u_height = gl.get_uniform_location(grid_program, "u_height").unwrap();

        let quad: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
        let quad_vbo = gl.create_buffer().unwrap();
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(quad_vbo));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_u8_slice(&quad), glow::STATIC_DRAW);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);

        // Max geometry: 32 bars × 2 quads × 6 verts × 6 floats
        let bar_vbo = gl.create_buffer().unwrap();
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(bar_vbo));
        gl.buffer_data_u8_slice(
            glow::ARRAY_BUFFER,
            as_u8_slice(&vec![0.0f32; BANDS * 12 * 6]),
            glow::DYNAMIC_DRAW,
        );
        gl.bind_buffer(glow::ARRAY_BUFFER, None);

        let fbo = gl.create_framebuffer().unwrap();
        let fbo_tex = gl.create_texture().unwrap();

        Self {
            bar_program,
            bar_pos,
            bar_color,
            bar_vbo,
            bloom_program,
            bloom_u_tex,
            bloom_u_texel,
            bloom_u_radius,
            bloom_u_strength,
            copy_program,
            copy_u_tex,
            quad_pos,
            grid_program,
            grid_u_color,
            grid_u_width,
            grid_u_height,
            quad_vbo,
            fbo,
            fbo_tex,
            fbo_w: 0,
            fbo_h: 0,
        }
    }

    unsafe fn ensure_fbo(&mut self, gl: &glow::Context, w: u32, h: u32) {
        if self.fbo_w == w && self.fbo_h == h {
            return;
        }
        gl.bind_texture(glow::TEXTURE_2D, Some(self.fbo_tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA as i32,
            w as i32,
            h as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        gl.bind_texture(glow::TEXTURE_2D, None);
        self.fbo_w = w;
        self.fbo_h = h;
    }

    unsafe fn teardown(&mut self, gl: &glow::Context) {
        gl.delete_program(self.bar_program);
        gl.delete_program(self.bloom_program);
        gl.delete_program(self.copy_program);
        gl.delete_program(self.grid_program);
        gl.delete_buffer(self.bar_vbo);
        gl.delete_buffer(self.quad_vbo);
        gl.delete_framebuffer(self.fbo);
        gl.delete_texture(self.fbo_tex);
    }
}

// ── ArcRenderer ───────────────────────────────────────────────────────────────

pub struct ArcRenderer {
    theme_id: i32,
    state: Option<GlState>,
}

impl ArcRenderer {
    pub fn new(theme_id: i32) -> Self {
        Self { theme_id, state: None }
    }
}

impl SpectrumRenderer for ArcRenderer {
    fn setup(&mut self, gl: &glow::Context, _w: u32, _h: u32) {
        self.state = Some(unsafe { GlState::new(gl) });
    }

    fn resize(&mut self, gl: &glow::Context, w: u32, h: u32) {
        if let Some(s) = &mut self.state {
            unsafe { s.ensure_fbo(gl, w, h) };
        }
    }

    fn teardown(&mut self, gl: &glow::Context) {
        if let Some(mut s) = self.state.take() {
            unsafe { s.teardown(gl) };
        }
    }

    fn render(&mut self, gl: &glow::Context, w: u32, h: u32, bands: &[f32], peaks: &[f32]) {
        let Some(s) = &mut self.state else { return };
        unsafe { render_arc(gl, s, w, h, bands, peaks, self.theme_id) };
    }
}

// ── Core render logic ─────────────────────────────────────────────────────────

unsafe fn render_arc(
    gl: &glow::Context,
    s: &mut GlState,
    w: u32,
    h: u32,
    bands: &[f32],
    peaks: &[f32],
    theme_id: i32,
) {
    s.ensure_fbo(gl, w, h);

    let (base_rgb, tip_rgb, peak_rgb, grid_rgb) = theme_palette(theme_id);

    let prev_fbo = {
        let raw = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
        NonZeroU32::new(raw as u32).map(glow::NativeFramebuffer)
    };

    // ── Pass 1: grid + fan bars → offscreen FBO ───────────────────────────
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(s.fbo));
    gl.framebuffer_texture_2d(
        glow::FRAMEBUFFER,
        glow::COLOR_ATTACHMENT0,
        glow::TEXTURE_2D,
        Some(s.fbo_tex),
        0,
    );
    gl.viewport(0, 0, w as i32, h as i32);
    gl.disable(glow::DEPTH_TEST);
    gl.disable(glow::BLEND);
    gl.clear_color(0.0, 0.0, 0.0, 1.0);
    gl.clear(glow::COLOR_BUFFER_BIT);

    // Background dot grid
    gl.use_program(Some(s.grid_program));
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(s.quad_vbo));
    gl.enable_vertex_attrib_array(s.quad_pos);
    gl.vertex_attrib_pointer_f32(s.quad_pos, 2, glow::FLOAT, false, 0, 0);
    gl.uniform_3_f32(Some(&s.grid_u_color), grid_rgb[0], grid_rgb[1], grid_rgb[2]);
    gl.uniform_1_f32(Some(&s.grid_u_width), w as f32);
    gl.uniform_1_f32(Some(&s.grid_u_height), h as f32);
    gl.enable(glow::BLEND);
    gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
    gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
    gl.disable_vertex_attrib_array(s.quad_pos);

    // Fan bar geometry
    let verts = build_arc_geometry(w, h, bands, peaks, theme_id, &base_rgb, &tip_rgb, &peak_rgb);
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(s.bar_vbo));
    gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, as_u8_slice(&verts));

    gl.use_program(Some(s.bar_program));
    const STRIDE: i32 = 24;
    gl.enable_vertex_attrib_array(s.bar_pos);
    gl.vertex_attrib_pointer_f32(s.bar_pos, 2, glow::FLOAT, false, STRIDE, 0);
    gl.enable_vertex_attrib_array(s.bar_color);
    gl.vertex_attrib_pointer_f32(s.bar_color, 4, glow::FLOAT, false, STRIDE, 8);
    gl.draw_arrays(glow::TRIANGLES, 0, verts.len() as i32 / 6);
    gl.disable_vertex_attrib_array(s.bar_pos);
    gl.disable_vertex_attrib_array(s.bar_color);

    // ── Pass 2: composite onto original framebuffer ───────────────────────
    gl.bind_framebuffer(glow::FRAMEBUFFER, prev_fbo);
    gl.viewport(0, 0, w as i32, h as i32);
    gl.clear_color(0.0, 0.0, 0.0, 1.0);
    gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);

    gl.active_texture(glow::TEXTURE0);
    gl.bind_texture(glow::TEXTURE_2D, Some(s.fbo_tex));

    // Additive bloom
    gl.enable(glow::BLEND);
    gl.blend_func(glow::SRC_ALPHA, glow::ONE);
    gl.use_program(Some(s.bloom_program));
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(s.quad_vbo));
    gl.enable_vertex_attrib_array(s.quad_pos);
    gl.vertex_attrib_pointer_f32(s.quad_pos, 2, glow::FLOAT, false, 0, 0);
    gl.uniform_1_i32(Some(&s.bloom_u_tex), 0);
    gl.uniform_2_f32(Some(&s.bloom_u_texel), 1.0 / w as f32, 1.0 / h as f32);
    gl.uniform_1_f32(Some(&s.bloom_u_radius), 6.0); // wider spread for 3×3 kernel
    gl.uniform_1_f32(Some(&s.bloom_u_strength), 1.4);
    gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

    // Sharp copy
    gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
    gl.use_program(Some(s.copy_program));
    gl.uniform_1_i32(Some(&s.copy_u_tex), 0);
    gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

    gl.disable_vertex_attrib_array(s.quad_pos);
    gl.disable(glow::BLEND);
    gl.bind_buffer(glow::ARRAY_BUFFER, None);
    gl.bind_texture(glow::TEXTURE_2D, None);
    gl.use_program(None);
}

// ── Fan geometry builder ──────────────────────────────────────────────────────

/// Build the fan-bar vertex data.
///
/// Each bar is a rotated quad: its long axis points from the focal origin
/// outward at the bar's angle, and its short axis (width) is perpendicular.
/// The focal origin is below the display area by `FOCAL_DEPTH × h`.
fn build_arc_geometry(
    w: u32,
    h: u32,
    bands: &[f32],
    peaks: &[f32],
    theme_id: i32,
    base_rgb: &[f32; 3],
    tip_rgb: &[f32; 3],
    peak_rgb: &[f32; 3],
) -> Vec<f32> {
    let wf = w as f32;
    let hf = h as f32;

    // Focal origin in screen pixels.
    let content_left = SIDEBAR_W;
    let content_w = wf - content_left;
    let cx = content_left + content_w * 0.5;
    let cy = hf * (1.0 + FOCAL_DEPTH);

    // Maximum bar length: from focal origin to near the top of the display.
    let max_len = hf * (1.0 + FOCAL_DEPTH) - hf * 0.06;
    let min_len = hf * 0.10; // short base even when magnitude is 0

    // Bar width: narrow enough that adjacent bars don't visually overlap at
    // the tips even at max length.
    let bar_half_w = (content_w / BANDS as f32) * 0.28;
    let peak_half_h = 3.0_f32;

    let mut verts: Vec<f32> = Vec::with_capacity(bands.len() * 12 * 6);

    for i in 0..bands.len() {
        // Angle from vertical for this band. Left bands are negative (lean left).
        let frac = i as f32 / (BANDS - 1) as f32; // 0.0 … 1.0
        let angle = -FAN_HALF_ANGLE + frac * FAN_HALF_ANGLE * 2.0;

        // Direction unit vector for the bar axis (away from focal point).
        let (sin_a, cos_a) = angle.sin_cos();
        let dir = [sin_a, -cos_a]; // positive y is down in screen space

        // Perpendicular to bar axis (for bar width).
        let perp = [cos_a, sin_a];

        let band_val = bands[i];
        let peak_val = peaks[i];

        let bar_len = min_len + band_val * (max_len - min_len);
        let peak_len = min_len + peak_val * (max_len - min_len);

        let (t_rgb, p_rgb) = if theme_id == 3 {
            (neon_tip(i), [1.0f32, 0.9, 1.0])
        } else {
            (*tip_rgb, *peak_rgb)
        };

        let t = band_val;
        let bc = lerp3(base_rgb, base_rgb, 0.0);
        let tc = lerp3(base_rgb, &t_rgb, t);

        // Bar quad corners in screen space
        let base_x = cx; // all bars start from the focal origin
        let base_y = cy;
        let tip_x = cx + dir[0] * bar_len;
        let tip_y = cy + dir[1] * bar_len;

        push_rotated_quad(
            &mut verts,
            base_x, base_y,
            tip_x, tip_y,
            bar_half_w, &perp,
            &bc, &tc, wf, hf,
        );

        // Peak dot
        if peak_val > 0.02 {
            let pk_mid_x = cx + dir[0] * peak_len;
            let pk_mid_y = cy + dir[1] * peak_len;
            let pk_base_x = cx + dir[0] * (peak_len - peak_half_h);
            let pk_base_y = cy + dir[1] * (peak_len - peak_half_h);
            push_rotated_quad(
                &mut verts,
                pk_base_x, pk_base_y,
                pk_mid_x + dir[0] * peak_half_h,
                pk_mid_y + dir[1] * peak_half_h,
                bar_half_w, &perp,
                &p_rgb, &p_rgb, wf, hf,
            );
        }
    }

    verts
}

/// Push a bar quad whose long axis goes from `(x0,y0)` to `(x1,y1)` and
/// whose short axis half-width is `hw` along `perp`.
fn push_rotated_quad(
    verts: &mut Vec<f32>,
    x0: f32, y0: f32,
    x1: f32, y1: f32,
    hw: f32,
    perp: &[f32; 2],
    base_rgb: &[f32; 3],
    tip_rgb: &[f32; 3],
    w: f32,
    h: f32,
) {
    let bl = [x0 - perp[0] * hw, y0 - perp[1] * hw];
    let br = [x0 + perp[0] * hw, y0 + perp[1] * hw];
    let tl = [x1 - perp[0] * hw, y1 - perp[1] * hw];
    let tr = [x1 + perp[0] * hw, y1 + perp[1] * hw];

    // Triangle 1: bl, br, tr
    vert(verts, to_ndc_x(bl[0], w), to_ndc_y(bl[1], h), base_rgb, 1.0);
    vert(verts, to_ndc_x(br[0], w), to_ndc_y(br[1], h), base_rgb, 1.0);
    vert(verts, to_ndc_x(tr[0], w), to_ndc_y(tr[1], h), tip_rgb,  1.0);
    // Triangle 2: bl, tr, tl
    vert(verts, to_ndc_x(bl[0], w), to_ndc_y(bl[1], h), base_rgb, 1.0);
    vert(verts, to_ndc_x(tr[0], w), to_ndc_y(tr[1], h), tip_rgb,  1.0);
    vert(verts, to_ndc_x(tl[0], w), to_ndc_y(tl[1], h), tip_rgb,  1.0);
}

#[inline]
fn vert(verts: &mut Vec<f32>, x: f32, y: f32, rgb: &[f32; 3], a: f32) {
    verts.extend_from_slice(&[x, y, rgb[0], rgb[1], rgb[2], a]);
}

#[inline]
fn to_ndc_x(px: f32, w: f32) -> f32 { (px / w) * 2.0 - 1.0 }
#[inline]
fn to_ndc_y(py: f32, h: f32) -> f32 { 1.0 - (py / h) * 2.0 }

#[inline]
fn lerp3(a: &[f32; 3], b: &[f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}
