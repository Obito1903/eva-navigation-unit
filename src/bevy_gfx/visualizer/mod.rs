//! Spectrum visualizer rendered by Bevy into the `viz-texture` target.
//!
//! [`SpectrumProcessor`] still does all the CAVA-style DSP, but it now runs as
//! a Bevy resource driven by a system instead of being called from the GL
//! rendering notifier. The two renderers (`0 = BARS`, `1 = ARC`) are selected
//! by Bevy state rather than an `AtomicI32` hot-swap, and both rebuild a single
//! vertex-coloured mesh each frame; HDR vertex colours plus camera bloom
//! replace the old additive blur passes.

mod arc;
mod bars;
pub(crate) mod spectrum_proc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ClearColorConfig, Hdr, RenderTarget, ScalingMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::math::Affine2;
use bevy::mesh::{Mesh, PrimitiveTopology};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;

use crate::config::VizConfig;
use crate::spectrum::BANDS;

use super::{UiState, VIZ_VIEW};
pub(crate) use spectrum_proc::SpectrumProcessor;

/// Layer the visualizer camera and geometry live on.
const LAYER: usize = 4;

/// Height of the HUD control bar at the bottom of the visualizer view, plus a
/// little breathing room. Bars stop above it (see `ui/views/visualizer.slint`).
const BOTTOM_PAD: f32 = 56.0;
/// Margin above the bars.
const TOP_PAD: f32 = 12.0;

/// Spacing of the ARC renderer's background dot grid, in pixels.
const GRID_SPACING: f32 = 8.0;

/// Which spectrum renderer is live. Ids match `viz-renderer` in the UI and the
/// `[viz]` config, so both stay compatible.
#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum VizRenderer {
    #[default]
    Bars,
    Arc,
}

impl VizRenderer {
    fn from_id(id: i32) -> Self {
        match id {
            1 => Self::Arc,
            _ => Self::Bars,
        }
    }
}

/// Bar/segment geometry knobs from the `[viz]` config section.
#[derive(Resource)]
struct VizSettings {
    bar_gap: f32,
    seg_gap_px: f32,
    seg_count: usize,
}

/// The single mesh both renderers rebuild each frame.
#[derive(Component)]
struct VizMesh;

/// The ARC renderer's tiled dot-grid backdrop.
#[derive(Component)]
struct VizGrid;

#[derive(Resource)]
struct VizMaterials {
    grid: Handle<StandardMaterial>,
}

pub(super) struct VisualizerPlugin {
    pub(super) cfg: VizConfig,
}

impl Plugin for VisualizerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(VizSettings {
            bar_gap: self.cfg.bar_gap,
            seg_gap_px: self.cfg.seg_gap_px,
            seg_count: self.cfg.seg_count,
        })
        .init_state::<VizRenderer>()
        .add_systems(Startup, setup)
        .add_systems(Update, (sync_state, process_audio, rebuild).chain());
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            order: 3,
            is_active: false,
            ..default()
        },
        RenderTarget::TextureView(VIZ_VIEW),
        Hdr,
        // Fixed 2×2 world units so geometry can be built straight in NDC.
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::Fixed { width: 2.0, height: 2.0 },
            ..OrthographicProjection::default_3d()
        }),
        Tonemapping::TonyMcMapface,
        Bloom::NATURAL,
        Transform::from_xyz(0.0, 0.0, 1.0).looking_at(Vec3::ZERO, Vec3::Y),
        RenderLayers::layer(LAYER),
    ));

    let bars = materials.add(StandardMaterial {
        // Intensity lives in the (HDR) vertex colours.
        base_color: Color::WHITE,
        unlit: true,
        ..default()
    });
    let grid = materials.add(StandardMaterial {
        base_color_texture: Some(images.add(dot_tile())),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    commands.insert_resource(VizMaterials { grid: grid.clone() });

    commands.spawn((
        Mesh3d(meshes.add(fullscreen_quad())),
        MeshMaterial3d(grid),
        VizGrid,
        Visibility::Hidden,
        Transform::from_xyz(0.0, 0.0, -0.1),
        RenderLayers::layer(LAYER),
    ));
    commands.spawn((
        Mesh3d(meshes.add(empty_mesh())),
        MeshMaterial3d(bars),
        VizMesh,
        // Nothing to draw until the first rebuild fills the mesh.
        Visibility::Hidden,
        RenderLayers::layer(LAYER),
    ));
}

/// Mirror the Slint `viz-renderer` selector into Bevy state, and gate the
/// camera on the visualizer view actually being on screen.
fn sync_state(
    ui: Res<UiState>,
    current: Res<State<VizRenderer>>,
    mut next: ResMut<NextState<VizRenderer>>,
    mut cameras: Query<(&RenderTarget, &mut Camera)>,
    mut grid: Query<&mut Visibility, With<VizGrid>>,
) {
    let wanted = VizRenderer::from_id(ui.viz_renderer);
    if *current.get() != wanted {
        next.set(wanted);
    }

    let active = ui.active_view == 3;
    for (target, mut camera) in &mut cameras {
        if matches!(target, RenderTarget::TextureView(VIZ_VIEW)) {
            camera.is_active = active;
        }
    }
    if let Ok(mut visibility) = grid.single_mut() {
        *visibility = if active && wanted == VizRenderer::Arc {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

/// Drain the capture ring buffer and run one frame of the CAVA pipeline.
///
/// `SpectrumProcessor` owns a `ringbuf` consumer, which is `Send` but not
/// `Sync`, so it lives as a non-send resource — fine, because the whole Bevy
/// app is driven from Slint's thread.
fn process_audio(ui: Res<UiState>, processor: Option<NonSendMut<SpectrumProcessor>>) {
    if ui.active_view != 3 {
        return;
    }
    if let Some(mut processor) = processor {
        processor.process();
    }
}

#[expect(clippy::too_many_arguments, reason = "one system, many small inputs")]
fn rebuild(
    ui: Res<UiState>,
    renderer: Res<State<VizRenderer>>,
    settings: Res<VizSettings>,
    processor: Option<NonSend<SpectrumProcessor>>,
    materials: Res<VizMaterials>,
    mut assets: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut bars_mesh: Query<(&Mesh3d, &mut Visibility), With<VizMesh>>,
) {
    if ui.active_view != 3 {
        return;
    }
    let Some(processor) = processor else { return };
    let size = ui.viz_size.as_vec2();
    if size.x < 1.0 || size.y < 1.0 {
        return;
    }

    let n = processor.bands.len().min(BANDS);
    let (positions, colors) = match renderer.get() {
        VizRenderer::Bars => bars::build(bars::Params {
            size,
            bands: &processor.bands[..n],
            peaks: &processor.peaks[..n],
            theme_id: ui.viz_theme,
            bar_gap: settings.bar_gap,
            seg_gap_px: settings.seg_gap_px,
            seg_count: settings.seg_count,
            top_pad: TOP_PAD * ui.scale,
            bottom_pad: BOTTOM_PAD * ui.scale,
        }),
        VizRenderer::Arc => arc::build(arc::Params {
            size,
            bands: &processor.bands[..n],
            peaks: &processor.peaks[..n],
            theme_id: ui.viz_theme,
        }),
    };

    if let Ok((Mesh3d(handle), mut visibility)) = bars_mesh.single_mut()
        && let Some(mut mesh) = meshes.get_mut(handle)
    {
        let count = positions.len();
        *visibility = if count == 0 { Visibility::Hidden } else { Visibility::Visible };
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; count]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; count]);
    }

    if *renderer.get() == VizRenderer::Arc {
        // The dot tile repeats through the material's UV transform, so its
        // density stays fixed in pixels however the view is sized.
        if let Some(mut material) = assets.get_mut(&materials.grid) {
            material.uv_transform = Affine2::from_scale(size / (GRID_SPACING * ui.scale));
            material.base_color = Color::LinearRgba(arc::palette(ui.viz_theme).3);
        }
    }
}

fn empty_mesh() -> Mesh {
    // A single degenerate triangle rather than zero vertices: an empty mesh
    // gets no slab allocation, and the mesh allocator then errors when the
    // first rebuild tries to copy data into it. `RenderAssetUsages::default()`
    // (main + render world) because both renderers rewrite this in place every
    // frame; a RENDER_WORLD-only mesh is dropped from the main world after its
    // first upload.
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0_f32, 0.0, 0.0]; 3])
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0_f32, 0.0, 1.0]; 3])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0_f32, 0.0]; 3])
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0_f32, 0.0, 0.0, 0.0]; 3])
}

/// A quad covering the whole target, in the camera's 2×2 NDC world.
fn fullscreen_quad() -> Mesh {
    let positions = vec![
        [-1.0, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
    ];
    let uvs = vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 6])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
}

/// An 8×8 repeating tile holding one soft dot — the ARC renderer's backdrop,
/// previously drawn by `GRID_FRAG`.
fn dot_tile() -> Image {
    let n = GRID_SPACING as usize;
    let mut data = vec![0u8; n * n * 4];
    let center = GRID_SPACING * 0.5;
    for y in 0..n {
        for x in 0..n {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let d = (dx * dx + dy * dy).sqrt();
            // smoothstep(0.7, 1.5, d), inverted.
            let t = ((d - 0.7) / 0.8).clamp(0.0, 1.0);
            let dot = 1.0 - t * t * (3.0 - 2.0 * t);
            let a = (dot * 0.28 * 255.0) as u8;
            let i = (y * n + x) * 4;
            data[i] = 255;
            data[i + 1] = 255;
            data[i + 2] = 255;
            data[i + 3] = a;
        }
    }
    let mut image = Image::new(
        bevy::render::render_resource::Extent3d {
            width: n as u32,
            height: n as u32,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        data,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..default()
    });
    image
}
