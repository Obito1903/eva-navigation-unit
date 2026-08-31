//! Backend-agnostic draw list.
//!
//! This is the entire surface a rendering backend must implement. Layout and
//! style flatten the segment field into [`SegmentInstance`]s; a backend turns
//! those into draw calls and knows nothing about audio, bands or scenes.

/// Shape carved out of a segment's quad by the backend.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SegmentShape {
    #[default]
    Rect,
    /// Classic VFD triple element: block, spine, block.
    Vfd,
}

impl SegmentShape {
    /// Discriminant handed to the shader; must match the branch order there.
    pub fn id(self) -> f32 {
        match self {
            Self::Rect => 0.0,
            Self::Vfd => 1.0,
        }
    }
}

/// Pixel rectangle the visualizer draws into, excluding sidebar and HUD.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Viewport {
    pub fn aspect(&self) -> f32 {
        if self.h > 0.0 {
            self.w / self.h
        } else {
            1.0
        }
    }
}

/// One drawable element.
///
/// `center` is normalised per axis: `(0, 0)` top-left, `(1, 1)` bottom-right.
/// `half_size` is normalised to the viewport **height on both axes**, which
/// keeps it isotropic so that a rotated quad is not sheared by a non-square
/// viewport. A layout converts a width fraction `f` into these units with
/// `f * aspect`.
#[derive(Debug, Clone, Copy)]
pub struct SegmentInstance {
    pub center: [f32; 2],
    pub half_size: [f32; 2],
    /// Clockwise in screen space, radians.
    pub rotation: f32,
    /// Straight (non-premultiplied) linear RGBA.
    pub color: [f32; 4],
    /// Additive brightness boost fed into the bloom pass.
    pub glow: f32,
    pub shape: SegmentShape,
}

impl Default for SegmentInstance {
    fn default() -> Self {
        Self {
            center: [0.5, 0.5],
            half_size: [0.01, 0.01],
            rotation: 0.0,
            color: [1.0; 4],
            glow: 0.0,
            shape: SegmentShape::Rect,
        }
    }
}

/// Full-frame effects applied after the instances are drawn.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct PostFx {
    pub bloom_strength: f32,
    pub bloom_radius: f32,
    /// Dot-grid backdrop colour, or `None` to skip the pass.
    pub grid: Option<[f32; 3]>,
    /// Scanline darkening, `0..=1`.
    pub scanline: f32,
}

impl Default for PostFx {
    fn default() -> Self {
        Self { bloom_strength: 1.0, bloom_radius: 2.0, grid: None, scanline: 0.0 }
    }
}

/// Everything a backend needs to draw one frame.
///
/// Reused across frames: call [`DrawList::begin`] and push into it rather than
/// allocating a new list, so a 60 Hz render loop stays allocation-free.
#[derive(Debug, Default)]
pub struct DrawList {
    pub instances: Vec<SegmentInstance>,
    pub postfx: PostFx,
    pub background: [f32; 4],
}

impl DrawList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears the instances, keeping the allocation, and sets frame state.
    pub fn begin(&mut self, background: [f32; 4], postfx: PostFx) {
        self.instances.clear();
        self.background = background;
        self.postfx = postfx;
    }

    pub fn push(&mut self, instance: SegmentInstance) {
        self.instances.push(instance);
    }

    // Companion to `len`, required by clippy::len_without_is_empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_ids_are_distinct() {
        assert_ne!(SegmentShape::Rect.id(), SegmentShape::Vfd.id());
    }

    #[test]
    fn begin_keeps_capacity() {
        let mut list = DrawList::new();
        list.push(SegmentInstance::default());
        let cap = list.instances.capacity();
        list.begin([0.0; 4], PostFx::default());
        assert!(list.is_empty());
        assert_eq!(list.instances.capacity(), cap);
    }
}
