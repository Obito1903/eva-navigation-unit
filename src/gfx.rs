//! GPU-rendered background graphics.
//!
//! Draws a rotating wireframe sphere as a full-window *underlay* behind all
//! Slint content, using the same OpenGL context femtovg renders into. The
//! sphere is plain line geometry (latitude/longitude grid) rotated on the GPU
//! — the classic NERV/"Magi" wireframe look.
//!
//! Integration follows Slint's `opengl_underlay` pattern:
//!   • `set_rendering_notifier` installs one closure on the window.
//!   • `RenderingSetup`    → build the `glow` context + GL resources.
//!   • `BeforeRendering`   → clear to black and (if enabled) draw the sphere,
//!                           then request another redraw to keep animating.
//!   • `RenderingTeardown` → drop the GL resources.
//!
//! For the sphere to be visible, the Slint backgrounds stacked above it must be
//! transparent (see `gfx-bg-enabled` gating in `app.slint`). Where Slint draws
//! nothing/transparent, this underlay shows through.

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use glow::HasContext;
use slint::{
    BorrowedOpenGLTextureBuilder, BorrowedOpenGLTextureOrigin, ComponentHandle, Global,
    GraphicsAPI, Image, RenderingState,
};

use crate::visualizer::VisualizerSystem;
use crate::{AppWindow, Theme};

/// Number of latitude parallels (excluding the poles).
const PARALLELS: usize = 7;
/// Number of longitude meridians.
const MERIDIANS: usize = 14;
/// Segments per parallel circle.
const PARALLEL_SEG: usize = 64;
/// Segments per meridian half-circle (pole to pole).
const MERIDIAN_SEG: usize = 32;

/// Width of the left navigation sidebar, in logical pixels (matches
/// `ui/components/sidebar.slint`). The sphere is offset right by this so it
/// centers within the content area rather than the whole window.
const SIDEBAR_W: f32 = 96.0;

/// Pixel size of the offscreen target used to render the spinning sidebar car
/// icon. Square so the wireframe stays isotropic (no stretching); Slint scales
/// it down into the nav button with `image-fit: contain`.
const ICON_SIZE: u32 = 160;

/// Model indices into `Underlay::models` for the spinning nav icons (must match
/// the build order in `Underlay::new`).
const ICON_MODEL_CAR: usize = 4;
const ICON_MODEL_GEAR: usize = 5;

/// GLSL ES 1.00 vertex shader: rotates the sphere on two axes over time and
/// applies a simple perspective projection (aspect-corrected).
const VERTEX_SHADER: &str = r"#version 100
attribute vec3 pos;
uniform float u_time;
uniform float u_aspect;
uniform vec2 u_offset;
uniform float u_tilt;   // 1.0 = animated X tilt; 0.0 = pure horizontal spin

void main() {
    float a = u_time * 0.45;                     // spin around Y
    float b = (u_time * 0.17 + 0.5) * u_tilt;    // slow tilt around X (gated)

    mat3 ry = mat3(
        cos(a), 0.0, -sin(a),
        0.0,    1.0,  0.0,
        sin(a), 0.0,  cos(a)
    );
    mat3 rx = mat3(
        1.0, 0.0,     0.0,
        0.0, cos(b), -sin(b),
        0.0, sin(b),  cos(b)
    );

    vec3 p = rx * ry * pos;

    // Perspective: push the sphere back along Z and divide.
    float dist = 3.2;
    float fov = 2.0;
    vec2 proj = (p.xy * fov) / (p.z + dist);

    // Keep the sphere circular regardless of window aspect ratio.
    if (u_aspect >= 1.0) {
        proj.x /= u_aspect;
    } else {
        proj.y *= u_aspect;
    }

    // Shift into the content area (right of the sidebar).
    proj += u_offset;

    gl_Position = vec4(proj, 0.0, 1.0);
}
";

/// GLSL ES 1.00 fragment shader: solid wireframe color from a uniform.
const FRAGMENT_SHADER: &str = r"#version 100
precision mediump float;
uniform vec3 u_color;

void main() {
    gl_FragColor = vec4(u_color, 1.0);
}
";

/// GLSL ES 1.00 vertex shader for the fullscreen frost quad: passes clip-space
/// positions straight through and derives `[0,1]` texture coordinates.
const FROST_VERTEX_SHADER: &str = r"#version 100
attribute vec2 pos;
varying vec2 v_uv;

void main() {
    v_uv = pos * 0.5 + 0.5;
    gl_Position = vec4(pos, 0.0, 1.0);
}
";

/// GLSL ES 1.00 fragment shader: the frosted-glass pass. Samples the rendered
/// sphere texture with a wide Gaussian-weighted blur (diffusing the sharp
/// wireframe), then lifts the result toward a cool frost tint so it reads like
/// the sphere is sitting behind a pane of frosted glass.
const FROST_FRAGMENT_SHADER: &str = r"#version 100
precision mediump float;
uniform sampler2D u_tex;
uniform vec2 u_texel;    // 1.0 / framebuffer resolution
uniform float u_radius;  // blur spread in texels
varying vec2 v_uv;

void main() {
    vec4 sum = vec4(0.0);
    float total = 0.0;
    // 5x5 separable-ish Gaussian sampled in a single pass.
    for (int x = -2; x <= 2; x++) {
        for (int y = -2; y <= 2; y++) {
            vec2 off = vec2(float(x), float(y)) * u_texel * u_radius;
            float w = 1.0 / (1.0 + float(x * x + y * y));
            sum += texture2D(u_tex, v_uv + off) * w;
            total += w;
        }
    }
    vec3 blurred = (sum / total).rgb;

    // Frost tint: whiten proportionally to local brightness and add a faint
    // cool base haze so dark areas still look like glass rather than void.
    float luma = max(max(blurred.r, blurred.g), blurred.b);
    vec3 frost_tint = vec3(0.72, 0.80, 0.88);
    vec3 frosted = mix(blurred, frost_tint, luma * 0.35) + frost_tint * 0.04;

    gl_FragColor = vec4(frosted, 1.0);
}
";

/// Owns the GL programs, buffers and offscreen framebuffer used to render the
/// wireframe sphere and the frosted-glass post pass. Created on
/// `RenderingSetup`, dropped on `RenderingTeardown`.
struct Underlay {
    gl: glow::Context,
    // Sphere pass.
    program: glow::Program,
    vbo: glow::Buffer,
    // Per-model `(start_vertex, vertex_count)` ranges into `vbo`, indexed by the
    // `gfx-model` selector: 0 = sphere, 1 = cube, 2 = car.
    models: Vec<(i32, i32)>,
    pos_location: u32,
    u_time: glow::UniformLocation,
    u_aspect: glow::UniformLocation,
    u_offset: glow::UniformLocation,
    u_color: glow::UniformLocation,
    u_tilt: glow::UniformLocation,
    // Frost (fullscreen) pass.
    frost_program: glow::Program,
    quad_vbo: glow::Buffer,
    frost_pos_location: u32,
    u_tex: glow::UniformLocation,
    u_texel: glow::UniformLocation,
    u_radius: glow::UniformLocation,
    // Offscreen target the sphere renders into before being frosted.
    fbo: glow::Framebuffer,
    fbo_tex: glow::Texture,
    fbo_w: u32,
    fbo_h: u32,
    // Offscreen target for the spinning sidebar icons. Each icon owns its own
    // color texture (so both can be displayed simultaneously); the textures are
    // handed to Slint as borrowed GL textures (zero-copy, no glReadPixels).
    icon_fbo: glow::Framebuffer,
    car_tex: glow::Texture,
    gear_tex: glow::Texture,
    icons_ready: bool,
}

/// Compile + link a vertex/fragment shader pair into a program, panicking with
/// the GL info log on any compile/link failure.
unsafe fn build_program(gl: &glow::Context, vs: &str, fs: &str) -> glow::Program {
    let program = gl.create_program().expect("create_program");
    let shader_sources = [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)];
    let mut shaders = Vec::with_capacity(shader_sources.len());
    for (kind, source) in shader_sources {
        let shader = gl.create_shader(kind).expect("create_shader");
        gl.shader_source(shader, source);
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            panic!("gfx shader compile error: {}", gl.get_shader_info_log(shader));
        }
        gl.attach_shader(program, shader);
        shaders.push(shader);
    }
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        panic!("gfx program link error: {}", gl.get_program_info_log(program));
    }
    for shader in shaders {
        gl.detach_shader(program, shader);
        gl.delete_shader(shader);
    }
    program
}

impl Underlay {
    fn new(gl: glow::Context) -> Self {
        unsafe {
            // ── Sphere program + geometry ─────────────────────────────────
            let program = build_program(&gl, VERTEX_SHADER, FRAGMENT_SHADER);

            // Pack every wireframe model into one buffer; record each model's
            // vertex range so `render` can draw whichever is selected.
            let mut vertices: Vec<f32> = Vec::new();
            let mut models: Vec<(i32, i32)> = Vec::new();
            for model in [
                build_sphere_wireframe(),
                build_cube_wireframe(),
                build_car_wireframe(),
                build_speaker_wireframe(),
                build_car_icon_wireframe(),
                build_gear_icon_wireframe(),
            ] {
                let start = (vertices.len() / 3) as i32;
                let count = (model.len() / 3) as i32;
                vertices.extend_from_slice(&model);
                models.push((start, count));
            }

            let vbo = gl.create_buffer().expect("create_buffer");
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck_cast(&vertices),
                glow::STATIC_DRAW,
            );
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            let pos_location = gl.get_attrib_location(program, "pos").expect("attrib pos");
            let u_time = gl.get_uniform_location(program, "u_time").expect("uniform u_time");
            let u_aspect =
                gl.get_uniform_location(program, "u_aspect").expect("uniform u_aspect");
            let u_offset =
                gl.get_uniform_location(program, "u_offset").expect("uniform u_offset");
            let u_color = gl.get_uniform_location(program, "u_color").expect("uniform u_color");
            let u_tilt = gl.get_uniform_location(program, "u_tilt").expect("uniform u_tilt");

            // ── Frost program + fullscreen quad ───────────────────────────
            let frost_program =
                build_program(&gl, FROST_VERTEX_SHADER, FROST_FRAGMENT_SHADER);

            // Fullscreen triangle-strip quad in clip space.
            let quad: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
            let quad_vbo = gl.create_buffer().expect("create_buffer");
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(quad_vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck_cast(&quad), glow::STATIC_DRAW);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            let frost_pos_location =
                gl.get_attrib_location(frost_program, "pos").expect("frost attrib pos");
            let u_tex = gl.get_uniform_location(frost_program, "u_tex").expect("uniform u_tex");
            let u_texel =
                gl.get_uniform_location(frost_program, "u_texel").expect("uniform u_texel");
            let u_radius =
                gl.get_uniform_location(frost_program, "u_radius").expect("uniform u_radius");

            // ── Offscreen FBO (sized lazily in `render`) ──────────────────
            let fbo = gl.create_framebuffer().expect("create_framebuffer");
            let fbo_tex = gl.create_texture().expect("create_texture");

            // ── Icon FBO (fixed-size, sized lazily in `render_icon`) ──────
            let icon_fbo = gl.create_framebuffer().expect("create_framebuffer");
            let car_tex = gl.create_texture().expect("create_texture");
            let gear_tex = gl.create_texture().expect("create_texture");

            Self {
                gl,
                program,
                vbo,
                models,
                pos_location,
                u_time,
                u_aspect,
                u_offset,
                u_color,
                u_tilt,
                frost_program,
                quad_vbo,
                frost_pos_location,
                u_tex,
                u_texel,
                u_radius,
                fbo,
                fbo_tex,
                fbo_w: 0,
                fbo_h: 0,
                icon_fbo,
                car_tex,
                gear_tex,
                icons_ready: false,
            }
        }
    }

    /// (Re)allocate the offscreen color texture to match the framebuffer size.
    unsafe fn ensure_fbo(&mut self, width: u32, height: u32) {
        if self.fbo_w == width && self.fbo_h == height {
            return;
        }
        let gl = &self.gl;
        gl.bind_texture(glow::TEXTURE_2D, Some(self.fbo_tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA as i32,
            width as i32,
            height as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.bind_texture(glow::TEXTURE_2D, None);
        self.fbo_w = width;
        self.fbo_h = height;
    }

    /// Clear the framebuffer to black and, when `enabled`, render the rotating
    /// wireframe model into an offscreen target and composite it back through
    /// the frosted-glass pass. `offset_x` shifts the model horizontally in NDC
    /// so it can center on the content area instead of the whole window.
    /// `model` selects which wireframe to draw (0 = sphere, 1 = cube, 2 = car).
    fn render(
        &mut self,
        width: u32,
        height: u32,
        time: f32,
        color: (f32, f32, f32),
        offset_x: f32,
        model: i32,
        enabled: bool,
    ) {
        if !enabled {
            let gl = &self.gl;
            unsafe {
                gl.viewport(0, 0, width as i32, height as i32);
                gl.disable(glow::DEPTH_TEST);
                gl.clear_color(0.0, 0.0, 0.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            }
            return;
        }

        unsafe { self.ensure_fbo(width, height) };

        let gl = &self.gl;
        unsafe {
            // Save the framebuffer femtovg is rendering into so we can restore
            // it after the offscreen sphere pass.
            let prev_fbo = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
            let prev_fbo = NonZeroU32::new(prev_fbo as u32).map(glow::NativeFramebuffer);

            // ── Pass 1: sphere → offscreen FBO ────────────────────────────
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(self.fbo_tex),
                0,
            );
            gl.viewport(0, 0, width as i32, height as i32);
            gl.disable(glow::DEPTH_TEST);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);

            gl.use_program(Some(self.program));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.enable_vertex_attrib_array(self.pos_location);
            gl.vertex_attrib_pointer_f32(self.pos_location, 3, glow::FLOAT, false, 0, 0);
            gl.uniform_1_f32(Some(&self.u_time), time);
            let aspect = if height == 0 { 1.0 } else { width as f32 / height as f32 };
            gl.uniform_1_f32(Some(&self.u_aspect), aspect);
            gl.uniform_2_f32(Some(&self.u_offset), offset_x, 0.0);
            gl.uniform_3_f32(Some(&self.u_color), color.0, color.1, color.2);
            gl.uniform_1_f32(Some(&self.u_tilt), 1.0);
            let (start, count) = self
                .models
                .get(model.max(0) as usize)
                .copied()
                .unwrap_or(self.models[0]);
            gl.draw_arrays(glow::LINES, start, count);
            gl.disable_vertex_attrib_array(self.pos_location);

            // ── Pass 2: frosted-glass composite → femtovg's framebuffer ───
            gl.bind_framebuffer(glow::FRAMEBUFFER, prev_fbo);
            gl.viewport(0, 0, width as i32, height as i32);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);

            gl.use_program(Some(self.frost_program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.fbo_tex));
            gl.uniform_1_i32(Some(&self.u_tex), 0);
            let (tw, th) = (width.max(1) as f32, height.max(1) as f32);
            gl.uniform_2_f32(Some(&self.u_texel), 1.0 / tw, 1.0 / th);
            gl.uniform_1_f32(Some(&self.u_radius), 4.0);

            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.quad_vbo));
            gl.enable_vertex_attrib_array(self.frost_pos_location);
            gl.vertex_attrib_pointer_f32(self.frost_pos_location, 2, glow::FLOAT, false, 0, 0);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            // Restore the bits femtovg does not unconditionally reset itself.
            gl.disable_vertex_attrib_array(self.frost_pos_location);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
        }
    }

    /// Allocate both icon color textures once, on first use.
    unsafe fn ensure_icon_fbo(&mut self) {
        if self.icons_ready {
            return;
        }
        let gl = &self.gl;
        for tex in [self.car_tex, self.gear_tex] {
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                ICON_SIZE as i32,
                ICON_SIZE as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        }
        gl.bind_texture(glow::TEXTURE_2D, None);
        self.icons_ready = true;
    }

    /// Render a wireframe icon model (spinning about its vertical axis with
    /// `time`) into `tex` over a transparent background, and return it wrapped
    /// as a *borrowed* Slint OpenGL texture (zero-copy — no `glReadPixels`, so
    /// it does not stall the GPU pipeline). Reuses the main wireframe
    /// program/geometry; `model` selects which model range to draw.
    fn render_icon(&mut self, time: f32, color: (f32, f32, f32), model: usize, tex: glow::Texture) -> Image {
        unsafe { self.ensure_icon_fbo() };

        let gl = &self.gl;
        unsafe {
            let prev_fbo = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
            let prev_fbo = NonZeroU32::new(prev_fbo as u32).map(glow::NativeFramebuffer);

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.icon_fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(tex),
                0,
            );
            gl.viewport(0, 0, ICON_SIZE as i32, ICON_SIZE as i32);
            gl.disable(glow::DEPTH_TEST);
            // Transparent background so the icon blends into the nav button.
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);

            gl.use_program(Some(self.program));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.enable_vertex_attrib_array(self.pos_location);
            gl.vertex_attrib_pointer_f32(self.pos_location, 3, glow::FLOAT, false, 0, 0);
            gl.uniform_1_f32(Some(&self.u_time), time);
            // Square target → aspect 1.0 (isotropic); no horizontal offset.
            gl.uniform_1_f32(Some(&self.u_aspect), 1.0);
            gl.uniform_2_f32(Some(&self.u_offset), 0.0, 0.0);
            gl.uniform_3_f32(Some(&self.u_color), color.0, color.1, color.2);
            // Pure horizontal spin (no X tilt) for the icon.
            gl.uniform_1_f32(Some(&self.u_tilt), 0.0);
            // Thicker stroke so the small icon reads clearly. (Driver may
            // clamp wide aliased lines; Mesa typically allows several px.)
            gl.line_width(4.0);
            let (start, count) = self.models.get(model).copied().unwrap_or(self.models[0]);
            gl.draw_arrays(glow::LINES, start, count);
            gl.disable_vertex_attrib_array(self.pos_location);
            gl.line_width(1.0);

            gl.bind_framebuffer(glow::FRAMEBUFFER, prev_fbo);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.use_program(None);
        }

        // Hand Slint the live GL texture directly. GL's origin is bottom-left,
        // so `BottomLeft` flips it to Slint's top-left screen origin.
        unsafe {
            BorrowedOpenGLTextureBuilder::new_gl_2d_rgba_texture(
                tex.0,
                [ICON_SIZE, ICON_SIZE].into(),
            )
        }
        .origin(BorrowedOpenGLTextureOrigin::BottomLeft)
        .build()
    }
}

impl Drop for Underlay {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_program(self.program);
            self.gl.delete_program(self.frost_program);
            self.gl.delete_buffer(self.vbo);
            self.gl.delete_buffer(self.quad_vbo);
            self.gl.delete_framebuffer(self.fbo);
            self.gl.delete_texture(self.fbo_tex);
            self.gl.delete_framebuffer(self.icon_fbo);
            self.gl.delete_texture(self.car_tex);
            self.gl.delete_texture(self.gear_tex);
        }
    }
}

/// Build a unit-sphere wireframe as a flat list of `GL_LINES` vertices
/// (`[x, y, z, x, y, z, ...]`): `PARALLELS` latitude circles plus `MERIDIANS`
/// longitude half-circles. Each drawn segment contributes two vertices.
fn build_sphere_wireframe() -> Vec<f32> {
    use std::f32::consts::PI;
    let mut v: Vec<f32> = Vec::new();

    let point = |phi: f32, theta: f32| -> [f32; 3] {
        [
            phi.sin() * theta.cos(),
            phi.cos(),
            phi.sin() * theta.sin(),
        ]
    };

    // Latitude parallels: fixed phi, sweep theta around.
    for i in 1..=PARALLELS {
        let phi = PI * (i as f32) / (PARALLELS as f32 + 1.0);
        for s in 0..PARALLEL_SEG {
            let t0 = 2.0 * PI * (s as f32) / (PARALLEL_SEG as f32);
            let t1 = 2.0 * PI * ((s + 1) as f32) / (PARALLEL_SEG as f32);
            v.extend_from_slice(&point(phi, t0));
            v.extend_from_slice(&point(phi, t1));
        }
    }

    // Longitude meridians: fixed theta, sweep phi from pole to pole.
    for m in 0..MERIDIANS {
        let theta = 2.0 * PI * (m as f32) / (MERIDIANS as f32);
        for s in 0..MERIDIAN_SEG {
            let p0 = PI * (s as f32) / (MERIDIAN_SEG as f32);
            let p1 = PI * ((s + 1) as f32) / (MERIDIAN_SEG as f32);
            v.extend_from_slice(&point(p0, theta));
            v.extend_from_slice(&point(p1, theta));
        }
    }

    v
}

/// Append a line segment (two endpoints) to a `GL_LINES` vertex list.
fn edge(v: &mut Vec<f32>, a: [f32; 3], b: [f32; 3]) {
    v.extend_from_slice(&a);
    v.extend_from_slice(&b);
}

/// Append the 12 edges of an axis-aligned box spanning `min`..`max`.
fn box_edges(v: &mut Vec<f32>, min: [f32; 3], max: [f32; 3]) {
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    // 8 corners.
    let c = [
        [x0, y0, z0],
        [x1, y0, z0],
        [x1, y1, z0],
        [x0, y1, z0],
        [x0, y0, z1],
        [x1, y0, z1],
        [x1, y1, z1],
        [x0, y1, z1],
    ];
    // Bottom face, top face, then the 4 vertical pillars.
    let pairs = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    for (a, b) in pairs {
        edge(v, c[a], c[b]);
    }
}

/// Build a unit-cube wireframe (the 12 edges of a cube centered on the origin)
/// as a `GL_LINES` vertex list.
fn build_cube_wireframe() -> Vec<f32> {
    let mut v: Vec<f32> = Vec::new();
    box_edges(&mut v, [-0.8, -0.8, -0.8], [0.8, 0.8, 0.8]);
    v
}

/// Build a stylized wireframe sports car in the spirit of a Renault Alpine
/// A310 — a wedge fastback with a low pointed nose, raked windshield, short
/// tapered greenhouse and a long sloping tail. Returned as a `GL_LINES` vertex
/// list, centered on the origin and scaled to roughly fit the same view volume
/// as the sphere/cube.
fn build_car_wireframe() -> Vec<f32> {
    use std::f32::consts::PI;
    let mut v: Vec<f32> = Vec::new();

    // ── Body shell ────────────────────────────────────────────────────────
    // Side silhouette as a closed loop of `[x, y, half_width]` points.
    //   x: +front .. -rear,  y: up,  half_width: body half-thickness in z.
    // More points than a box give the wedge profile its curves: a dropped nose,
    // rising fender, raked screen, short roof and long fastback tail.
    let profile: [[f32; 3]; 18] = [
        [1.18, -0.10, 0.22],  // nose tip top (narrow, low)
        [1.10, 0.02, 0.34],   // hood leading edge
        [0.82, 0.06, 0.46],   // front fender crest
        [0.50, 0.07, 0.50],   // cowl / base of windshield
        [0.30, 0.22, 0.44],   // mid windshield
        [0.12, 0.40, 0.34],   // windshield top (greenhouse)
        [-0.10, 0.42, 0.34],  // roof mid
        [-0.32, 0.41, 0.34],  // roof rear (greenhouse)
        [-0.58, 0.30, 0.44],  // backlight / rear glass
        [-0.82, 0.16, 0.48],  // rear haunch
        [-1.05, 0.04, 0.44],  // tail top
        [-1.15, -0.06, 0.42], // tail edge
        [-1.13, -0.24, 0.42], // tail bottom
        [-0.80, -0.30, 0.50], // rear sill
        [0.00, -0.32, 0.52],  // floor pan mid
        [0.80, -0.30, 0.50],  // front sill
        [1.10, -0.24, 0.30],  // nose bottom
        [1.18, -0.18, 0.24],  // nose lip
    ];

    let n = profile.len();
    // Left + right side outlines, plus a rib joining the two sides at each
    // vertex (these ribs also form the windshield, roof and tail cross-sections).
    for i in 0..n {
        let a = profile[i];
        let b = profile[(i + 1) % n];
        edge(&mut v, [a[0], a[1], -a[2]], [b[0], b[1], -b[2]]);
        edge(&mut v, [a[0], a[1], a[2]], [b[0], b[1], b[2]]);
        edge(&mut v, [a[0], a[1], -a[2]], [a[0], a[1], a[2]]);
    }

    // ── Greenhouse / side windows ───────────────────────────────────────────
    // A tapered glasshouse outline drawn just inboard of each flank so the
    // cabin reads as glazed. Points: windshield base → top → roof rear →
    // backlight base → belt line, back to start.
    let glass: [[f32; 2]; 5] = [
        [0.46, 0.10],   // A-pillar base
        [0.14, 0.39],   // A-pillar top
        [-0.34, 0.40],  // C-pillar top
        [-0.56, 0.28],  // C-pillar base
        [-0.30, 0.18],  // belt line return
    ];
    let glass_hw = 0.40;
    for hw in [-glass_hw, glass_hw] {
        for i in 0..glass.len() {
            let a = glass[i];
            let b = glass[(i + 1) % glass.len()];
            edge(&mut v, [a[0], a[1], hw], [b[0], b[1], hw]);
        }
    }
    // Door-glass divider (B-pillar) for a two-window look.
    for hw in [-glass_hw, glass_hw] {
        edge(&mut v, [-0.06, 0.41, hw], [-0.06, 0.17, hw]);
    }

    // ── Longitudinal creases ────────────────────────────────────────────────
    // Belt line and a lower body crease give the flanks definition.
    let belt = [
        [1.08_f32, -0.02],
        [0.48, 0.06],
        [-0.30, 0.14],
        [-1.04, 0.02],
    ];
    let lower = [
        [1.10_f32, -0.18],
        [0.40, -0.14],
        [-0.40, -0.12],
        [-1.06, -0.16],
    ];
    for line in [&belt, &lower] {
        for hw in [-0.49_f32, 0.49] {
            for i in 0..line.len() - 1 {
                edge(
                    &mut v,
                    [line[i][0], line[i][1], hw],
                    [line[i + 1][0], line[i + 1][1], hw],
                );
            }
        }
    }

    // ── Lights ───────────────────────────────────────────────────────────────
    // Front: a pair of small ellipse-ish circles per side. Rear: short bars.
    for &(cx, cy, hw, r) in &[
        (1.02_f32, 0.04_f32, 0.30_f32, 0.07_f32),
        (1.02, 0.04, 0.42, 0.07),
    ] {
        for hw in [-hw, hw] {
            for s in 0..12 {
                let a0 = 2.0 * PI * (s as f32) / 12.0;
                let a1 = 2.0 * PI * ((s + 1) as f32) / 12.0;
                edge(
                    &mut v,
                    [cx + r * a0.cos() * 0.7, cy + r * a0.sin(), hw],
                    [cx + r * a1.cos() * 0.7, cy + r * a1.sin(), hw],
                );
            }
        }
    }
    // Rear light bar across the tail.
    for y in [-0.02_f32, 0.06] {
        edge(&mut v, [-1.13, y, -0.34], [-1.13, y, 0.34]);
    }

    // ── Wheel arches + wheels ─────────────────────────────────────────────────
    let wheel_r = 0.26;
    let arch_r = 0.32;
    let wheel_seg = 24;
    let arch_hw = 0.50; // flank position for the arch outline
    let wheels = [
        [0.62_f32, -0.28],  // front axle (x, y)
        [-0.62, -0.28],     // rear axle
    ];
    for w in wheels {
        let (cx, cy) = (w[0], w[1]);
        // Wheel-arch: an upper half-circle cut into each flank.
        for hw in [-arch_hw, arch_hw] {
            for s in 0..12 {
                let a0 = PI * (s as f32) / 12.0;
                let a1 = PI * ((s + 1) as f32) / 12.0;
                edge(
                    &mut v,
                    [cx + arch_r * a0.cos(), cy + arch_r * a0.sin(), hw],
                    [cx + arch_r * a1.cos(), cy + arch_r * a1.sin(), hw],
                );
            }
        }
        // The two wheels on this axle, with a hub + spokes.
        for hw in [-0.50_f32, 0.50] {
            wheel_disc(&mut v, cx, cy, hw, wheel_r, wheel_seg);
        }
    }

    v
}

/// Append a wireframe wheel at `(cx, cy, z)`: a tyre circle, a hub circle and
/// four spokes, drawn in the X/Y plane.
fn wheel_disc(v: &mut Vec<f32>, cx: f32, cy: f32, z: f32, r: f32, seg: usize) {
    use std::f32::consts::PI;
    let hub = r * 0.4;
    for s in 0..seg {
        let a0 = 2.0 * PI * (s as f32) / (seg as f32);
        let a1 = 2.0 * PI * ((s + 1) as f32) / (seg as f32);
        // Tyre.
        edge(
            v,
            [cx + r * a0.cos(), cy + r * a0.sin(), z],
            [cx + r * a1.cos(), cy + r * a1.sin(), z],
        );
        // Hub.
        edge(
            v,
            [cx + hub * a0.cos(), cy + hub * a0.sin(), z],
            [cx + hub * a1.cos(), cy + hub * a1.sin(), z],
        );
    }
    // Four spokes from hub to rim.
    for k in 0..4 {
        let a = 2.0 * PI * (k as f32) / 4.0 + PI / 4.0;
        edge(
            v,
            [cx + hub * a.cos(), cy + hub * a.sin(), z],
            [cx + r * a.cos(), cy + r * a.sin(), z],
        );
    }
}

/// Build a wireframe hi-fi speaker: a tall rectangular cabinet with a recessed
/// front baffle, a large woofer (with surround + dust cap) and a small tweeter
/// on the front face, plus a bass-reflex port. Returned as a `GL_LINES` vertex
/// list, centered on the origin.
fn build_speaker_wireframe() -> Vec<f32> {
    use std::f32::consts::PI;
    let mut v: Vec<f32> = Vec::new();

    // Cabinet: width (x) × height (y) × depth (z). Front face sits at +z.
    let (hx, hy, hz) = (0.55_f32, 0.9_f32, 0.5_f32);
    box_edges(&mut v, [-hx, -hy, -hz], [hx, hy, hz]);

    // Recessed front baffle outline, inset from the cabinet edges.
    let bx = hx - 0.10;
    let by = hy - 0.10;
    let bz = hz; // on the front face
    let baffle = [
        [-bx, -by],
        [bx, -by],
        [bx, by],
        [-bx, by],
    ];
    for i in 0..baffle.len() {
        let a = baffle[i];
        let b = baffle[(i + 1) % baffle.len()];
        edge(&mut v, [a[0], a[1], bz], [b[0], b[1], bz]);
    }

    // Concentric circle on the front face at `(cx, cy)`, radius `r`. A second
    // ring slightly forward in z fakes the cone depth.
    let mut driver = |cx: f32, cy: f32, r: f32, depth: f32, seg: usize| {
        for s in 0..seg {
            let a0 = 2.0 * PI * (s as f32) / (seg as f32);
            let a1 = 2.0 * PI * ((s + 1) as f32) / (seg as f32);
            // Outer surround on the baffle.
            edge(
                &mut v,
                [cx + r * a0.cos(), cy + r * a0.sin(), bz],
                [cx + r * a1.cos(), cy + r * a1.sin(), bz],
            );
            // Inner cone rim, pushed back into the cabinet.
            let ri = r * 0.55;
            edge(
                &mut v,
                [cx + ri * a0.cos(), cy + ri * a0.sin(), bz - depth],
                [cx + ri * a1.cos(), cy + ri * a1.sin(), bz - depth],
            );
            // Spokes from surround to cone rim suggest the cone surface.
            if s % 3 == 0 {
                edge(
                    &mut v,
                    [cx + r * a0.cos(), cy + r * a0.sin(), bz],
                    [cx + ri * a0.cos(), cy + ri * a0.sin(), bz - depth],
                );
            }
        }
        // Dust cap at the cone center.
        let rc = r * 0.18;
        for s in 0..seg {
            let a0 = 2.0 * PI * (s as f32) / (seg as f32);
            let a1 = 2.0 * PI * ((s + 1) as f32) / (seg as f32);
            edge(
                &mut v,
                [cx + rc * a0.cos(), cy + rc * a0.sin(), bz - depth],
                [cx + rc * a1.cos(), cy + rc * a1.sin(), bz - depth],
            );
        }
    };

    // Woofer (lower, large) and tweeter (upper, small).
    driver(0.0, -0.30, 0.34, 0.14, 28);
    driver(0.0, 0.42, 0.13, 0.06, 20);

    // Bass-reflex port: a small ring near the bottom of the baffle.
    let (px, py, pr) = (0.0_f32, -0.74_f32, 0.08_f32);
    for s in 0..16 {
        let a0 = 2.0 * PI * (s as f32) / 16.0;
        let a1 = 2.0 * PI * ((s + 1) as f32) / 16.0;
        edge(
            &mut v,
            [px + pr * a0.cos(), py + pr * a0.sin(), bz],
            [px + pr * a1.cos(), py + pr * a1.sin(), bz],
        );
    }

    v
}

/// Build a deliberately simple, centered wireframe car for the nav icon: a
/// lower body box, a smaller cabin box on top, and four wheel rings (one per
/// corner, on both flanks). Centered on the origin in all axes so it sits in
/// the middle of the square icon target, and kept low-poly so it stays legible
/// at icon size while spinning. Returned as a `GL_LINES` vertex list.
fn build_car_icon_wireframe() -> Vec<f32> {
    use std::f32::consts::PI;
    let mut v: Vec<f32> = Vec::new();

    let hw = 0.40; // body half-width (z)

    // Lower body box.
    box_edges(&mut v, [-0.85, -0.18, -hw], [0.85, 0.08, hw]);
    // Cabin / greenhouse: narrower box sitting on the body.
    box_edges(&mut v, [-0.45, 0.08, -hw * 0.82], [0.35, 0.34, hw * 0.82]);

    // Four wheels as rings in the x-y plane at both flanks.
    let wheel_r = 0.16;
    let wheel_y = -0.18;
    let seg = 16;
    for &cx in &[-0.5_f32, 0.5] {
        for &z in &[-hw, hw] {
            for s in 0..seg {
                let a0 = 2.0 * PI * (s as f32) / seg as f32;
                let a1 = 2.0 * PI * ((s + 1) as f32) / seg as f32;
                edge(
                    &mut v,
                    [cx + wheel_r * a0.cos(), wheel_y + wheel_r * a0.sin(), z],
                    [cx + wheel_r * a1.cos(), wheel_y + wheel_r * a1.sin(), z],
                );
            }
        }
    }

    // Scale the whole model up so it fills more of the square icon frame
    // (less empty margin → reads larger). Geometry stays centered on origin.
    for c in v.iter_mut() {
        *c *= 1.5;
    }

    v
}

/// Build a simple 3D wireframe gear/cog for the settings nav icon: two parallel
/// toothed rings (front and back faces) joined into a short extruded disc, with
/// a small central bore. Centered on the origin so it sits in the middle of the
/// square icon target. Returned as a `GL_LINES` vertex list.
fn build_gear_icon_wireframe() -> Vec<f32> {
    use std::f32::consts::PI;
    let mut v: Vec<f32> = Vec::new();

    let teeth = 8;
    let r_root = 0.52; // valley radius
    let r_tip = 0.78; // tooth-tip radius
    let r_bore = 0.20; // central hole radius
    let hz = 0.18; // half thickness (extrusion along z)

    // Toothed outline as a list of (x, y) points: each tooth contributes a
    // rise to the tip, a flat tip, and a fall back to the root.
    let mut outline: Vec<[f32; 2]> = Vec::new();
    let steps = teeth * 4; // 4 vertices per tooth
    for i in 0..steps {
        let frac = i as f32 / steps as f32;
        let ang = 2.0 * PI * frac;
        // Within each tooth (4 slots): 0,1 = tip, 2,3 = root.
        let slot = i % 4;
        let r = if slot == 0 || slot == 1 { r_tip } else { r_root };
        outline.push([r * ang.cos(), r * ang.sin()]);
    }

    // Front (+z) and back (-z) toothed rings.
    for &z in &[-hz, hz] {
        for i in 0..outline.len() {
            let a = outline[i];
            let b = outline[(i + 1) % outline.len()];
            edge(&mut v, [a[0], a[1], z], [b[0], b[1], z]);
        }
    }
    // Spokes joining front and back outline at each vertex (extrusion edges).
    for i in 0..outline.len() {
        let a = outline[i];
        edge(&mut v, [a[0], a[1], -hz], [a[0], a[1], hz]);
    }

    // Central bore: front + back rings plus joining edges.
    let bore_seg = 16;
    for &z in &[-hz, hz] {
        for s in 0..bore_seg {
            let a0 = 2.0 * PI * (s as f32) / bore_seg as f32;
            let a1 = 2.0 * PI * ((s + 1) as f32) / bore_seg as f32;
            edge(
                &mut v,
                [r_bore * a0.cos(), r_bore * a0.sin(), z],
                [r_bore * a1.cos(), r_bore * a1.sin(), z],
            );
        }
    }
    for s in 0..bore_seg {
        let a = 2.0 * PI * (s as f32) / bore_seg as f32;
        edge(
            &mut v,
            [r_bore * a.cos(), r_bore * a.sin(), -hz],
            [r_bore * a.cos(), r_bore * a.sin(), hz],
        );
    }

    v
}

/// Reinterpret an `f32` slice as bytes for `buffer_data_u8_slice`.
fn bytemuck_cast(data: &[f32]) -> &[u8] {
    // Safety: `f32` has no padding/invalid bit patterns and `u8` has alignment
    // 1, so viewing the same bytes as `&[u8]` is always valid.
    unsafe {
        std::slice::from_raw_parts(data.as_ptr() as *const u8, std::mem::size_of_val(data))
    }
}

/// Read the active theme's accent color as normalized RGB for the wireframe.
fn theme_color(window: &AppWindow) -> (f32, f32, f32) {
    let c = Theme::get(window).get_red();
    (
        c.red() as f32 / 255.0,
        c.green() as f32 / 255.0,
        c.blue() as f32 / 255.0,
    )
}

/// Read the active theme's primary text color as normalized RGB (used to tint
/// the nav car icon so it matches the surrounding label text).
fn theme_text_color(window: &AppWindow) -> (f32, f32, f32) {
    let c = Theme::get(window).get_text();
    (
        c.red() as f32 / 255.0,
        c.green() as f32 / 255.0,
        c.blue() as f32 / 255.0,
    )
}

/// Install the wireframe-sphere underlay and the visualizer system on `window`.
///
/// Sets a single rendering notifier that manages GL resources for both systems.
/// When `active-view == 3` the visualizer renders instead of the sphere.
pub(crate) fn install(
    window: &AppWindow,
    consumer: crate::spectrum::AudioConsumer,
    viz_renderer_id: Arc<AtomicI32>,
    viz_theme: Arc<AtomicI32>,
    viz_cfg: Arc<crate::config::VizConfig>,
) {
    let weak = window.as_weak();
    let start = Instant::now();
    let mut underlay: Option<Underlay> = None;
    let mut viz_gl: Option<glow::Context> = None;
    let mut viz_system: Option<VisualizerSystem> = None;
    // consumer is moved into VisualizerSystem on first VIZ view activation.
    let mut viz_consumer: Option<crate::spectrum::AudioConsumer> = Some(consumer);
    // Frame-time instrumentation for the VIZ view.
    let mut viz_last_log = Instant::now();
    let mut viz_frame_count: u32 = 0;
    let mut viz_acc_ms: f32 = 0.0;
    // Dedicated spin clocks for the nav icons. Each only advances while its
    // view is active, so an icon pauses when another view is selected and
    // resumes from the same angle (rather than jumping with wall-clock time).
    let mut car_time: f32 = 0.0;
    let mut gear_time: f32 = 0.0;
    let mut prev_time: f32 = 0.0;
    // Last active view, so the inactive icon is re-rendered only on a view
    // change (and at startup) instead of every frame.
    let mut last_view: i32 = -1;

    let result = window.window().set_rendering_notifier(move |state, graphics_api| {
        match state {
            RenderingState::RenderingSetup => {
                match graphics_api {
                    GraphicsAPI::NativeOpenGL { get_proc_address } => unsafe {
                        // Two Context instances from the same native GL context: they share
                        // all GL state (same function pointers, same driver objects).
                        let ctx1 = glow::Context::from_loader_function_cstr(|s| get_proc_address(s));
                        let ctx2 = glow::Context::from_loader_function_cstr(|s| get_proc_address(s));
                        underlay = Some(Underlay::new(ctx1));
                        viz_gl = Some(ctx2);
                    },
                    _ => {
                        log::error!("gfx: unexpected graphics API; underlay disabled");
                        return;
                    }
                };
            }
            RenderingState::BeforeRendering => {
                if let (Some(underlay), Some(win)) = (underlay.as_mut(), weak.upgrade()) {
                    let active_view = win.get_active_view();
                    let size = win.window().size();

                    // ── Visualizer view (index 3): hand off to VisualizerSystem ──
                    if active_view == 3 {
                        if let Some(gl) = viz_gl.as_ref() {
                            // Lazy-initialise the VisualizerSystem on first use,
                            // consuming the AudioConsumer from the capture thread.
                            if viz_system.is_none() {
                                if let Some(consumer) = viz_consumer.take() {
                                    viz_system = Some(VisualizerSystem::new(
                                        gl,
                                        size.width,
                                        size.height,
                                        consumer,
                                        viz_renderer_id.clone(),
                                        viz_theme.clone(),
                                        &viz_cfg,
                                    ));
                                }
                            }
                            if let Some(viz) = viz_system.as_mut() {
                                let t0 = Instant::now();
                                viz.render_frame(gl, size.width, size.height);
                                let frame_ms = t0.elapsed().as_secs_f32() * 1000.0;
                                viz_acc_ms += frame_ms;
                                viz_frame_count += 1;
                                let since_log = viz_last_log.elapsed().as_secs_f32();
                                if since_log >= 2.0 {
                                    let fps = viz_frame_count as f32 / since_log;
                                    let avg_ms = viz_acc_ms / viz_frame_count as f32;
                                    log::info!(
                                        "viz: {fps:.1} fps  render {avg_ms:.2} ms/frame  \
                                         res {}x{}",
                                        size.width, size.height
                                    );
                                    viz_last_log = Instant::now();
                                    viz_frame_count = 0;
                                    viz_acc_ms = 0.0;
                                }
                            }
                        }
                        win.window().request_redraw();
                        return;
                    }

                    let enabled = win.get_gfx_bg_enabled();
                    let time = start.elapsed().as_secs_f32();
                    let color = theme_color(&win);
                    let model = win.get_gfx_model();
                    // Center the sphere on the content area (right of the
                    // sidebar): the content center sits `sidebar_px / width`
                    // to the right of the window center in NDC.
                    let scale = win.window().scale_factor();
                    let offset_x = if size.width == 0 {
                        0.0
                    } else {
                        SIDEBAR_W * scale / size.width as f32
                    };
                    underlay.render(size.width, size.height, time, color, offset_x, model, enabled);

                    // Spinning nav icons. Each icon owns a GL texture handed to
                    // Slint as a zero-copy borrowed texture (no glReadPixels, so
                    // no GPU pipeline stall). The active view's icon is
                    // re-rendered every frame (advancing its spin clock); the
                    // other icon's texture keeps its last content, so it is only
                    // re-rendered on a view change (or startup) — a parked,
                    // frozen frame. Resuming a view continues from the same
                    // angle (no wall-clock jump).
                    let dt = (time - prev_time).max(0.0);
                    prev_time = time;
                    let icon_color = theme_text_color(&win);
                    let view_changed = active_view != last_view;
                    last_view = active_view;
                    let car_tex = underlay.car_tex;
                    let gear_tex = underlay.gear_tex;

                    if active_view == 0 {
                        car_time += dt;
                        let car = underlay.render_icon(car_time, icon_color, ICON_MODEL_CAR, car_tex);
                        win.set_auto_icon(car);
                        if view_changed {
                            let gear = underlay
                                .render_icon(gear_time, icon_color, ICON_MODEL_GEAR, gear_tex);
                            win.set_settings_icon(gear);
                        }
                    } else if active_view == 1 {
                        gear_time += dt;
                        let gear =
                            underlay.render_icon(gear_time, icon_color, ICON_MODEL_GEAR, gear_tex);
                        win.set_settings_icon(gear);
                        if view_changed {
                            let car =
                                underlay.render_icon(car_time, icon_color, ICON_MODEL_CAR, car_tex);
                            win.set_auto_icon(car);
                        }
                    }

                    // Keep animating while the GL background, nav icons, or
                    // the visualizer view is active.
                    if enabled || active_view == 0 || active_view == 1 || active_view == 3 {
                        win.window().request_redraw();
                    }
                }
            }
            RenderingState::RenderingTeardown => {
                if let (Some(mut viz), Some(gl)) = (viz_system.take(), viz_gl.as_ref()) {
                    viz.teardown(gl);
                }
                viz_gl = None;
                drop(underlay.take());
            }
            _ => {}
        }
    });

    if let Err(e) = result {
        log::error!("gfx: failed to install rendering notifier: {e}");
    }
}
