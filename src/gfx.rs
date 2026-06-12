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

/// Width of the left navigation sidebar, in logical pixels (matches
/// `ui/components/sidebar.slint`). The sphere is offset right by this so it
/// centers within the content area rather than the whole window.
const SIDEBAR_W: f32 = 96.0;

/// GLSL ES 1.00 vertex shader: rotates the sphere on two axes over time and
/// applies a simple perspective projection (aspect-corrected).
const VERTEX_SHADER: &str = r"#version 100
attribute vec3 pos;
uniform float u_time;
uniform float u_aspect;
uniform vec2 u_offset;

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
                if let (Some(underlay), Some(win)) = (underlay.as_mut(), weak.upgrade()) {
                    let enabled = win.get_gfx_bg_enabled();
                    let size = win.window().size();
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
