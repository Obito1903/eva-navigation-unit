//! VFD-style spectrum analyzer — discrete illuminated segments.
//!
//! The entire display is rendered by a single fullscreen fragment shader.
//! Bar heights and peak positions are encoded into a 1×n_bands RGBA texture
//! (R = bar level, G = peak level, both 0–255) that is uploaded each frame.
//! The shader divides the content area into a grid of rectangular cells and
//! determines per-pixel whether each cell is active, unlit, or the peak dot.
//!
//! VFD aesthetics:
//!   • 28 discrete segment rows per bar — the "digital column" look
//!   • Soft per-cell vignette (phosphor dot: centre brighter than edges)
//!   • Subtle intra-cell scanline pattern (VFD pixel rows)
//!   • Darkened but clearly visible unlit segments — the full grid silhouette
//!     always reads, matching a real VFD's always-visible dark segments
//!   • Additive bloom pass for the phosphor halo

use std::num::NonZeroU32;

use glow::HasContext;

use crate::visualizer::{
    as_u8_slice, build_program, BLOOM_FRAG, COPY_FRAG, QUAD_VERT, SIDEBAR_W,
};
use crate::visualizer::SpectrumRenderer;
use crate::spectrum::BANDS;

// ── VFD fragment shader ───────────────────────────────────────────────────────

const VFD_FRAG: &str = r"#version 100
precision mediump float;

uniform sampler2D u_bars;   // 1 x n_bands  R=bar G=peak  (UNSIGNED_BYTE → 0..1)
uniform float u_n_bands;
uniform float u_tex_width;  // actual texture width = BANDS (64); used for texture coords
uniform float u_n_segs;     // vertical segment count (e.g. 50)
uniform float u_bar_gap;    // horizontal gap between bar columns (fraction of slot, 0..0.45)
uniform float u_seg_gap_px; // vertical gap between segment rows in pixels
uniform float u_width;
uniform float u_height;
uniform float u_sidebar;    // sidebar width in pixels
uniform vec3  u_color_lo;   // active colour at lowest level
uniform vec3  u_color_hi;   // active colour at highest level
uniform vec3  u_color_peak; // peak-dot colour
uniform vec3  u_color_dim;  // unlit segment tint

varying vec2 v_uv;

void main() {
    float px = v_uv.x * u_width;
    float py = v_uv.y * u_height;

    // ── Content area ──────────────────────────────────────────────────────
    float cx     = px - u_sidebar;
    float cw     = u_width - u_sidebar - 4.0;
    float top_pad = 12.0;
    float bot_pad = 56.0;         // room for the HUD control bar
    float area_h  = u_height - top_pad - bot_pad;
    float y       = py - top_pad; // 0 = top of content, area_h = bottom

    if (cx < 0.0 || cx > cw || y < 0.0 || y > area_h) {
        gl_FragColor = vec4(0.0, 0.0, 0.0, 1.0);
        return;
    }

    // ── Bar column ────────────────────────────────────────────────────────
    float slot_w = cw / u_n_bands;
    float bar_fi = cx / slot_w;
    float bar_i  = floor(bar_fi);
    float x_slot = fract(bar_fi);   // 0..1 within slot

    // Horizontal gap between bar columns
    float h_gap  = u_bar_gap;
    float h_half = h_gap * 0.5;
    if (x_slot < h_half || x_slot > 1.0 - h_half) {
        gl_FragColor = vec4(0.0, 0.0, 0.0, 1.0);
        return;
    }
    float x_bar = (x_slot - h_half) / (1.0 - h_gap); // 0..1 inside bar body

    // ── Segment row ───────────────────────────────────────────────────────
    // Index from bottom: seg 0 = lowest band, seg n_segs-1 = highest
    float seg_slot = area_h / u_n_segs;
    float seg_fi   = (area_h - y) / seg_slot;   // 0.0 = bottom, n_segs = top
    float seg_i    = floor(seg_fi);
    float y_slot   = fract(seg_fi);             // 0..1 within segment slot

    // Gap between segments (top of each slot)
    float v_gap = u_seg_gap_px / seg_slot;
    if (y_slot > 1.0 - v_gap) {
        // Segment separator — very dark background
        gl_FragColor = vec4(0.002, 0.002, 0.004, 1.0);
        return;
    }
    float y_body = y_slot / (1.0 - v_gap);     // 0..1 within cell body

    // ── Sample bar data texture ───────────────────────────────────────────
    // u_tex_width is the full texture width (BANDS=64), not the active band
    // count, so the coordinate correctly addresses the uploaded texels.
    float tx     = (bar_i + 0.5) / u_tex_width;
    vec4  data   = texture2D(u_bars, vec2(tx, 0.5));
    float bar_h  = data.r;   // 0..1
    float peak_h = data.g;   // 0..1

    // ── Segment state ─────────────────────────────────────────────────────
    float seg_level = (seg_i + 0.5) / u_n_segs;  // fractional level 0=bot 1=top

    // Active if segment level is within the bar height
    float active = step(seg_level, bar_h + 0.5 / u_n_segs);

    // Peak dot: the single segment that straddles peak_h
    float peak_seg  = floor(peak_h * u_n_segs);
    float is_peak   = step(0.015, peak_h)
                    * (1.0 - step(0.5, abs(seg_i - peak_seg)));

    // Level-based gradient computed here so it's available for the early return
    vec3 act_color = mix(u_color_lo, u_color_hi, seg_level);

    // ── VFD segment shape: █|█ ───────────────────────────────────────────────
    // Each segment is three full-height sub-elements arranged horizontally:
    //   • Left block  (solid, ~30% of bar width)
    //   • Centre line (thin vertical bar, ~10% of bar width)
    //   • Right block (solid, ~30% of bar width)
    // The gaps between them render at near-black so the three pieces read as
    // individual phosphor elements while still belonging to the same segment row.

    float in_left   = step(x_bar, 0.30);
    float in_right  = step(0.70, x_bar);
    float in_center = 1.0 - step(0.05, abs(x_bar - 0.5));  // ±5 % around centre

    float in_shape = clamp(in_left + in_right + in_center, 0.0, 1.0);

    // Pixels in the gaps between sub-elements → near-black, but with a faint
    // dim tint on inactive segments so the full grid silhouette stays visible.
    if (in_shape < 0.5) {
        vec3 bg = (active > 0.5) ? act_color * 0.03 : u_color_dim * 0.06;
        gl_FragColor = vec4(bg, 1.0);
        return;
    }

    // ── Shading within a sub-element ─────────────────────────────────────
    // Vertical vignette: top and bottom edges of each cell are slightly dimmer,
    // giving each row of phosphor elements a soft lit appearance.
    float dy_centre = abs(y_body - 0.5) * 2.0;   // 0 = v-centre, 1 = edge
    float vignette  = 1.0 - 0.25 * dy_centre * dy_centre;

    // The centre bar is slightly brighter than the blocks (it's the connecting
    // spine, so it reads at higher intensity — typical of VFD wiring grids).
    float line_boost = 1.0 + 0.20 * in_center * (1.0 - in_left) * (1.0 - in_right);

    float cell_mod = vignette * line_boost;

    vec3 color;
    if (is_peak > 0.5) {
        color = u_color_peak * (cell_mod * 1.15);
    } else if (active > 0.5) {
        color = act_color * cell_mod;
    } else {
        // Unlit: dimmed but clearly visible silhouette — every segment in the
        // grid should read as a dark phosphor element, not a near-invisible
        // ghost, so the whole VFD matrix is always on screen at low contrast.
        color = u_color_dim * (0.24 * vignette);
    }

    gl_FragColor = vec4(color, 1.0);
}
";

// ── Theme palettes ────────────────────────────────────────────────────────────

/// Returns `(color_lo, color_hi, color_peak, color_dim)` for a theme ID.
fn theme_palette(theme_id: i32) -> ([f32; 3], [f32; 3], [f32; 3], [f32; 3]) {
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
            [0.00, 0.30, 0.42], // lo:   dim teal
            [0.00, 0.88, 1.00], // hi:   bright cyan
            [0.55, 1.00, 1.00], // peak: white-cyan
            [0.00, 0.28, 0.38], // dim:  deep teal ghost
        ),
    }
}

// ── GL state ──────────────────────────────────────────────────────────────────

struct GlState {
    // VFD fullscreen segment shader
    seg_program:       glow::Program,
    seg_pos:           u32,
    seg_u_bars:        glow::UniformLocation,
    seg_u_n_bands:     glow::UniformLocation,
    seg_u_n_segs:      glow::UniformLocation,
    seg_u_width:       glow::UniformLocation,
    seg_u_height:      glow::UniformLocation,
    seg_u_sidebar:     glow::UniformLocation,
    seg_u_color_lo:    glow::UniformLocation,
    seg_u_color_hi:    glow::UniformLocation,
    seg_u_color_peak:  glow::UniformLocation,
    seg_u_color_dim:   glow::UniformLocation,
    seg_u_bar_gap:     glow::UniformLocation,
    seg_u_seg_gap_px:  glow::UniformLocation,
    seg_u_tex_width:   glow::UniformLocation,

    // 1 × n_bands RGBA bar-data texture  (R=bar, G=peak, both as u8 0-255)
    bar_data_tex: glow::Texture,
    n_bands: usize,

    // Bloom + copy passes
    bloom_program:    glow::Program,
    bloom_u_tex:      glow::UniformLocation,
    bloom_u_texel:    glow::UniformLocation,
    bloom_u_radius:   glow::UniformLocation,
    bloom_u_strength: glow::UniformLocation,

    copy_program: glow::Program,
    copy_u_tex:   glow::UniformLocation,
    quad_pos:     u32,

    quad_vbo: glow::Buffer,

    fbo:   glow::Framebuffer,
    fbo_tex: glow::Texture,
    fbo_w: u32,
    fbo_h: u32,
}

impl GlState {
    unsafe fn new(gl: &glow::Context) -> Self {
        // ── VFD segment program ───────────────────────────────────────────
        let seg_program = build_program(gl, QUAD_VERT, VFD_FRAG);
        let seg_pos = gl.get_attrib_location(seg_program, "pos").unwrap();
        let seg_u_bars       = gl.get_uniform_location(seg_program, "u_bars").unwrap();
        let seg_u_n_bands    = gl.get_uniform_location(seg_program, "u_n_bands").unwrap();
        let seg_u_n_segs     = gl.get_uniform_location(seg_program, "u_n_segs").unwrap();
        let seg_u_width      = gl.get_uniform_location(seg_program, "u_width").unwrap();
        let seg_u_height     = gl.get_uniform_location(seg_program, "u_height").unwrap();
        let seg_u_sidebar    = gl.get_uniform_location(seg_program, "u_sidebar").unwrap();
        let seg_u_color_lo   = gl.get_uniform_location(seg_program, "u_color_lo").unwrap();
        let seg_u_color_hi   = gl.get_uniform_location(seg_program, "u_color_hi").unwrap();
        let seg_u_color_peak = gl.get_uniform_location(seg_program, "u_color_peak").unwrap();
        let seg_u_color_dim  = gl.get_uniform_location(seg_program, "u_color_dim").unwrap();
        let seg_u_bar_gap    = gl.get_uniform_location(seg_program, "u_bar_gap").unwrap();
        let seg_u_seg_gap_px = gl.get_uniform_location(seg_program, "u_seg_gap_px").unwrap();
        let seg_u_tex_width  = gl.get_uniform_location(seg_program, "u_tex_width").unwrap();

        // ── Bloom program ─────────────────────────────────────────────────
        let bloom_program    = build_program(gl, QUAD_VERT, BLOOM_FRAG);
        let bloom_u_tex      = gl.get_uniform_location(bloom_program, "u_tex").unwrap();
        let bloom_u_texel    = gl.get_uniform_location(bloom_program, "u_texel").unwrap();
        let bloom_u_radius   = gl.get_uniform_location(bloom_program, "u_radius").unwrap();
        let bloom_u_strength = gl.get_uniform_location(bloom_program, "u_strength").unwrap();

        // ── Copy program ──────────────────────────────────────────────────
        let copy_program = build_program(gl, QUAD_VERT, COPY_FRAG);
        let copy_u_tex   = gl.get_uniform_location(copy_program, "u_tex").unwrap();
        let quad_pos     = gl.get_attrib_location(copy_program, "pos").unwrap();

        // ── Fullscreen quad VBO ───────────────────────────────────────────
        let quad: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
        let quad_vbo = gl.create_buffer().unwrap();
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(quad_vbo));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_u8_slice(&quad), glow::STATIC_DRAW);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);

        // ── Bar data texture (1 × BANDS RGBA, updated each frame) ─────────
        // NEAREST filtering so adjacent bands don't bleed into each other.
        let bar_data_tex = gl.create_texture().unwrap();
        gl.bind_texture(glow::TEXTURE_2D, Some(bar_data_tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D, 0, glow::RGBA as i32,
            BANDS as i32, 1, 0,
            glow::RGBA, glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&vec![0u8; BANDS * 4])),
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        gl.bind_texture(glow::TEXTURE_2D, None);

        // ── Offscreen FBO (sized lazily in render) ────────────────────────
        let fbo     = gl.create_framebuffer().unwrap();
        let fbo_tex = gl.create_texture().unwrap();

        Self {
            seg_program,
            seg_pos,
            seg_u_bars,
            seg_u_n_bands,
            seg_u_n_segs,
            seg_u_width,
            seg_u_height,
            seg_u_sidebar,
            seg_u_color_lo,
            seg_u_color_hi,
            seg_u_color_peak,
            seg_u_color_dim,
            seg_u_bar_gap,
            seg_u_seg_gap_px,
            seg_u_tex_width,
            bar_data_tex,
            n_bands: BANDS, // actual active count set in render
            bloom_program,
            bloom_u_tex,
            bloom_u_texel,
            bloom_u_radius,
            bloom_u_strength,
            copy_program,
            copy_u_tex,
            quad_pos,
            quad_vbo,
            fbo,
            fbo_tex,
            fbo_w: 0,
            fbo_h: 0,
        }
    }

    unsafe fn ensure_fbo(&mut self, gl: &glow::Context, w: u32, h: u32) {
        if self.fbo_w == w && self.fbo_h == h { return; }
        gl.bind_texture(glow::TEXTURE_2D, Some(self.fbo_tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D, 0, glow::RGBA as i32,
            w as i32, h as i32, 0,
            glow::RGBA, glow::UNSIGNED_BYTE,
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
        gl.delete_program(self.seg_program);
        gl.delete_program(self.bloom_program);
        gl.delete_program(self.copy_program);
        gl.delete_buffer(self.quad_vbo);
        gl.delete_texture(self.bar_data_tex);
        gl.delete_framebuffer(self.fbo);
        gl.delete_texture(self.fbo_tex);
    }
}

// ── BarsRenderer ─────────────────────────────────────────────────────────────

pub struct BarsRenderer {
    theme_id: i32,
    bar_gap: f32,
    seg_gap_px: f32,
    seg_count: usize,
    state: Option<GlState>,
}

impl BarsRenderer {
    pub fn new(theme_id: i32, bar_gap: f32, seg_gap_px: f32, seg_count: usize) -> Self {
        Self { theme_id, bar_gap, seg_gap_px, seg_count, state: None }
    }
}

impl SpectrumRenderer for BarsRenderer {
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
        unsafe { render_bars(gl, s, w, h, bands, peaks, self.theme_id, self.bar_gap, self.seg_gap_px, self.seg_count) };
    }
}

// ── Core render logic ─────────────────────────────────────────────────────────

unsafe fn render_bars(
    gl: &glow::Context,
    s: &mut GlState,
    w: u32,
    h: u32,
    bands: &[f32],
    peaks: &[f32],
    theme_id: i32,
    bar_gap: f32,
    seg_gap_px: f32,
    seg_count: usize,
) {
    let n = bands.len().min(peaks.len()).min(BANDS);
    s.ensure_fbo(gl, w, h);
    s.n_bands = n;

    // ── Upload bar data as a 1×n RGBA texture ─────────────────────────────
    // R = bar height (0–255), G = peak height (0–255)
    let mut texels = vec![0u8; BANDS * 4];
    for i in 0..n {
        texels[i * 4    ] = (bands[i].clamp(0.0, 1.0) * 255.0).round() as u8;
        texels[i * 4 + 1] = (peaks[i].clamp(0.0, 1.0) * 255.0).round() as u8;
        texels[i * 4 + 2] = 0;
        texels[i * 4 + 3] = 255;
    }
    gl.bind_texture(glow::TEXTURE_2D, Some(s.bar_data_tex));
    gl.tex_sub_image_2d(
        glow::TEXTURE_2D, 0, 0, 0,
        BANDS as i32, 1,
        glow::RGBA, glow::UNSIGNED_BYTE,
        glow::PixelUnpackData::Slice(Some(&texels)),
    );
    gl.bind_texture(glow::TEXTURE_2D, None);

    let (col_lo, col_hi, col_peak, col_dim) = theme_palette(theme_id);

    // Save the framebuffer Slint is rendering into.
    let prev_fbo = {
        let raw = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
        NonZeroU32::new(raw as u32).map(glow::NativeFramebuffer)
    };

    // ── Pass 1: VFD segments → offscreen FBO ─────────────────────────────
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(s.fbo));
    gl.framebuffer_texture_2d(
        glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0,
        glow::TEXTURE_2D, Some(s.fbo_tex), 0,
    );
    gl.viewport(0, 0, w as i32, h as i32);
    gl.disable(glow::DEPTH_TEST);
    gl.disable(glow::BLEND);
    gl.clear_color(0.0, 0.0, 0.0, 1.0);
    gl.clear(glow::COLOR_BUFFER_BIT);

    gl.use_program(Some(s.seg_program));
    // Bind bar data to texture unit 1, so TEXTURE0 is free for bloom later.
    gl.active_texture(glow::TEXTURE1);
    gl.bind_texture(glow::TEXTURE_2D, Some(s.bar_data_tex));
    gl.uniform_1_i32(Some(&s.seg_u_bars),  1);
    gl.uniform_1_f32(Some(&s.seg_u_n_bands),  n as f32);
    gl.uniform_1_f32(Some(&s.seg_u_n_segs),   seg_count as f32);
    gl.uniform_1_f32(Some(&s.seg_u_width),    w as f32);
    gl.uniform_1_f32(Some(&s.seg_u_height),   h as f32);
    gl.uniform_1_f32(Some(&s.seg_u_sidebar),  SIDEBAR_W);
    gl.uniform_3_f32(Some(&s.seg_u_color_lo),   col_lo[0],   col_lo[1],   col_lo[2]);
    gl.uniform_3_f32(Some(&s.seg_u_color_hi),   col_hi[0],   col_hi[1],   col_hi[2]);
    gl.uniform_3_f32(Some(&s.seg_u_color_peak), col_peak[0], col_peak[1], col_peak[2]);
    gl.uniform_3_f32(Some(&s.seg_u_color_dim),  col_dim[0],  col_dim[1],  col_dim[2]);
    gl.uniform_1_f32(Some(&s.seg_u_bar_gap),    bar_gap);
    gl.uniform_1_f32(Some(&s.seg_u_seg_gap_px), seg_gap_px);
    gl.uniform_1_f32(Some(&s.seg_u_tex_width),  BANDS as f32);

    gl.bind_buffer(glow::ARRAY_BUFFER, Some(s.quad_vbo));
    gl.enable_vertex_attrib_array(s.seg_pos);
    gl.vertex_attrib_pointer_f32(s.seg_pos, 2, glow::FLOAT, false, 0, 0);
    gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
    gl.disable_vertex_attrib_array(s.seg_pos);

    // ── Pass 2: composite onto the original framebuffer ───────────────────
    gl.bind_framebuffer(glow::FRAMEBUFFER, prev_fbo);
    gl.viewport(0, 0, w as i32, h as i32);
    gl.clear_color(0.0, 0.0, 0.0, 1.0);
    gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);

    gl.active_texture(glow::TEXTURE0);
    gl.bind_texture(glow::TEXTURE_2D, Some(s.fbo_tex));

    gl.bind_buffer(glow::ARRAY_BUFFER, Some(s.quad_vbo));
    gl.enable_vertex_attrib_array(s.quad_pos);
    gl.vertex_attrib_pointer_f32(s.quad_pos, 2, glow::FLOAT, false, 0, 0);

    // 2a. Additive bloom → phosphor halo
    gl.enable(glow::BLEND);
    gl.blend_func(glow::SRC_ALPHA, glow::ONE);
    gl.use_program(Some(s.bloom_program));
    gl.uniform_1_i32(Some(&s.bloom_u_tex), 0);
    gl.uniform_2_f32(Some(&s.bloom_u_texel), 1.0 / w as f32, 1.0 / h as f32);
    gl.uniform_1_f32(Some(&s.bloom_u_radius),   5.0);
    gl.uniform_1_f32(Some(&s.bloom_u_strength), 1.2);
    gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

    // 2b. Sharp copy on top
    gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
    gl.use_program(Some(s.copy_program));
    gl.uniform_1_i32(Some(&s.copy_u_tex), 0);
    gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

    // Restore GL state
    gl.disable_vertex_attrib_array(s.quad_pos);
    gl.disable(glow::BLEND);
    gl.bind_buffer(glow::ARRAY_BUFFER, None);
    gl.bind_texture(glow::TEXTURE_2D, None);
    gl.active_texture(glow::TEXTURE1);
    gl.bind_texture(glow::TEXTURE_2D, None);
    gl.active_texture(glow::TEXTURE0);
    gl.use_program(None);
}
