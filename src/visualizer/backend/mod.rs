//! Rendering backends.
//!
//! A backend consumes a [`DrawList`] and knows nothing about audio, bands,
//! layouts or scenes. Keeping the contract this narrow is what makes the
//! renderer swappable — a wgpu/Bevy backend implements the same three methods.

use super::instance::{DrawList, Viewport};

pub mod gl;
pub use gl::GlBackend;

/// The render target plus the sub-rect the visualizer occupies within it.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub viewport: Viewport,
}

pub trait VizBackend {
    /// Allocates GPU resources. Called once, and again after a context loss.
    fn setup(&mut self, frame: Frame);
    fn draw(&mut self, list: &DrawList, frame: Frame);
    fn teardown(&mut self);
}
