//! A visualizer definition.
//!
//! A [`Scene`] is self-contained: it declares its own segment behaviour,
//! layout, style and post effects. Adding a visualizer means adding a scene,
//! not adding a renderer.
//!
//! [`SceneDef`] is the serde schema loaded from `.viz.ron` files (see
//! [`super::library`]) and is also how the built-in fallback scene is
//! expressed, so there is exactly one representation of "what a scene looks
//! like" rather than a Rust copy and a file copy drifting apart. Colour is
//! deliberately not part of it: theme selection stays the existing runtime
//! toggle, applied last via [`SceneDef::into_scene`].

use super::field::{SegmentFieldConfig, ThresholdCurve};
use super::instance::{PostFx, Viewport};
use super::layout::{Baseline, Layout};
use super::style::{builtin_style, Style};

/// Padding carved out of the window before the visualizer is laid out.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Insets {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

impl Insets {
    /// Matches the original VFD framing: sidebar on the left, HUD at the
    /// bottom, small breathing room top and right.
    pub const VFD: Self = Self { left: super::SIDEBAR_W, right: 4.0, top: 12.0, bottom: 56.0 };

    pub fn apply(&self, width: u32, height: u32) -> Viewport {
        let w = (width as f32 - self.left - self.right).max(1.0);
        let h = (height as f32 - self.top - self.bottom).max(1.0);
        Viewport { x: self.left, y: self.top, w, h }
    }
}

impl Default for Insets {
    fn default() -> Self {
        Self::VFD
    }
}

/// A scene as authored: everything except colour, which the theme selector
/// supplies. This is what `.viz.ron` files deserialize into.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SceneDef {
    pub id: String,
    #[serde(default)]
    pub field: SegmentFieldConfig,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub postfx: PostFx,
    #[serde(default)]
    pub insets: Insets,
    #[serde(default)]
    pub background: [f32; 4],
}

impl SceneDef {
    /// The scene used when the scene directory has no valid `.viz.ron`
    /// files, parameterised by the existing `[viz]` config knobs so
    /// behaviour is unchanged for anyone who hasn't added scene files yet.
    pub fn builtin(bar_gap: f32, seg_gap: f32, seg_count: usize) -> Self {
        Self {
            id: "vfd_bars".into(),
            field: SegmentFieldConfig {
                seg_count,
                decay_ms: 0.0,
                curve: ThresholdCurve::Linear,
            },
            layout: Layout::Columns { bar_gap, seg_gap, baseline: Baseline::Bottom },
            postfx: PostFx { bloom_strength: 1.0, bloom_radius: 2.0, grid: None, scanline: 0.0 },
            insets: Insets::VFD,
            background: [0.0, 0.0, 0.0, 1.0],
        }
    }

    /// Attaches a theme's colours, producing a renderable [`Scene`].
    pub fn into_scene(self, theme_id: i32) -> Scene {
        Scene {
            id: self.id,
            field: self.field,
            layout: self.layout,
            style: builtin_style(theme_id),
            postfx: self.postfx,
            insets: self.insets,
            background: self.background,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Scene {
    pub id: String,
    pub field: SegmentFieldConfig,
    pub layout: Layout,
    pub style: Style,
    pub postfx: PostFx,
    pub insets: Insets,
    pub background: [f32; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insets_shrink_the_viewport() {
        let vp = Insets::VFD.apply(1280, 720);
        assert_eq!(vp.x, super::super::SIDEBAR_W);
        assert_eq!(vp.y, 12.0);
        assert!((vp.w - (1280.0 - super::super::SIDEBAR_W - 4.0)).abs() < 1e-6);
        assert!((vp.h - (720.0 - 12.0 - 56.0)).abs() < 1e-6);
    }

    #[test]
    fn insets_never_produce_a_degenerate_viewport() {
        let vp = Insets::VFD.apply(1, 1);
        assert!(vp.w >= 1.0 && vp.h >= 1.0);
        assert!(vp.aspect().is_finite());
    }

    #[test]
    fn builtin_round_trips_through_ron() {
        let def = SceneDef::builtin(0.15, 0.25, 32);
        let text = ron::to_string(&def).expect("serialize");
        let back: SceneDef = ron::from_str(&text).expect("deserialize");
        assert_eq!(back.id, def.id);
    }

    #[test]
    fn a_minimal_file_only_needs_an_id() {
        let def: SceneDef = ron::from_str("(id: \"my_scene\")").expect("minimal scene parses");
        assert_eq!(def.id, "my_scene");
        assert_eq!(def.background, [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn into_scene_attaches_the_requested_theme() {
        let scene = SceneDef::builtin(0.15, 0.25, 32).into_scene(2);
        assert_eq!(scene.style.gradient.sample(1.0), builtin_style(2).gradient.sample(1.0));
    }

    /// Guards the shipped starter scene against RON syntax drift: this is
    /// the only thing that actually exercises hand-written RON syntax rather
    /// than a struct serialized by `ron::to_string`.
    #[test]
    fn the_shipped_vfd_bars_scene_parses() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes/vfd_bars.viz.ron");
        let text = std::fs::read_to_string(&path).expect("scenes/vfd_bars.viz.ron must exist");
        let def: SceneDef = ron::from_str(&text).expect("scenes/vfd_bars.viz.ron must parse");
        assert_eq!(def.id, "vfd_bars");
        assert_eq!(def.field.seg_count, 50);
    }
}
