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

use std::time::Instant;

use glow::HasContext;
use slint::{ComponentHandle, Global, GraphicsAPI, RenderingState};

use crate::{AppWindow, Theme};

/// Number of latitude parallels (excluding the poles).
const PARALLELS: usize = 7;
/// Number of longitude meridians.
const MERIDIANS: usize = 14;
/// Segments per parallel circle.
const PARALLEL_SEG: usize = 64;
/// Segments per meridian half-circle (pole to pole).
const MERIDIAN_SEG: usize = 32;

/// GLSL ES 1.00 vertex shader: rotates the sphere on two axes over time and
/// applies a simple perspective projection (aspect-corrected).
const VERTEX_SHADER: &str = r"#version 100
attribute vec3 pos;
uniform float u_time;
uniform float u_aspect;

void main() {
    float a = u_time * 0.45;          // spin around Y
    float b = u_time * 0.17 + 0.5;    // slow tilt around X

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

/// Owns the GL program, vertex buffer and uniform locations for the wireframe
/// sphere. Created on `RenderingSetup`, dropped on `RenderingTeardown`.
struct Underlay {
    gl: glow::Context,
    program: glow::Program,
    vbo: glow::Buffer,
    vertex_count: i32,
    pos_location: u32,
    u_time: glow::UniformLocation,
    u_aspect: glow::UniformLocation,
    u_color: glow::UniformLocation,
}

impl Underlay {
    fn new(gl: glow::Context) -> Self {
        unsafe {
            let program = gl.create_program().expect("create_program");

            let shader_sources = [
                (glow::VERTEX_SHADER, VERTEX_SHADER),
                (glow::FRAGMENT_SHADER, FRAGMENT_SHADER),
            ];
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

            let vertices = build_sphere_wireframe();
            let vertex_count = (vertices.len() / 3) as i32;

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
            let u_color = gl.get_uniform_location(program, "u_color").expect("uniform u_color");

            Self {
                gl,
                program,
                vbo,
                vertex_count,
                pos_location,
                u_time,
                u_aspect,
                u_color,
            }
        }
    }

    /// Clear the framebuffer to black and, when `enabled`, draw the rotating
    /// wireframe sphere in `color` for the given viewport size and time.
    fn render(&self, width: u32, height: u32, time: f32, color: (f32, f32, f32), enabled: bool) {
        let gl = &self.gl;
        unsafe {
            gl.viewport(0, 0, width as i32, height as i32);
            gl.disable(glow::DEPTH_TEST);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);

            if !enabled {
                return;
            }

            gl.use_program(Some(self.program));

            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.enable_vertex_attrib_array(self.pos_location);
            gl.vertex_attrib_pointer_f32(self.pos_location, 3, glow::FLOAT, false, 0, 0);

            gl.uniform_1_f32(Some(&self.u_time), time);
            let aspect = if height == 0 { 1.0 } else { width as f32 / height as f32 };
            gl.uniform_1_f32(Some(&self.u_aspect), aspect);
            gl.uniform_3_f32(Some(&self.u_color), color.0, color.1, color.2);

            gl.draw_arrays(glow::LINES, 0, self.vertex_count);

            // Restore the bits femtovg does not unconditionally reset itself.
            gl.disable_vertex_attrib_array(self.pos_location);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.use_program(None);
        }
    }
}

impl Drop for Underlay {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_program(self.program);
            self.gl.delete_buffer(self.vbo);
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

/// Install the wireframe-sphere underlay on `window`.
///
/// Sets a single rendering notifier that manages the GL resources across the
/// renderer lifecycle and animates the sphere while `gfx-bg-enabled` is set.
pub(crate) fn install(window: &AppWindow) {
    let weak = window.as_weak();
    let start = Instant::now();
    let mut underlay: Option<Underlay> = None;

    let result = window.window().set_rendering_notifier(move |state, graphics_api| {
        match state {
            RenderingState::RenderingSetup => {
                let context = match graphics_api {
                    GraphicsAPI::NativeOpenGL { get_proc_address } => unsafe {
                        glow::Context::from_loader_function_cstr(|s| get_proc_address(s))
                    },
                    _ => {
                        log::error!("gfx: unexpected graphics API; wireframe underlay disabled");
                        return;
                    }
                };
                underlay = Some(Underlay::new(context));
            }
            RenderingState::BeforeRendering => {
                if let (Some(underlay), Some(win)) = (underlay.as_ref(), weak.upgrade()) {
                    let enabled = win.get_gfx_bg_enabled();
                    let size = win.window().size();
                    let time = start.elapsed().as_secs_f32();
                    let color = theme_color(&win);
                    underlay.render(size.width, size.height, time, color, enabled);
                    if enabled {
                        // Keep the animation going.
                        win.window().request_redraw();
                    }
                }
            }
            RenderingState::RenderingTeardown => {
                drop(underlay.take());
            }
            _ => {}
        }
    });

    if let Err(e) = result {
        log::error!("gfx: failed to install rendering notifier: {e}");
    }
}
