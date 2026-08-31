//! Modular spectrum visualizer.
//!
//! The pipeline is layered so that only the last step touches the GPU:
//!
//! ```text
//! audio -> SpectrumProcessor -> SpectrumFrame   (model.rs)   generic band data
//!                            -> SegmentField    (field.rs)   per-segment state
//!       Scene { layout, style } -> DrawList     (instance.rs) backend-agnostic
//!                            -> VizBackend      (backend/)   GL today
//! ```
//!
//! Everything above [`backend`] is pure CPU and unit-tested. Adding a
//! visualizer means adding a [`scene::Scene`], not a renderer.

pub mod backend;
pub mod field;
pub mod instance;
pub mod layout;
pub mod library;
pub mod model;
pub mod scene;
pub mod spectrum_proc;
pub mod style;

pub use scene::Scene;
pub use spectrum_proc::SpectrumProcessor;

use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

use glow::HasContext;

use backend::{Frame, GlBackend, VizBackend};
use field::SegmentField;
use instance::{DrawList, SegmentInstance};
use layout::LayoutCtx;
use library::SceneLibrary;
use scene::SceneDef;

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

// ── Scene evaluation ──────────────────────────────────────────────────────────

/// Flattens a scene plus the current segment state into a draw list.
///
/// Pure and backend-free, so a scene's output can be asserted without a GL
/// context.
pub fn build_draw_list(
    scene: &Scene,
    field: &SegmentField,
    frame: &model::SpectrumFrame,
    ctx: &LayoutCtx,
    out: &mut DrawList,
) {
    out.begin(scene.background, scene.postfx);
    for band in frame.bands.iter().take(field.band_count()) {
        for seg in field.band(band.index) {
            let placement = scene.layout.place(band, seg, ctx);
            let (color, glow) = scene.style.shade(seg, band);
            if color[3] <= 0.0 {
                continue;
            }
            out.push(SegmentInstance {
                center: placement.center,
                half_size: placement.half_size,
                rotation: placement.rotation,
                color,
                glow,
                shape: scene.style.shape,
            });
        }
    }
}

// ── VisualizerSystem ──────────────────────────────────────────────────────────

pub struct VisualizerSystem {
    processor: SpectrumProcessor,
    library: SceneLibrary,
    scenes: Vec<Scene>,
    field: SegmentField,
    list: DrawList,
    renderer: Box<dyn VizBackend>,
    current_id: i32,
    current_theme: i32,
    pub pending_id: Arc<AtomicI32>,
    pub pending_theme: Arc<AtomicI32>,
}

impl VisualizerSystem {
    pub fn new(
        gl: Rc<glow::Context>,
        w: u32,
        h: u32,
        consumer: crate::spectrum::AudioConsumer,
        pending_id: Arc<AtomicI32>,
        pending_theme: Arc<AtomicI32>,
        viz: &crate::config::VizConfig,
    ) -> Self {
        let current_id = pending_id.load(Ordering::Relaxed);
        let current_theme = pending_theme.load(Ordering::Relaxed);
        // seg_gap_px is authored in pixels; the fallback scene wants a slot
        // fraction. File-authored scenes specify the fraction directly.
        let seg_gap = seg_gap_fraction(viz.seg_gap_px, viz.seg_count, h);
        let fallback = SceneDef::builtin(viz.bar_gap, seg_gap, viz.seg_count);
        let library = SceneLibrary::new(viz.scene_dir.clone(), fallback);
        let scenes = build_scenes(&library, current_theme);

        let processor = SpectrumProcessor::new(consumer, viz);
        let active = pick(&scenes, current_id);
        let field = SegmentField::new(viz.bands, active.field);

        let mut renderer = Box::new(GlBackend::new(gl));
        renderer.setup(Frame {
            width: w,
            height: h,
            viewport: active.insets.apply(w, h),
        });

        Self {
            processor,
            library,
            scenes,
            field,
            list: DrawList::new(),
            renderer,
            current_id,
            current_theme,
            pending_id,
            pending_theme,
        }
    }

    pub fn render_frame(&mut self, w: u32, h: u32) {
        let new_id = self.pending_id.load(Ordering::Relaxed);
        let new_theme = self.pending_theme.load(Ordering::Relaxed);

        // A theme change or a scene-file reload rebuilds the scene list; a
        // scene selection change only reselects within it.
        if self.library.poll() || new_theme != self.current_theme {
            self.scenes = build_scenes(&self.library, new_theme);
            self.current_theme = new_theme;
        }
        if new_id != self.current_id {
            self.current_id = new_id;
            log::info!("viz: scene -> {}", pick(&self.scenes, new_id).id);
        }

        self.processor.process();
        let frame = self.processor.frame();

        let scene = pick(&self.scenes, self.current_id);
        if self.field.seg_count() != scene.field.seg_count
            || self.field.band_count() != frame.len()
        {
            self.field = SegmentField::new(frame.len(), scene.field);
        }
        self.field.update(frame);

        let viewport = scene.insets.apply(w, h);
        let ctx = LayoutCtx {
            aspect: viewport.aspect(),
            band_count: frame.len(),
            seg_count: self.field.seg_count(),
            elapsed: frame.elapsed,
        };
        build_draw_list(scene, &self.field, frame, &ctx, &mut self.list);
        self.renderer.draw(&self.list, Frame { width: w, height: h, viewport });
    }

    pub fn teardown(&mut self) {
        self.renderer.teardown();
    }
}

fn pick(scenes: &[Scene], id: i32) -> &Scene {
    let index = usize::try_from(id).unwrap_or(0).min(scenes.len().saturating_sub(1));
    &scenes[index]
}

fn build_scenes(library: &SceneLibrary, theme_id: i32) -> Vec<Scene> {
    library.defs().iter().cloned().map(|def| def.into_scene(theme_id)).collect()
}

/// Converts an authored pixel gap into a fraction of the segment slot.
fn seg_gap_fraction(seg_gap_px: f32, seg_count: usize, height: u32) -> f32 {
    let slot_px = height as f32 / seg_count.max(1) as f32;
    if slot_px > 0.0 {
        (seg_gap_px / slot_px).clamp(0.0, 0.9)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use field::SegmentFieldConfig;
    use model::{Band, SpectrumFrame};

    fn frame_at(level: f32, count: usize) -> SpectrumFrame {
        let bands = (0..count)
            .map(|index| Band {
                index,
                band_t: model::normalized_index(index, count),
                level,
                peak: level,
                ..Band::default()
            })
            .collect();
        SpectrumFrame { bands, dt: 0.016, elapsed: 0.0 }
    }

    fn ctx(bands: usize, segs: usize) -> LayoutCtx {
        LayoutCtx { aspect: 2.0, band_count: bands, seg_count: segs, elapsed: 0.0 }
    }

    #[test]
    fn silence_still_draws_the_unlit_grid() {
        let scene = SceneDef::builtin(0.15, 0.25, 8).into_scene(0);
        let mut f = SegmentField::new(4, scene.field);
        let frame = frame_at(0.0, 4);
        f.update(&frame);
        let mut list = DrawList::new();
        build_draw_list(&scene, &f, &frame, &ctx(4, 8), &mut list);
        // The whole silhouette stays visible: every segment is emitted.
        assert_eq!(list.len(), 4 * 8);
    }

    #[test]
    fn instance_count_matches_the_field() {
        let scene = SceneDef::builtin(0.15, 0.25, 16).into_scene(0);
        let mut f = SegmentField::new(8, scene.field);
        let frame = frame_at(1.0, 8);
        f.update(&frame);
        let mut list = DrawList::new();
        build_draw_list(&scene, &f, &frame, &ctx(8, 16), &mut list);
        assert_eq!(list.len(), 8 * 16);
    }

    #[test]
    fn fully_transparent_segments_are_skipped() {
        let mut scene = SceneDef::builtin(0.15, 0.25, 8).into_scene(0);
        scene.style.unlit = [0.0, 0.0, 0.0, 0.0];
        let mut f = SegmentField::new(4, scene.field);
        let frame = frame_at(0.0, 4);
        f.update(&frame);
        let mut list = DrawList::new();
        build_draw_list(&scene, &f, &frame, &ctx(4, 8), &mut list);
        assert!(list.is_empty());
    }

    #[test]
    fn every_builtin_theme_produces_finite_geometry() {
        let frame = frame_at(0.7, 12);
        for theme_id in 0..4 {
            let scene = SceneDef::builtin(0.15, 0.25, 20).into_scene(theme_id);
            let mut f = SegmentField::new(12, scene.field);
            f.update(&frame);
            let mut list = DrawList::new();
            build_draw_list(&scene, &f, &frame, &ctx(12, 20), &mut list);
            assert!(!list.is_empty(), "theme {theme_id} drew nothing");
            for i in &list.instances {
                assert!(i.center.iter().all(|v| v.is_finite()), "theme {theme_id}");
                assert!(
                    i.half_size.iter().all(|v| v.is_finite() && *v > 0.0),
                    "theme {theme_id} produced a degenerate quad"
                );
                assert!(i.rotation.is_finite(), "theme {theme_id}");
            }
        }
    }

    #[test]
    fn draw_list_is_reused_across_frames() {
        let scene = SceneDef::builtin(0.15, 0.25, 8).into_scene(0);
        let mut f = SegmentField::new(4, scene.field);
        let frame = frame_at(1.0, 4);
        f.update(&frame);
        let mut list = DrawList::new();
        build_draw_list(&scene, &f, &frame, &ctx(4, 8), &mut list);
        let first = list.len();
        build_draw_list(&scene, &f, &frame, &ctx(4, 8), &mut list);
        assert_eq!(list.len(), first, "instances must not accumulate");
    }

    #[test]
    fn pick_clamps_out_of_range_ids() {
        let scenes: Vec<Scene> = vec![
            SceneDef::builtin(0.15, 0.25, 8).into_scene(0),
            SceneDef { id: "second".into(), ..SceneDef::builtin(0.15, 0.25, 8) }.into_scene(0),
        ];
        assert_eq!(pick(&scenes, -1).id, scenes[0].id);
        assert_eq!(pick(&scenes, 999).id, scenes[scenes.len() - 1].id);
    }

    #[test]
    fn seg_gap_fraction_is_bounded() {
        assert_eq!(seg_gap_fraction(4.0, 50, 0), 0.0);
        assert_eq!(seg_gap_fraction(1e6, 50, 720), 0.9);
        // A zero segment count must not divide by zero.
        assert!(seg_gap_fraction(4.0, 0, 720).is_finite());
        let g = seg_gap_fraction(4.0, 50, 720);
        assert!((0.0..=0.9).contains(&g));
    }

    #[test]
    fn field_is_rebuilt_when_the_band_count_changes() {
        let cfg = SegmentFieldConfig { seg_count: 8, ..Default::default() };
        let mut f = SegmentField::new(4, cfg);
        assert_eq!(f.band_count(), 4);
        f = SegmentField::new(16, cfg);
        assert_eq!(f.band_count(), 16);
        f.update(&frame_at(1.0, 16));
        assert!(f.band(15).iter().any(|s| s.lit));
    }
}
