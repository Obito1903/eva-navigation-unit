//! The animated wireframe background (`gfx-model`), rendered by Bevy.
//!
//! One perspective camera on [`RenderLayers`] 1 draws the selected wireframe
//! model into the content-area-sized [`BG_VIEW`] target. `gfx-frost-enabled`
//! adds [`Frost`], a fullscreen post-process that ports the old GL frosted-glass
//! pass (5×5 gaussian blur, cool tint lift, faint haze) on top of HDR bloom.

use bevy::asset::embedded_asset;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ClearColorConfig, Hdr, RenderTarget};
use bevy::core_pipeline::fullscreen_material::{FullscreenMaterial, FullscreenMaterialPlugin};
use bevy::core_pipeline::tonemapping::{tonemapping, Tonemapping};
use bevy::core_pipeline::Core3dSystems;
use bevy::ecs::schedule::{IntoScheduleConfigs, ScheduleConfigs};
use bevy::ecs::system::BoxedSystem;
use bevy::post_process::bloom::{bloom, Bloom};
use bevy::prelude::*;
use bevy::render::extract_component::ExtractComponent;
use bevy::render::render_resource::ShaderType;
use bevy::shader::ShaderRef;

use super::models;
use super::{UiState, BG_VIEW};

/// Layer the background camera and its models live on.
const LAYER: usize = 1;

/// Camera distance and vertical FOV chosen to frame a unit-radius model the
/// same way the old `pos.xy * 2.0 / (pos.z + 3.2)` vertex shader did.
const CAMERA_Z: f32 = 3.2;
const FOV: f32 = 0.927_295_2; // 2 * atan(0.5)

/// Cool tint the frost pass lifts the wireframe toward.
const FROST_TINT: Vec3 = Vec3::new(0.72, 0.80, 0.88);

/// Blur spread in texels, and how hard bright areas are pulled to the tint.
/// Both come straight from the old GL frost shader.
const FROST_RADIUS: f32 = 4.0;
const FROST_STRENGTH: f32 = 0.35;
const FROST_HAZE: f32 = 0.04;

/// How far past 1.0 the wireframe colour is pushed at brightness 1.0, so bloom
/// gives it a halo on top of the frost blur.
const WIREFRAME_GAIN: f32 = 4.0;

/// Fullscreen frosted-glass pass applied to the background target.
///
/// Runs after bloom and before tonemapping, so it blurs in HDR and its output
/// is tonemapped like the rest of the scene.
#[derive(Component, ExtractComponent, ShaderType, Clone, Copy)]
pub(super) struct Frost {
    tint: Vec3,
    radius: f32,
    strength: f32,
    haze: f32,
    _padding: Vec2,
}

impl Default for Frost {
    fn default() -> Self {
        Self {
            tint: FROST_TINT,
            radius: FROST_RADIUS,
            strength: FROST_STRENGTH,
            haze: FROST_HAZE,
            _padding: Vec2::ZERO,
        }
    }
}

impl FullscreenMaterial for Frost {
    fn fragment_shader() -> ShaderRef {
        "embedded://eva_navigation_unit/bevy_gfx/frost.wgsl".into()
    }

    fn schedule_configs(system: ScheduleConfigs<BoxedSystem>) -> ScheduleConfigs<BoxedSystem> {
        // Explicit order: blur bloom's output, then let tonemapping run.
        system
            .in_set(Core3dSystems::PostProcess)
            .after(bloom)
            .before(tonemapping)
    }
}

/// Marks the entity every background model hangs off, so one transform spins
/// them all.
#[derive(Component)]
pub(super) struct Spinner;

/// The `gfx-model` index this entity renders.
#[derive(Component)]
pub(super) struct ModelIndex(pub i32);

/// The material shared by every background model, kept so the per-frame system
/// can retint it without a query.
#[derive(Resource)]
struct BackgroundMaterial(Handle<StandardMaterial>);

pub(super) struct BackgroundPlugin;

impl Plugin for BackgroundPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "frost.wgsl");
        app.add_plugins(FullscreenMaterialPlugin::<Frost>::default())
            .add_systems(Startup, setup)
            .add_systems(Update, (sync_camera, sync_models).chain());
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let material = materials.add(StandardMaterial {
        // Unlit: `base_color` is the shader's whole output, so the wireframe
        // colour (and its HDR overdrive for bloom) lives there, not in
        // `emissive` — which the unlit path ignores.
        base_color: Color::WHITE,
        unlit: true,
        ..default()
    });
    commands.insert_resource(BackgroundMaterial(material.clone()));

    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            order: 0,
            ..default()
        },
        RenderTarget::TextureView(BG_VIEW),
        Hdr,
        Projection::Perspective(PerspectiveProjection { fov: FOV, ..default() }),
        Tonemapping::TonyMcMapface,
        Bloom::NATURAL,
        Transform::from_xyz(0.0, 0.0, CAMERA_Z).looking_at(Vec3::ZERO, Vec3::Y),
        RenderLayers::layer(LAYER),
    ));

    // One spinner parent so a single transform drives whichever model is shown.
    let spinner = commands
        .spawn((Spinner, Transform::IDENTITY, Visibility::Visible))
        .id();

    // Order must match the `gfx-model` selector: 0 = sphere, 1 = cube,
    // 2 = car, 3 = speaker.
    for (index, positions) in [
        models::sphere(),
        models::cube(),
        models::car(),
        models::speaker(),
    ]
    .into_iter()
    .enumerate()
    {
        commands.spawn((
            Mesh3d(meshes.add(models::line_mesh(positions))),
            MeshMaterial3d(material.clone()),
            ModelIndex(index as i32),
            Visibility::Hidden,
            RenderLayers::layer(LAYER),
            ChildOf(spinner),
        ));
    }
}

/// Enable the camera only while the background is actually on screen, and add
/// or remove the frosted-glass pass to match `gfx-frost-enabled`.
fn sync_camera(
    mut commands: Commands,
    ui: Res<UiState>,
    mut cameras: Query<(Entity, &RenderTarget, &mut Camera, &mut Bloom, Has<Frost>)>,
) {
    for (entity, target, mut camera, mut bloom, has_frost) in &mut cameras {
        if !matches!(target, RenderTarget::TextureView(BG_VIEW)) {
            continue;
        }
        camera.is_active = ui.bg_enabled && ui.active_view != 3;

        if ui.frost_enabled != has_frost {
            if ui.frost_enabled {
                commands.entity(entity).insert(Frost::default());
            } else {
                commands.entity(entity).remove::<Frost>();
            }
        }

        // Without frost the wireframe is drawn sharp, so keep the halo tight;
        // with frost the blur already spreads it and bloom only needs to lift
        // the brightest strokes.
        bloom.intensity = if ui.frost_enabled { 0.25 } else { 0.15 };
    }
}

fn sync_models(
    ui: Res<UiState>,
    material: Res<BackgroundMaterial>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut spinner: Query<&mut Transform, With<Spinner>>,
    mut models: Query<(&ModelIndex, &mut Visibility)>,
) {
    // Same two-axis spin the old vertex shader applied: `rx(b) * ry(a) * pos`.
    let a = ui.elapsed * 0.45;
    let b = ui.elapsed * 0.17 + 0.5;
    if let Ok(mut transform) = spinner.single_mut() {
        transform.rotation = Quat::from_rotation_x(b) * Quat::from_rotation_y(a);
    }

    let selected = ui.model.max(0);
    for (index, mut visibility) in &mut models {
        *visibility = if index.0 == selected {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }

    if let Some(mut material) = materials.get_mut(&material.0) {
        // The frost pass does the tinting now, so the model just carries the
        // theme accent. Push past 1.0 so `gfx-bg-brightness` drives it into the
        // bloom threshold rather than only tinting it.
        let accent = ui.accent;
        let gain = WIREFRAME_GAIN * ui.brightness.max(0.0);
        material.base_color = Color::LinearRgba(LinearRgba::rgb(
            accent.red * gain,
            accent.green * gain,
            accent.blue * gain,
        ));
    }
}
