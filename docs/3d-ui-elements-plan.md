# Plan: 3D OpenGL Elements in the Slint UI

## Goal

Add small 3D objects (a rotating sphere / box rendered via raymarched SDF
shaders) to the NERV head-unit UI. Two integration modes:

- **Texture-into-Image** — small positioned 3D accent widgets inside the layout.
- **Underlay** — a full-window animated 3D background behind all Slint content.
- **Stretch goal** — make the 3D element interactive (touch / data driven).

## Key Findings (verified)

- `slint 1.16.1` and `glow 0.17.0` are **both already in `Cargo.lock`** (femtovg
  pulls in `glow`).
- The active backend is **winit + femtovg (OpenGL) + software**. Skia is a
  *declared* but *not enabled* feature. With no explicit `BackendSelector` /
  `SLINT_BACKEND` in the code, Slint auto-selects winit + femtovg — exactly what
  the texture/underlay approach needs. **No backend swap required.**
- Recommended safety net: add an explicit
  `BackendSelector::new().require_opengl_es().select()` in `main` so a silent
  software-renderer fallback (where `NativeOpenGL` never fires) becomes a hard,
  visible failure.
- API confirmed for 1.16:
  - `Window::set_rendering_notifier(cb)`
  - `slint::RenderingState::{RenderingSetup, BeforeRendering, AfterRendering, RenderingTeardown}`
  - `slint::GraphicsAPI::NativeOpenGL { get_proc_address }`
  - `glow::Context::from_loader_function_cstr`
  - `slint::BorrowedOpenGLTextureBuilder::new_gl_2d_rgba_texture(tex_id, (w, h)).build()`
- Existing pattern to mirror: video frames are pushed to a Slint `<image>` prop
  (`video.rs` uses `SharedPixelBuffer` + `win.set_video_frame`).
- `ui.rs::wire()` is the central wiring point (UI thread, `Timer` +
  `invoke_from_event_loop`). `main.rs` creates `AppWindow`, calls `ui::wire`,
  then `run()`.
- `app.slint` `AppWindow`: `HorizontalLayout` of `[sidebar 96px | content]`. The
  content area renders the AA video / settings.

## Reference Examples (Slint master, same API)

- **`opengl_underlay/main.rs`** — `EGLUnderlay` struct; `RenderingSetup` builds
  the `glow` context; `BeforeRendering` renders a fullscreen quad + calls
  `request_redraw`. Shader is `#version 100` GLSL ES, fullscreen triangle-strip
  quad (4 verts), with `effect_time` / `rotation` uniforms.
- **`opengl_texture/main.rs`** — `DemoTexture` (FBO + texture); `DemoRenderer`
  renders an SDF rounded box into the FBO and returns a `slint::Image` via
  `BorrowedOpenGLTextureBuilder`; double-buffered (`displayed` / `next`).
  `scene.slint` binds `<image>` ⟷ `image.source` and reads back the requested
  texture width/height from the `Image` element size.

## Architecture Decision

New module `src/gfx.rs` (graphics / 3D) with two responsibilities:

1. `Underlay` renderer (fullscreen background SDF) — toggled by a UI bool.
2. `TextureRenderer` (offscreen FBO → `Image`) for the small accent(s).

Both share GLSL-ES raymarch helpers. Wire both through **one**
`set_rendering_notifier` closure installed in `gfx::install(&window)`, called
from `main` (or `ui::wire`).

State machine inside the notifier closure (owns `Option<GfxState>`):

- `RenderingSetup` → build `glow::Context`, construct `Underlay` + `TextureRenderer`.
- `BeforeRendering` → if background enabled: `underlay.render()`; always:
  `texture_renderer.render(size, params)` → `win.set_*texture*`; `request_redraw()`.
- `RenderingTeardown` → drop state.

## Slint UI Additions (`app.slint`)

- Props on `AppWindow`:
  - `in-out property <bool> gfx-bg-enabled: true;` (toggle the underlay background)
  - `in property <image> hud-orb;` (the small 3D accent texture)
  - *(interactive stretch)* `in-out property <float> orb-spin-speed: 1.0;`
- Place a small `Image { source: root.hud-orb; }` element — e.g. in the sidebar
  header area (augmenting / replacing the "NERV" glyph block), fixed size ~64×64px.
  Mirror the `opengl_texture` scene: expose `out property <int> hud-orb-w / h`
  from the `Image` size so Rust renders at native resolution.
- The underlay shows through only where Slint draws nothing / transparent. The
  NERV background is solid `#000000`, so when `gfx-bg-enabled` is on, make the
  content-area `Rectangle` background transparent so the GL underlay shows
  behind the (still opaque) sidebar.

## Steps

1. **Add explicit GL backend select** in `main.rs` before `AppWindow::new()`:
   `slint::BackendSelector::new().require_opengl_es().select()`. Keeps
   femtovg/GL guaranteed. *No new deps.*
2. **Add `glow` as a direct dep** in `a310/Cargo.toml` (`glow = "0.17"`) so it can
   be called directly (it is transitively present; declaring it makes it usable
   and locks the version).
3. **Create `src/gfx.rs`**:
   - GLSL-ES raymarch shader(s): rotating sphere SDF (+ optional box) with `time`
     and `spin-speed` uniforms, in the NERV red palette.
   - `struct Underlay` (program, VAO/VBO fullscreen quad, time + spin uniforms)
     with `render(&self, enabled, spin)`; scoped GL state save/restore.
   - `struct TextureRenderer` (FBO + texture double-buffer like
     `DemoTexture`/`DemoRenderer`) with `render(&mut self, w, h, spin) -> slint::Image`.
   - `struct GfxState { gl: Rc<glow::Context>, underlay: Underlay, orb: TextureRenderer }`.
   - `pub fn install(window: &AppWindow)`: sets the rendering-notifier closure,
     pulling `gfx-bg-enabled`, `orb-spin-speed`, `hud-orb-w/h` from the window and
     writing back the `hud-orb` image. Uses an app weak handle like the examples.
4. **Wire from `main`** (preferred) right after `ui::wire(&window, setup)` to keep
   gfx independent of the AA worker. *depends on steps 1–3.*
5. **Declare `mod gfx;`** in `main.rs`.
6. **Edit `app.slint`**: add the 3 props, the small `Image` accent (sidebar
   header), expose its width/height as out props, and gate the content-area
   transparency on `gfx-bg-enabled`. *parallel with step 3.*
7. **(Interactive stretch)** Add a `TouchArea` on the orb `Image` (or reuse a
   sidebar button) that bumps `orb-spin-speed`, and/or drive `orb-spin-speed`
   from the `aa-connected` state (e.g. spins faster when connected). Pure Slint +
   existing prop; minimal Rust. *depends on step 6.*

## Relevant Files

- `a310/src/main.rs` — add `mod gfx;`, backend select, call `gfx::install`.
- `a310/Cargo.toml` — add `glow = "0.17"` under `[dependencies]`.
- `a310/src/gfx.rs` — **new**. All GL/3D code. Model on `opengl_underlay` +
  `opengl_texture`.
- `a310/ui/app.slint` — new props + small `Image` accent + background
  transparency gating. The sidebar header "NERV" block is the natural home for
  the orb.
- `a310/src/video.rs` — reference only (`SharedPixelBuffer` / `set_video_frame`
  image pattern).
- `a310/src/ui.rs` — reference for wiring / weak-handle / timer patterns; possible
  call site.

## Verification

1. `cargo build` in `a310` — compiles with the new `glow` dep + `gfx` module.
2. `cargo run --release` — window shows the small rotating 3D orb in the sidebar
   header; no panic from the rendering notifier; CPU/GPU stable.
3. Toggle `gfx-bg-enabled` — confirm the animated SDF background appears behind
   content when on, solid black when off.
4. Confirm AA video still renders correctly in the content area (underlay/texture
   must not clobber femtovg state — verify scoped bind save/restore works; a black
   or corrupted video frame signals GL-state leakage).
5. *(Interactive)* Verify orb spin speed changes on the chosen trigger.
6. Run on the actual head-unit / target GL stack if available (GL ES vs desktop
   GL: shaders use `#version 100` ES which works on both via femtovg).

## Decisions / Scope

- **Included**: SDF raymarch sphere (no mesh files); both texture-widget +
  underlay; light interactivity via a spin-speed prop.
- **Excluded**: real polygon-mesh loading (glTF/OBJ), matrix-math crate, lighting
  models beyond simple shader shading, multiple distinct 3D widgets (start with
  one orb).
- **Risk**: GL state leakage between our GL code and femtovg → use the scoped
  save/restore binding pattern from the `opengl_texture` example verbatim.
- **Risk**: if a target ever forces the software renderer, `NativeOpenGL` won't
  fire; the explicit `require_opengl_es()` makes this a hard, visible failure
  instead of a silent no-3D. Acceptable; AA video also benefits from GL.

## Further Considerations

1. **Where should the orb live?**
   A) Sidebar header (replaces the "NERV" text) — compact, always visible.
   B) Content-area corner overlay.
   C) Settings view only.
   *Recommend A* — the persistent NERV status accent.
2. **Underlay background default?**
   A) On by default (always animated background).
   B) Off by default, opt-in.
   *Recommend B* — keep the AA video view pure black unless ambiance is enabled;
   expose a Settings toggle later.
3. **Interactivity trigger?**
   A) Tap the orb to cycle speed.
   B) Auto-spin faster when `aa-connected`.
   C) Both.
   *Recommend B* — ties the 3D to real state with zero new UI.
