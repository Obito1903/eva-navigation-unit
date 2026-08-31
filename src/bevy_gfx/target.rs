//! Colour targets shared, zero-copy, between Bevy and Slint.
//!
//! One `wgpu::Texture` is allocated on the device both renderers share: Bevy
//! draws into it through a [`ManualTextureView`], Slint samples it as a
//! [`slint::Image`]. No pixels are ever read back or copied.

use bevy::math::UVec2;
use bevy::render::render_resource::TextureView;
use bevy::render::renderer::RenderDevice;
use bevy::render::texture::ManualTextureView;
use slint::wgpu_29::wgpu;

/// Slint only imports `Rgba8Unorm`/`Rgba8UnormSrgb` textures, and this is also
/// the format Bevy's manual texture views default to.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

pub(crate) struct SharedTexture {
    view: TextureView,
    image: slint::Image,
    size: UVec2,
}

impl SharedTexture {
    pub(crate) fn new(device: &RenderDevice, label: &str, size: UVec2) -> Self {
        let size = size.max(UVec2::ONE);
        let texture = device.wgpu_device().create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.x,
                height: size.y,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            // Slint rejects the import without both of these.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = TextureView::from(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        let image = slint::Image::try_from(texture)
            .expect("wgpu texture rejected by Slint (format/usage mismatch)");
        Self { view, image, size }
    }

    pub(crate) fn manual_view(&self) -> ManualTextureView {
        ManualTextureView {
            texture_view: self.view.clone(),
            size: self.size,
            view_format: FORMAT,
        }
    }

    pub(crate) fn image(&self) -> slint::Image {
        self.image.clone()
    }

    pub(crate) fn size(&self) -> UVec2 {
        self.size
    }
}
