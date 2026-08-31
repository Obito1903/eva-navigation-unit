//! Spinning wireframe nav icons (AUTO car / SYS gear), rendered by Bevy.
//!
//! Two 160×160 transparent-clear targets on [`RenderLayers`] 2 and 3 feed the
//! `auto-icon` / `settings-icon` Slint properties. Each icon keeps its own spin
//! clock that only advances while its view is active, so switching away parks
//! the icon and switching back resumes from the same angle.
//!
//! Unlike the old borrowed-GL-texture path there is no vertical flip to undo:
//! wgpu textures already have Slint's top-left origin.

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ClearColorConfig, Hdr, RenderTarget};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;

use super::models;
use super::{UiState, AUTO_ICON_VIEW, SETTINGS_ICON_VIEW};

/// Pixel size of both icon targets. Square so the wireframe stays isotropic;
/// Slint scales it into the nav button with `image-fit: contain`.
pub(crate) const ICON_SIZE: u32 = 160;

const CAMERA_Z: f32 = 3.2;
const FOV: f32 = 0.927_295_2; // 2 * atan(0.5), matching the background camera

/// How far past 1.0 the icon colour is pushed, so bloom thickens the 1px lines.
const ICON_GAIN: f32 = 4.0;

/// A spinning icon model plus the view index that keeps it animating.
#[derive(Component)]
struct Icon {
    /// `active-view` value this icon belongs to (0 = AUTO, 1 = SYS).
    view: i32,
    /// Accumulated spin time, advanced only while `view` is active.
    time: f32,
}

#[derive(Resource)]
struct IconMaterial(Handle<StandardMaterial>);

pub(super) struct IconsPlugin;

impl Plugin for IconsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, spin);
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let material = materials.add(StandardMaterial {
        // Unlit draws `base_color` directly and ignores `emissive`.
        base_color: Color::WHITE,
        unlit: true,
        ..default()
    });
    commands.insert_resource(IconMaterial(material.clone()));

    let icons = [
        (0, 2usize, AUTO_ICON_VIEW, models::car_icon() as Vec<[f32; 3]>),
        (1, 3usize, SETTINGS_ICON_VIEW, models::gear_icon()),
    ];

    for (view, layer, target, positions) in icons {
        commands.spawn((
            Camera3d::default(),
            Camera {
                // Transparent so the icon blends into the nav button.
                clear_color: ClearColorConfig::Custom(Color::NONE),
                order: 1 + view as isize,
                ..default()
            },
            RenderTarget::TextureView(target),
            Hdr,
            Projection::Perspective(PerspectiveProjection { fov: FOV, ..default() }),
            Tonemapping::TonyMcMapface,
            Bloom::NATURAL,
            Transform::from_xyz(0.0, 0.0, CAMERA_Z).looking_at(Vec3::ZERO, Vec3::Y),
            RenderLayers::layer(layer),
        ));

        commands.spawn((
            Mesh3d(meshes.add(models::line_mesh(positions))),
            MeshMaterial3d(material.clone()),
            Icon { view, time: 0.0 },
            RenderLayers::layer(layer),
        ));
    }
}

fn spin(
    ui: Res<UiState>,
    material: Res<IconMaterial>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut icons: Query<(&mut Icon, &mut Transform)>,
) {
    for (mut icon, mut transform) in &mut icons {
        if ui.active_view == icon.view {
            icon.time += ui.dt;
        }
        // Pure horizontal spin (the background model's X tilt is gated off
        // here, exactly like the old `u_tilt = 0.0` uniform).
        transform.rotation = Quat::from_rotation_y(icon.time * 0.45);
    }

    if let Some(mut material) = materials.get_mut(&material.0) {
        // Icons are deliberately unaffected by `gfx-bg-brightness`.
        let c = ui.icon_color;
        material.base_color = Color::LinearRgba(LinearRgba::rgb(
            c.red * ICON_GAIN,
            c.green * ICON_GAIN,
            c.blue * ICON_GAIN,
        ));
    }
}
