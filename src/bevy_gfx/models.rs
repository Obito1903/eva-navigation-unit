//! Procedural wireframe geometry for the Bevy background and nav icons.
//!
//! Every model is a flat list of line endpoints (`[a, b, a, b, ...]`) that
//! becomes a [`bevy::render::mesh::PrimitiveTopology::LineList`] mesh. The
//! shapes are the same NERV/"Magi" wireframes the OpenGL underlay used to draw,
//! but they are now real meshes lit by an emissive PBR material and bloom.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Mesh, PrimitiveTopology};

/// Number of latitude parallels (excluding the poles).
const PARALLELS: usize = 7;
/// Number of longitude meridians.
const MERIDIANS: usize = 14;
/// Segments per parallel circle.
const PARALLEL_SEG: usize = 64;
/// Segments per meridian half-circle (pole to pole).
const MERIDIAN_SEG: usize = 32;

/// Turn a line-endpoint list into a renderable `LineList` mesh.
///
/// `StandardMaterial`'s vertex layout always wants normals and UVs, so both are
/// filled with constants — the material is unlit, nothing reads them.
pub(crate) fn line_mesh(positions: Vec<[f32; 3]>) -> Mesh {
    let n = positions.len();
    Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default())
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; n])
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; n])
}

/// Append a line segment (two endpoints).
fn edge(v: &mut Vec<[f32; 3]>, a: [f32; 3], b: [f32; 3]) {
    v.push(a);
    v.push(b);
}

/// Append the 12 edges of an axis-aligned box spanning `min`..`max`.
fn box_edges(v: &mut Vec<[f32; 3]>, min: [f32; 3], max: [f32; 3]) {
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
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

/// Unit-sphere wireframe: `PARALLELS` latitude circles plus `MERIDIANS`
/// longitude half-circles.
pub(crate) fn sphere() -> Vec<[f32; 3]> {
    use std::f32::consts::PI;
    let mut v = Vec::new();

    let point = |phi: f32, theta: f32| -> [f32; 3] {
        [phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin()]
    };

    for i in 1..=PARALLELS {
        let phi = PI * (i as f32) / (PARALLELS as f32 + 1.0);
        for s in 0..PARALLEL_SEG {
            let t0 = 2.0 * PI * (s as f32) / (PARALLEL_SEG as f32);
            let t1 = 2.0 * PI * ((s + 1) as f32) / (PARALLEL_SEG as f32);
            edge(&mut v, point(phi, t0), point(phi, t1));
        }
    }

    for m in 0..MERIDIANS {
        let theta = 2.0 * PI * (m as f32) / (MERIDIANS as f32);
        for s in 0..MERIDIAN_SEG {
            let p0 = PI * (s as f32) / (MERIDIAN_SEG as f32);
            let p1 = PI * ((s + 1) as f32) / (MERIDIAN_SEG as f32);
            edge(&mut v, point(p0, theta), point(p1, theta));
        }
    }

    v
}

/// The 12 edges of a cube centered on the origin.
pub(crate) fn cube() -> Vec<[f32; 3]> {
    let mut v = Vec::new();
    box_edges(&mut v, [-0.8, -0.8, -0.8], [0.8, 0.8, 0.8]);
    v
}

/// Stylized wireframe sports car in the spirit of a Renault Alpine A310 — a
/// wedge fastback with a low pointed nose, raked windshield, short tapered
/// greenhouse and a long sloping tail.
pub(crate) fn car() -> Vec<[f32; 3]> {
    use std::f32::consts::PI;
    let mut v = Vec::new();

    // Side silhouette as a closed loop of `[x, y, half_width]` points.
    //   x: +front .. -rear,  y: up,  half_width: body half-thickness in z.
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
    for i in 0..n {
        let a = profile[i];
        let b = profile[(i + 1) % n];
        edge(&mut v, [a[0], a[1], -a[2]], [b[0], b[1], -b[2]]);
        edge(&mut v, [a[0], a[1], a[2]], [b[0], b[1], b[2]]);
        edge(&mut v, [a[0], a[1], -a[2]], [a[0], a[1], a[2]]);
    }

    // Greenhouse / side windows: a tapered glasshouse outline just inboard of
    // each flank so the cabin reads as glazed.
    let glass: [[f32; 2]; 5] = [
        [0.46, 0.10],  // A-pillar base
        [0.14, 0.39],  // A-pillar top
        [-0.34, 0.40], // C-pillar top
        [-0.56, 0.28], // C-pillar base
        [-0.30, 0.18], // belt line return
    ];
    let glass_hw = 0.40;
    for hw in [-glass_hw, glass_hw] {
        for i in 0..glass.len() {
            let a = glass[i];
            let b = glass[(i + 1) % glass.len()];
            edge(&mut v, [a[0], a[1], hw], [b[0], b[1], hw]);
        }
        // Door-glass divider (B-pillar) for a two-window look.
        edge(&mut v, [-0.06, 0.41, hw], [-0.06, 0.17, hw]);
    }

    // Longitudinal creases: belt line and a lower body crease.
    let belt = [[1.08_f32, -0.02], [0.48, 0.06], [-0.30, 0.14], [-1.04, 0.02]];
    let lower = [[1.10_f32, -0.18], [0.40, -0.14], [-0.40, -0.12], [-1.06, -0.16]];
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

    // Front lights: a pair of small ellipses per side.
    for &(cx, cy, hw, r) in &[(1.02_f32, 0.04_f32, 0.30_f32, 0.07_f32), (1.02, 0.04, 0.42, 0.07)] {
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

    // Wheel arches + wheels.
    let wheel_r = 0.26;
    let arch_r = 0.32;
    let arch_hw = 0.50;
    for [cx, cy] in [[0.62_f32, -0.28], [-0.62, -0.28]] {
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
        for hw in [-0.50_f32, 0.50] {
            wheel_disc(&mut v, cx, cy, hw, wheel_r, 24);
        }
    }

    v
}

/// A wireframe wheel at `(cx, cy, z)`: tyre circle, hub circle and four spokes.
fn wheel_disc(v: &mut Vec<[f32; 3]>, cx: f32, cy: f32, z: f32, r: f32, seg: usize) {
    use std::f32::consts::PI;
    let hub = r * 0.4;
    for s in 0..seg {
        let a0 = 2.0 * PI * (s as f32) / (seg as f32);
        let a1 = 2.0 * PI * ((s + 1) as f32) / (seg as f32);
        edge(
            v,
            [cx + r * a0.cos(), cy + r * a0.sin(), z],
            [cx + r * a1.cos(), cy + r * a1.sin(), z],
        );
        edge(
            v,
            [cx + hub * a0.cos(), cy + hub * a0.sin(), z],
            [cx + hub * a1.cos(), cy + hub * a1.sin(), z],
        );
    }
    for k in 0..4 {
        let a = 2.0 * PI * (k as f32) / 4.0 + PI / 4.0;
        edge(
            v,
            [cx + hub * a.cos(), cy + hub * a.sin(), z],
            [cx + r * a.cos(), cy + r * a.sin(), z],
        );
    }
}

/// Hi-fi speaker: a tall cabinet with a recessed baffle, a woofer, a tweeter
/// and a bass-reflex port on the front face.
pub(crate) fn speaker() -> Vec<[f32; 3]> {
    use std::f32::consts::PI;
    let mut v = Vec::new();

    let (hx, hy, hz) = (0.55_f32, 0.9_f32, 0.5_f32);
    box_edges(&mut v, [-hx, -hy, -hz], [hx, hy, hz]);

    let bx = hx - 0.10;
    let by = hy - 0.10;
    let bz = hz; // front face
    let baffle = [[-bx, -by], [bx, -by], [bx, by], [-bx, by]];
    for i in 0..baffle.len() {
        let a = baffle[i];
        let b = baffle[(i + 1) % baffle.len()];
        edge(&mut v, [a[0], a[1], bz], [b[0], b[1], bz]);
    }

    let mut driver = |cx: f32, cy: f32, r: f32, depth: f32, seg: usize| {
        for s in 0..seg {
            let a0 = 2.0 * PI * (s as f32) / (seg as f32);
            let a1 = 2.0 * PI * ((s + 1) as f32) / (seg as f32);
            edge(
                &mut v,
                [cx + r * a0.cos(), cy + r * a0.sin(), bz],
                [cx + r * a1.cos(), cy + r * a1.sin(), bz],
            );
            let ri = r * 0.55;
            edge(
                &mut v,
                [cx + ri * a0.cos(), cy + ri * a0.sin(), bz - depth],
                [cx + ri * a1.cos(), cy + ri * a1.sin(), bz - depth],
            );
            if s % 3 == 0 {
                edge(
                    &mut v,
                    [cx + r * a0.cos(), cy + r * a0.sin(), bz],
                    [cx + ri * a0.cos(), cy + ri * a0.sin(), bz - depth],
                );
            }
        }
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

    driver(0.0, -0.30, 0.34, 0.14, 28);
    driver(0.0, 0.42, 0.13, 0.06, 20);

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

/// Deliberately simple centered car for the AUTO nav icon: a body box, a cabin
/// box and four wheel rings, kept low-poly so it stays legible at icon size.
pub(crate) fn car_icon() -> Vec<[f32; 3]> {
    use std::f32::consts::PI;
    let mut v = Vec::new();

    let hw = 0.40;
    box_edges(&mut v, [-0.85, -0.18, -hw], [0.85, 0.08, hw]);
    box_edges(&mut v, [-0.45, 0.08, -hw * 0.82], [0.35, 0.34, hw * 0.82]);

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

    // Fill more of the square icon frame (less empty margin → reads larger).
    for p in v.iter_mut() {
        p[0] *= 1.5;
        p[1] *= 1.5;
        p[2] *= 1.5;
    }

    v
}

/// 3D gear/cog for the SYS nav icon: two parallel toothed rings joined into a
/// short extruded disc, with a central bore.
pub(crate) fn gear_icon() -> Vec<[f32; 3]> {
    use std::f32::consts::PI;
    let mut v = Vec::new();

    let teeth = 8;
    let r_root = 0.52;
    let r_tip = 0.78;
    let r_bore = 0.20;
    let hz = 0.18;

    // Each tooth contributes a rise to the tip, a flat tip and a fall back.
    let steps = teeth * 4;
    let outline: Vec<[f32; 2]> = (0..steps)
        .map(|i| {
            let ang = 2.0 * PI * (i as f32 / steps as f32);
            let r = if i % 4 == 0 || i % 4 == 1 { r_tip } else { r_root };
            [r * ang.cos(), r * ang.sin()]
        })
        .collect();

    for &z in &[-hz, hz] {
        for i in 0..outline.len() {
            let a = outline[i];
            let b = outline[(i + 1) % outline.len()];
            edge(&mut v, [a[0], a[1], z], [b[0], b[1], z]);
        }
    }
    for a in &outline {
        edge(&mut v, [a[0], a[1], -hz], [a[0], a[1], hz]);
    }

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
