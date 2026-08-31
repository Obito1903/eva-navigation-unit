//! OpenGL ES 2.0 backend.
//!
//! Expands the draw list into a dynamic triangle buffer and runs the same
//! three-pass pipeline the original VFD renderer used: geometry into an
//! offscreen FBO, an additive bloom composite, then a sharp copy on top.
//!
//! Segment shapes are carved in the fragment shader from a local `[-1, 1]` UV
//! so that one program covers every shape, rather than one program per
//! visualizer.

use std::num::NonZeroU32;
use std::rc::Rc;

use glow::HasContext;

use super::{Frame, VizBackend};
use crate::visualizer::instance::DrawList;
use crate::visualizer::{as_u8_slice, build_program, BLOOM_FRAG, COPY_FRAG, GRID_FRAG, QUAD_VERT};

/// `pos(2) + uv(2) + color(4) + shape(2)`
const FLOATS_PER_VERT: usize = 10;
const VERTS_PER_QUAD: usize = 6;
const STRIDE: i32 = (FLOATS_PER_VERT * 4) as i32;

const SEG_VERT: &str = r"#version 100
attribute vec2 a_pos;
attribute vec2 a_uv;
attribute vec4 a_color;
attribute vec2 a_shape;
varying vec2 v_uv;
varying vec4 v_color;
varying vec2 v_shape;
void main() {
    v_uv    = a_uv;
    v_color = a_color;
    v_shape = a_shape;
    gl_Position = vec4(a_pos, 0.0, 1.0);
}
";

const SEG_FRAG: &str = r"#version 100
precision mediump float;
uniform float u_scanline;
varying vec2 v_uv;      // local quad space, [-1,1]
varying vec4 v_color;   // premultiplied
varying vec2 v_shape;   // id, quad pixel aspect

void main() {
    float mask  = 1.0;
    float shade = 1.0;

    if (v_shape.x > 0.5) {
        // VFD: two outer blocks plus a thin centre spine.
        float ax     = abs(v_uv.x);
        float blocks = step(0.40, ax);
        float spine  = 1.0 - step(0.10, ax);
        mask = clamp(blocks + spine, 0.0, 1.0);
        // Soft vertical vignette; the spine reads slightly hotter.
        float dy = abs(v_uv.y);
        shade = (1.0 - 0.25 * dy * dy) * (1.0 + 0.20 * spine);
    }

    // Every other screen row is dimmed, giving the phosphor dither.
    float row = mod(floor(gl_FragCoord.y / 2.0), 2.0);
    mask *= 1.0 - row * u_scanline;

    gl_FragColor = vec4(v_color.rgb * shade, v_color.a) * mask;
}
";

struct GlState {
    seg_program: glow::Program,
    seg_pos: u32,
    seg_uv: u32,
    seg_color: u32,
    seg_shape: u32,
    seg_u_scanline: glow::UniformLocation,
    seg_vbo: glow::Buffer,
    /// Capacity in floats, grown on demand.
    seg_vbo_cap: usize,

    quad_vbo: glow::Buffer,

    bloom_program: glow::Program,
    bloom_pos: u32,
    bloom_u_tex: glow::UniformLocation,
    bloom_u_texel: glow::UniformLocation,
    bloom_u_radius: glow::UniformLocation,
    bloom_u_strength: glow::UniformLocation,

    copy_program: glow::Program,
    copy_pos: u32,
    copy_u_tex: glow::UniformLocation,

    grid_program: glow::Program,
    grid_pos: u32,
    grid_u_color: glow::UniformLocation,
    grid_u_width: glow::UniformLocation,
    grid_u_height: glow::UniformLocation,

    fbo: glow::Framebuffer,
    fbo_tex: glow::Texture,
    fbo_w: u32,
    fbo_h: u32,
}

pub struct GlBackend {
    gl: Rc<glow::Context>,
    state: Option<GlState>,
    /// Reused vertex scratch, so a 60 Hz loop does not allocate.
    verts: Vec<f32>,
}

impl GlBackend {
    pub fn new(gl: Rc<glow::Context>) -> Self {
        Self { gl, state: None, verts: Vec::new() }
    }

    /// Expands the draw list into triangles in NDC.
    fn build_vertices(&mut self, list: &DrawList, frame: Frame) {
        let vp = frame.viewport;
        let (fw, fh) = (frame.width.max(1) as f32, frame.height.max(1) as f32);
        self.verts.clear();
        self.verts
            .reserve(list.len() * VERTS_PER_QUAD * FLOATS_PER_VERT);

        for inst in &list.instances {
            // Centre: per-axis normalised viewport space -> pixels.
            let cx = vp.x + inst.center[0] * vp.w;
            let cy = vp.y + inst.center[1] * vp.h;
            // Extents are height-normalised on both axes, so both scale by h.
            let hx = inst.half_size[0] * vp.h;
            let hy = inst.half_size[1] * vp.h;
            if hx <= 0.0 || hy <= 0.0 {
                continue;
            }

            // rotation 0 points "up" (-y in screen space).
            let (sin_a, cos_a) = inst.rotation.sin_cos();
            let up = [sin_a, -cos_a];
            let right = [cos_a, sin_a];

            let corner = |sx: f32, sy: f32| {
                let px = cx + right[0] * hx * sx + up[0] * hy * sy;
                let py = cy + right[1] * hx * sx + up[1] * hy * sy;
                [px / fw * 2.0 - 1.0, 1.0 - py / fh * 2.0]
            };

            // Premultiply so the shader's coverage multiply stays correct.
            let a = inst.color[3].clamp(0.0, 1.0);
            let boost = 1.0 + inst.glow.max(0.0);
            let rgb = [
                (inst.color[0] * boost).min(1.0) * a,
                (inst.color[1] * boost).min(1.0) * a,
                (inst.color[2] * boost).min(1.0) * a,
            ];
            let shape = [inst.shape.id(), hx / hy];

            // Two triangles: (tl, bl, br), (tl, br, tr).
            const UVS: [[f32; 2]; 6] = [
                [-1.0, 1.0],
                [-1.0, -1.0],
                [1.0, -1.0],
                [-1.0, 1.0],
                [1.0, -1.0],
                [1.0, 1.0],
            ];
            for uv in UVS {
                let p = corner(uv[0], uv[1]);
                self.verts.extend_from_slice(&[
                    p[0], p[1], uv[0], uv[1], rgb[0], rgb[1], rgb[2], a, shape[0], shape[1],
                ]);
            }
        }
    }
}

impl VizBackend for GlBackend {
    fn setup(&mut self, frame: Frame) {
        let gl = self.gl.clone();
        unsafe {
            let seg_program = build_program(&gl, SEG_VERT, SEG_FRAG);
            let bloom_program = build_program(&gl, QUAD_VERT, BLOOM_FRAG);
            let copy_program = build_program(&gl, QUAD_VERT, COPY_FRAG);
            let grid_program = build_program(&gl, QUAD_VERT, GRID_FRAG);

            let quad_vbo = gl.create_buffer().expect("quad vbo");
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(quad_vbo));
            let quad: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_u8_slice(&quad), glow::STATIC_DRAW);

            let seg_vbo = gl.create_buffer().expect("seg vbo");

            let fbo = gl.create_framebuffer().expect("viz fbo");
            let fbo_tex = gl.create_texture().expect("viz fbo tex");

            let mut state = GlState {
                seg_pos: gl.get_attrib_location(seg_program, "a_pos").expect("a_pos") ,
                seg_uv: gl.get_attrib_location(seg_program, "a_uv").expect("a_uv"),
                seg_color: gl.get_attrib_location(seg_program, "a_color").expect("a_color"),
                seg_shape: gl.get_attrib_location(seg_program, "a_shape").expect("a_shape"),
                seg_u_scanline: gl
                    .get_uniform_location(seg_program, "u_scanline")
                    .expect("u_scanline"),
                seg_program,
                seg_vbo,
                seg_vbo_cap: 0,
                quad_vbo,
                bloom_pos: gl.get_attrib_location(bloom_program, "pos").expect("bloom pos"),
                bloom_u_tex: gl.get_uniform_location(bloom_program, "u_tex").unwrap(),
                bloom_u_texel: gl.get_uniform_location(bloom_program, "u_texel").unwrap(),
                bloom_u_radius: gl.get_uniform_location(bloom_program, "u_radius").unwrap(),
                bloom_u_strength: gl.get_uniform_location(bloom_program, "u_strength").unwrap(),
                bloom_program,
                copy_pos: gl.get_attrib_location(copy_program, "pos").expect("copy pos"),
                copy_u_tex: gl.get_uniform_location(copy_program, "u_tex").unwrap(),
                copy_program,
                grid_pos: gl.get_attrib_location(grid_program, "pos").expect("grid pos"),
                grid_u_color: gl.get_uniform_location(grid_program, "u_grid_color").unwrap(),
                grid_u_width: gl.get_uniform_location(grid_program, "u_width").unwrap(),
                grid_u_height: gl.get_uniform_location(grid_program, "u_height").unwrap(),
                grid_program,
                fbo,
                fbo_tex,
                fbo_w: 0,
                fbo_h: 0,
            };
            state.ensure_fbo(&gl, frame.width, frame.height);
            self.state = Some(state);
        }
    }

    fn draw(&mut self, list: &DrawList, frame: Frame) {
        if self.state.is_none() {
            return;
        }
        self.build_vertices(list, frame);

        let gl = self.gl.clone();
        let Some(s) = &mut self.state else { return };
        let (w, h) = (frame.width.max(1), frame.height.max(1));

        unsafe {
            s.ensure_fbo(&gl, w, h);

            let prev_fbo = {
                let raw = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
                NonZeroU32::new(raw as u32).map(glow::NativeFramebuffer)
            };

            // ── Pass 1: grid + segments → offscreen FBO ───────────────────
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(s.fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(s.fbo_tex),
                0,
            );
            gl.viewport(0, 0, w as i32, h as i32);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::BLEND);
            let bg = list.background;
            gl.clear_color(bg[0], bg[1], bg[2], bg[3]);
            gl.clear(glow::COLOR_BUFFER_BIT);

            gl.enable(glow::BLEND);
            // Premultiplied source.
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);

            if let Some(grid) = list.postfx.grid {
                gl.use_program(Some(s.grid_program));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(s.quad_vbo));
                gl.enable_vertex_attrib_array(s.grid_pos);
                gl.vertex_attrib_pointer_f32(s.grid_pos, 2, glow::FLOAT, false, 0, 0);
                gl.uniform_3_f32(Some(&s.grid_u_color), grid[0], grid[1], grid[2]);
                gl.uniform_1_f32(Some(&s.grid_u_width), w as f32);
                gl.uniform_1_f32(Some(&s.grid_u_height), h as f32);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                gl.disable_vertex_attrib_array(s.grid_pos);
            }

            if !self.verts.is_empty() {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(s.seg_vbo));
                if self.verts.len() > s.seg_vbo_cap {
                    gl.buffer_data_u8_slice(
                        glow::ARRAY_BUFFER,
                        as_u8_slice(&self.verts),
                        glow::DYNAMIC_DRAW,
                    );
                    s.seg_vbo_cap = self.verts.len();
                } else {
                    gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, as_u8_slice(&self.verts));
                }

                gl.use_program(Some(s.seg_program));
                gl.uniform_1_f32(Some(&s.seg_u_scanline), list.postfx.scanline.clamp(0.0, 1.0));
                gl.enable_vertex_attrib_array(s.seg_pos);
                gl.vertex_attrib_pointer_f32(s.seg_pos, 2, glow::FLOAT, false, STRIDE, 0);
                gl.enable_vertex_attrib_array(s.seg_uv);
                gl.vertex_attrib_pointer_f32(s.seg_uv, 2, glow::FLOAT, false, STRIDE, 8);
                gl.enable_vertex_attrib_array(s.seg_color);
                gl.vertex_attrib_pointer_f32(s.seg_color, 4, glow::FLOAT, false, STRIDE, 16);
                gl.enable_vertex_attrib_array(s.seg_shape);
                gl.vertex_attrib_pointer_f32(s.seg_shape, 2, glow::FLOAT, false, STRIDE, 32);

                let count = (self.verts.len() / FLOATS_PER_VERT) as i32;
                gl.draw_arrays(glow::TRIANGLES, 0, count);

                gl.disable_vertex_attrib_array(s.seg_pos);
                gl.disable_vertex_attrib_array(s.seg_uv);
                gl.disable_vertex_attrib_array(s.seg_color);
                gl.disable_vertex_attrib_array(s.seg_shape);
            }

            // ── Pass 2: additive bloom onto the target framebuffer ────────
            gl.bind_framebuffer(glow::FRAMEBUFFER, prev_fbo);
            gl.viewport(0, 0, w as i32, h as i32);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE);

            gl.use_program(Some(s.bloom_program));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(s.quad_vbo));
            gl.enable_vertex_attrib_array(s.bloom_pos);
            gl.vertex_attrib_pointer_f32(s.bloom_pos, 2, glow::FLOAT, false, 0, 0);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(s.fbo_tex));
            gl.uniform_1_i32(Some(&s.bloom_u_tex), 0);
            gl.uniform_2_f32(Some(&s.bloom_u_texel), 1.0 / w as f32, 1.0 / h as f32);
            gl.uniform_1_f32(Some(&s.bloom_u_radius), list.postfx.bloom_radius);
            gl.uniform_1_f32(Some(&s.bloom_u_strength), list.postfx.bloom_strength);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.disable_vertex_attrib_array(s.bloom_pos);

            // ── Pass 3: sharp copy on top ─────────────────────────────────
            gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
            gl.use_program(Some(s.copy_program));
            gl.enable_vertex_attrib_array(s.copy_pos);
            gl.vertex_attrib_pointer_f32(s.copy_pos, 2, glow::FLOAT, false, 0, 0);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(s.fbo_tex));
            gl.uniform_1_i32(Some(&s.copy_u_tex), 0);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.disable_vertex_attrib_array(s.copy_pos);

            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.use_program(None);
            gl.disable(glow::BLEND);
        }
    }

    fn teardown(&mut self) {
        let gl = self.gl.clone();
        if let Some(s) = self.state.take() {
            unsafe {
                gl.delete_program(s.seg_program);
                gl.delete_program(s.bloom_program);
                gl.delete_program(s.copy_program);
                gl.delete_program(s.grid_program);
                gl.delete_buffer(s.seg_vbo);
                gl.delete_buffer(s.quad_vbo);
                gl.delete_framebuffer(s.fbo);
                gl.delete_texture(s.fbo_tex);
            }
        }
    }
}

impl GlState {
    unsafe fn ensure_fbo(&mut self, gl: &glow::Context, w: u32, h: u32) {
        if self.fbo_w == w && self.fbo_h == h {
            return;
        }
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.fbo_tex));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                w as i32,
                h as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
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
        }
        self.fbo_w = w;
        self.fbo_h = h;
    }
}
