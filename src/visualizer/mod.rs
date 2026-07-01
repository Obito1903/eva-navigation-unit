//! Pluggable spectrum renderer system.
//!
//! [`SpectrumProcessor`] (spectrum_proc.rs) runs on the render thread each
//! frame. [`VisualizerSystem`] owns the processor and the active renderer,
//! and hot-swaps renderers when the user changes style or theme.

pub mod spectrum_proc;
pub use spectrum_proc::SpectrumProcessor;

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

use glow::HasContext;

mod arc;
mod bars;
pub use arc::ArcRenderer;
pub use bars::BarsRenderer;

pub const SIDEBAR_W: f32 = 96.0;

// ── Shared GL helpers ─────────────────────────────────────────────────────────

pub(super) unsafe fn build_program(gl: &glow::Context, vs: &str, fs: &str) -> glow::Program {
    unsafe {
        let program = gl.create_program().expect("create_program");
        let mut shaders = Vec::with_capacity(2);
        for (kind, src) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
            let shader = gl.create_shader(kind).expect("create_shader");
            gl.shader_source(shader, src);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                panic!("viz shader compile: {}", gl.get_shader_info_log(shader));
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            panic!("viz program link: {}", gl.get_program_info_log(program));
        }
        for s in shaders { gl.detach_shader(program, s); gl.delete_shader(s); }
        program
    }
}

pub(super) fn as_u8_slice(data: &[f32]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(data.as_ptr() as *const u8, std::mem::size_of_val(data))
    }
}

// ── Shared GLSL sources ───────────────────────────────────────────────────────

pub(super) const BAR_VERT: &str = r"#version 100
attribute vec2 a_pos;
attribute vec4 a_color;
varying vec4 v_color;
void main() {
    v_color = a_color;
    gl_Position = vec4(a_pos, 0.0, 1.0);
}
";

pub(super) const BAR_FRAG: &str = r"#version 100
precision mediump float;
varying vec4 v_color;
void main() {
    float row = mod(floor(gl_FragCoord.y / 2.0), 2.0);
    gl_FragColor = vec4(v_color.rgb, v_color.a * (1.0 - row * 0.20));
}
";

pub(super) const QUAD_VERT: &str = r"#version 100
attribute vec2 pos;
varying vec2 v_uv;
void main() {
    v_uv = pos * 0.5 + 0.5;
    gl_Position = vec4(pos, 0.0, 1.0);
}
";

pub(super) const BLOOM_FRAG: &str = r"#version 100
precision mediump float;
uniform sampler2D u_tex;
uniform vec2 u_texel;
uniform float u_radius;
uniform float u_strength;
varying vec2 v_uv;
void main() {
    vec4 sum = vec4(0.0);
    float total = 0.0;
    for (int x = -1; x <= 1; x++) {
        for (int y = -1; y <= 1; y++) {
            vec2 off = vec2(float(x), float(y)) * u_texel * u_radius;
            float w = 1.0 / (1.0 + float(x * x + y * y));
            sum += texture2D(u_tex, v_uv + off) * w;
            total += w;
        }
    }
    gl_FragColor = (sum / total) * u_strength;
}
";

pub(super) const COPY_FRAG: &str = r"#version 100
precision mediump float;
uniform sampler2D u_tex;
varying vec2 v_uv;
void main() {
    gl_FragColor = texture2D(u_tex, v_uv);
}
";

pub(super) const GRID_FRAG: &str = r"#version 100
precision mediump float;
uniform vec3 u_grid_color;
uniform float u_width;
uniform float u_height;
varying vec2 v_uv;
void main() {
    vec2 pos = v_uv * vec2(u_width, u_height);
    float sp = 8.0;
    vec2 cell = mod(pos, sp);
    float d = length(cell - vec2(sp * 0.5));
    float dot_val = 1.0 - smoothstep(0.7, 1.5, d);
    gl_FragColor = vec4(u_grid_color * dot_val, dot_val * 0.28);
}
";

// ── SpectrumRenderer trait ────────────────────────────────────────────────────

/// A self-contained OpenGL spectrum renderer. Receives pre-processed band
/// heights and peak positions from `SpectrumProcessor` each frame.
pub trait SpectrumRenderer {
    fn setup(&mut self, gl: &glow::Context, w: u32, h: u32);
    fn render(&mut self, gl: &glow::Context, w: u32, h: u32,
              bands: &[f32], peaks: &[f32]);
    fn resize(&mut self, gl: &glow::Context, w: u32, h: u32);
    fn teardown(&mut self, gl: &glow::Context);
}

pub fn make_renderer(renderer_id: i32, theme_id: i32, bar_gap: f32, seg_gap_px: f32) -> Box<dyn SpectrumRenderer> {
    match renderer_id {
        1 => Box::new(ArcRenderer::new(theme_id)),
        _ => Box::new(BarsRenderer::new(theme_id, bar_gap, seg_gap_px)),
    }
}

// ── VisualizerSystem ──────────────────────────────────────────────────────────

pub struct VisualizerSystem {
    processor:     SpectrumProcessor,
    active:        Box<dyn SpectrumRenderer>,
    current_id:    i32,
    current_theme: i32,
    pub pending_id:    Arc<AtomicI32>,
    pub pending_theme: Arc<AtomicI32>,
    bar_gap:    f32,
    seg_gap_px: f32,
}

impl VisualizerSystem {
    pub fn new(
        gl:            &glow::Context,
        w:             u32,
        h:             u32,
        consumer:      crate::spectrum::AudioConsumer,
        pending_id:    Arc<AtomicI32>,
        pending_theme: Arc<AtomicI32>,
        viz:           &crate::config::VizConfig,
    ) -> Self {
        let initial_id    = pending_id.load(Ordering::Relaxed);
        let initial_theme = pending_theme.load(Ordering::Relaxed);
        let bar_gap    = viz.bar_gap;
        let seg_gap_px = viz.seg_gap_px;
        let mut renderer  = make_renderer(initial_id, initial_theme, bar_gap, seg_gap_px);
        renderer.setup(gl, w, h);
        Self {
            processor:     SpectrumProcessor::new(consumer, viz),
            active:        renderer,
            current_id:    initial_id,
            current_theme: initial_theme,
            pending_id,
            pending_theme,
            bar_gap,
            seg_gap_px,
        }
    }

    pub fn render_frame(&mut self, gl: &glow::Context, w: u32, h: u32) {
        let new_id    = self.pending_id.load(Ordering::Relaxed);
        let new_theme = self.pending_theme.load(Ordering::Relaxed);

        if new_id != self.current_id || new_theme != self.current_theme {
            self.active.teardown(gl);
            self.active        = make_renderer(new_id, new_theme, self.bar_gap, self.seg_gap_px);
            self.active.setup(gl, w, h);
            self.current_id    = new_id;
            self.current_theme = new_theme;
        }

        self.processor.process();
        self.active.render(gl, w, h, &self.processor.bands, &self.processor.peaks);
    }

    pub fn teardown(&mut self, gl: &glow::Context) {
        self.active.teardown(gl);
    }
}
