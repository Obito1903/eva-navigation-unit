//! Bevy-backed GPU graphics, drawn into textures that Slint displays.
//!
//! Bevy runs *headless* on Slint's own wgpu device. [`SharedRenderer`] builds
//! the instance/adapter/device/queue through Bevy's `initialize_renderer` (so
//! Bevy's feature and limit requirements are satisfied by construction), hands
//! them to Slint via `WGPUConfiguration::Manual`, and hands the very same
//! objects to Bevy's `RenderPlugin` via `RenderCreation::Manual`. Nothing is
//! copied between the two: Bevy renders into `wgpu::Texture`s that become
//! `slint::Image`s through `Image::try_from`.
//!
//! Bevy never owns the event loop or a window. [`install`] drives `app.update()`
//! from Slint's `BeforeRendering` notification, so Bevy's command submissions
//! always land on the shared queue ahead of Slint's for the same frame and no
//! explicit fencing is needed.
//!
//! Layout: three independent targets, each with its own camera and render
//! layer — the background model, the two spinning nav icons, and the spectrum
//! visualizer.

mod background;
mod icons;
mod models;
mod target;
mod visualizer;

use std::time::Instant;

use bevy::app::{PanicHandlerPlugin, PluginGroup, TerminalCtrlCHandlerPlugin};
use bevy::camera::ManualTextureViewHandle;
use bevy::prelude::*;
use bevy::render::pipelined_rendering::PipelinedRenderingPlugin;
use bevy::render::renderer::{initialize_renderer, RenderDevice};
use bevy::render::settings::{Backends, RenderCreation, RenderResources, WgpuSettings};
use bevy::render::texture::ManualTextureViews;
use bevy::render::RenderPlugin;
use bevy::window::{ExitCondition, WindowPlugin};
use slint::wgpu_29::{wgpu, WGPUConfiguration};
use slint::{ComponentHandle, Global, RenderingState};

use target::SharedTexture;
use visualizer::SpectrumProcessor;

use crate::{AppWindow, Theme};

/// Width of the left navigation sidebar in logical pixels (matches
/// `ui/components/sidebar.slint`). The background and visualizer targets cover
/// the content area to its right.
const SIDEBAR_W: f32 = 96.0;

const BG_VIEW: ManualTextureViewHandle = ManualTextureViewHandle(1);
const AUTO_ICON_VIEW: ManualTextureViewHandle = ManualTextureViewHandle(2);
const SETTINGS_ICON_VIEW: ManualTextureViewHandle = ManualTextureViewHandle(3);
const VIZ_VIEW: ManualTextureViewHandle = ManualTextureViewHandle(4);

/// Slint-side state pushed into the Bevy world once per frame. This is what
/// replaces the `Arc<AtomicI32>` hand-off the GL renderers used.
#[derive(Resource, Default)]
struct UiState {
    active_view: i32,
    bg_enabled: bool,
    frost_enabled: bool,
    model: i32,
    brightness: f32,
    accent: LinearRgba,
    icon_color: LinearRgba,
    viz_renderer: i32,
    viz_theme: i32,
    viz_size: UVec2,
    /// Physical pixels per logical pixel, so layout constants expressed in
    /// logical pixels convert correctly on HiDPI displays.
    scale: f32,
    /// Seconds since the previous Slint frame.
    dt: f32,
    /// Seconds since startup, driving the background spin.
    elapsed: f32,
}

/// The wgpu objects Slint and Bevy share.
pub(crate) struct SharedRenderer {
    resources: RenderResources,
}

impl SharedRenderer {
    /// Initialize wgpu the way Bevy needs it. Must run before the Slint backend
    /// is selected, and therefore before `AppWindow::new()`.
    pub(crate) fn new() -> Self {
        let settings = WgpuSettings::default();
        let backends = settings.backends.unwrap_or(Backends::PRIMARY);
        let resources = bevy::tasks::block_on(initialize_renderer(backends, None, &settings));
        log::info!("bevy_gfx: wgpu adapter {:?}", *resources.2);
        Self { resources }
    }

    /// The same device/queue, handed to Slint so both renderers submit to one
    /// queue in a well-defined order.
    pub(crate) fn slint_configuration(&self) -> WGPUConfiguration {
        WGPUConfiguration::Manual {
            instance: wgpu::Instance::clone(&self.resources.4),
            adapter: wgpu::Adapter::clone(&self.resources.3),
            device: self.resources.0.wgpu_device().clone(),
            queue: wgpu::Queue::clone(&self.resources.1),
        }
    }
}

/// The headless Bevy app plus the textures it renders into.
struct BevyGfx {
    app: App,
    device: RenderDevice,
    background: SharedTexture,
    viz: SharedTexture,
    auto_icon: SharedTexture,
    settings_icon: SharedTexture,
    started: Instant,
    prev_elapsed: f32,
    published: bool,
}

impl BevyGfx {
    fn new(
        shared: SharedRenderer,
        consumer: crate::spectrum::AudioConsumer,
        viz_cfg: crate::config::VizConfig,
    ) -> Self {
        let device = shared.resources.0.clone();
        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    // Headless: Slint owns the only real window.
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    close_when_requested: false,
                    ..default()
                })
                .set(RenderPlugin {
                    render_creation: RenderCreation::Manual(shared.resources),
                    ..default()
                })
                // Slint's event loop drives the process lifetime and our own
                // tracing subscriber is already installed.
                .disable::<PanicHandlerPlugin>()
                .disable::<TerminalCtrlCHandlerPlugin>()
                // Pipelined rendering would run Bevy's render app one frame
                // behind on another thread, breaking the submission ordering
                // that lets us share Slint's queue without fences.
                .disable::<PipelinedRenderingPlugin>(),
        )
        .init_resource::<UiState>()
        .insert_non_send(SpectrumProcessor::new(consumer, &viz_cfg))
        .add_plugins((
            background::BackgroundPlugin,
            icons::IconsPlugin,
            visualizer::VisualizerPlugin { cfg: viz_cfg },
        ));

        let icon_size = UVec2::splat(icons::ICON_SIZE);
        let mut gfx = Self {
            background: SharedTexture::new(&device, "bevy-gfx-background", UVec2::ONE),
            viz: SharedTexture::new(&device, "bevy-gfx-visualizer", UVec2::ONE),
            auto_icon: SharedTexture::new(&device, "bevy-gfx-auto-icon", icon_size),
            settings_icon: SharedTexture::new(&device, "bevy-gfx-settings-icon", icon_size),
            device,
            app,
            started: Instant::now(),
            prev_elapsed: 0.0,
            published: false,
        };

        gfx.app.finish();
        gfx.app.cleanup();
        gfx
    }

    /// Push Slint's state into the Bevy world, run one Bevy frame, then publish
    /// the resulting textures back as Slint images.
    fn frame(&mut self, window: &AppWindow) {
        let content = self.content_size(window);
        self.resize(window, content);

        let elapsed = self.started.elapsed().as_secs_f32();
        let theme = Theme::get(window);
        let state = UiState {
            active_view: window.get_active_view(),
            bg_enabled: window.get_gfx_bg_enabled(),
            frost_enabled: window.get_gfx_frost_enabled(),
            model: window.get_gfx_model(),
            brightness: window.get_gfx_bg_brightness(),
            accent: linear(theme.get_red()),
            icon_color: linear(theme.get_text()),
            viz_renderer: window.get_viz_renderer(),
            viz_theme: window.get_viz_theme(),
            viz_size: content,
            scale: window.window().scale_factor(),
            dt: (elapsed - self.prev_elapsed).max(0.0),
            elapsed,
        };
        self.prev_elapsed = elapsed;
        self.app.insert_resource(state);

        self.app.update();
    }

    /// Size of the content area (the window minus the sidebar), in physical
    /// pixels — the background and visualizer targets both cover exactly it.
    fn content_size(&self, window: &AppWindow) -> UVec2 {
        let size = window.window().size();
        let sidebar = (SIDEBAR_W * window.window().scale_factor()).round() as u32;
        UVec2::new(size.width.saturating_sub(sidebar).max(1), size.height.max(1))
    }

    /// Reallocate the window-sized targets when the window changes size, and
    /// (re)publish every texture to Slint and to Bevy's manual view registry.
    fn resize(&mut self, window: &AppWindow, content: UVec2) {
        if self.published && self.background.size() == content {
            return;
        }
        self.published = true;

        if self.background.size() != content {
            self.background = SharedTexture::new(&self.device, "bevy-gfx-background", content);
            self.viz = SharedTexture::new(&self.device, "bevy-gfx-visualizer", content);
        }

        let mut views = self.app.world_mut().resource_mut::<ManualTextureViews>();
        views.insert(BG_VIEW, self.background.manual_view());
        views.insert(VIZ_VIEW, self.viz.manual_view());
        views.insert(AUTO_ICON_VIEW, self.auto_icon.manual_view());
        views.insert(SETTINGS_ICON_VIEW, self.settings_icon.manual_view());

        window.set_bg_texture(self.background.image());
        window.set_viz_texture(self.viz.image());
        window.set_auto_icon(self.auto_icon.image());
        window.set_settings_icon(self.settings_icon.image());
    }
}

/// Slint colours are sRGB; Bevy materials and vertex colours are linear.
fn linear(color: slint::Color) -> LinearRgba {
    Color::srgb_u8(color.red(), color.green(), color.blue()).to_linear()
}

/// Install the Bevy renderer on `window`.
///
/// Takes ownership of the audio consumer (moved into the spectrum processor)
/// and of the shared wgpu objects created before the Slint backend was selected.
pub(crate) fn install(
    window: &AppWindow,
    shared: SharedRenderer,
    consumer: crate::spectrum::AudioConsumer,
    viz_cfg: crate::config::VizConfig,
) {
    let weak = window.as_weak();
    let mut gfx = Some(BevyGfx::new(shared, consumer, viz_cfg));

    let result = window.window().set_rendering_notifier(move |state, _graphics_api| match state {
        RenderingState::BeforeRendering => {
            let (Some(gfx), Some(window)) = (gfx.as_mut(), weak.upgrade()) else {
                return;
            };
            gfx.frame(&window);
            // The background, icons and visualizer all animate continuously.
            window.window().request_redraw();
        }
        RenderingState::RenderingTeardown => {
            drop(gfx.take());
        }
        _ => {}
    });

    if let Err(e) = result {
        log::error!("bevy_gfx: failed to install rendering notifier: {e}");
    }
}
