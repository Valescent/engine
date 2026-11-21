//! CPU-only prototype for the far-field heightfield / clipmap ring idea.
//!
//! The prototype answers three questions without a GPU:
//!
//! * How much geometry does a five-level horizon ring need?
//! * How large is the interpolation error at the seam between adjacent levels?
//! * Does an 8 m downward skirt cover that error?
//!
//! Every level samples the same continuous [`height`] field, so vertices that
//! happen to share an `(x, z)` position always carry the same height.

use std::fmt;

/// Cells per side of every clipmap level.
pub const SIZE: usize = 64;
/// Number of clipmap levels.
pub const LEVELS: usize = 5;
/// Cell size of level 0, in metres.
pub const BASE_CELL: f32 = 1.0;
/// Depth of the vertical skirt that hides a ring seam, in metres.
pub const SKIRT_DEPTH: f32 = 8.0;
/// The prototype is only interesting if it stays well under full resolution.
pub const BUDGET_FRACTION: f64 = 0.05;

/// Word used by the fixed hash seed. No randomness, no time, no environment.
const SEED: u64 = 0x5EED_0F7A_11CE_5A11;

const HOLE_MIN: usize = SIZE / 4;
const HOLE_MAX: usize = HOLE_MIN + SIZE / 2;

const BASE_HEIGHT: f32 = 20.0;
const MAX_HEIGHT: f32 = 40.0;

struct Octave {
    wavelength: f32,
    amplitude: f32,
    offset: [f32; 2],
}

/// Three octaves of value noise. The amplitudes sum to 20 m around a 20 m
/// base, so the field stays in `[0, 40]` metres.
const OCTAVES: [Octave; 3] = [
    Octave {
        wavelength: 64.0,
        amplitude: 12.0,
        offset: [0.0, 0.0],
    },
    Octave {
        wavelength: 16.0,
        amplitude: 6.0,
        offset: [37.25, -19.75],
    },
    Octave {
        wavelength: 4.0,
        amplitude: 2.0,
        offset: [-11.5, 41.25],
    },
];

/// A deterministic height sample in metres.
pub fn height(x: f32, z: f32) -> f32 {
    let mut h = BASE_HEIGHT;
    for (index, octave) in OCTAVES.iter().enumerate() {
        let noise = value_noise(
            x + octave.offset[0],
            z + octave.offset[1],
            octave.wavelength,
            index as u64 + 1,
        );
        h += octave.amplitude * (2.0 * noise - 1.0);
    }
    h
}

fn value_noise(x: f32, z: f32, wavelength: f32, salt: u64) -> f32 {
    let fx = x / wavelength;
    let fz = z / wavelength;
    let x0 = fx.floor();
    let z0 = fz.floor();
    let tx = smoothstep(fx - x0);
    let tz = smoothstep(fz - z0);
    let x_index = x0 as i64;
    let z_index = z0 as i64;

    let v00 = hash01(x_index, z_index, salt);
    let v10 = hash01(x_index + 1, z_index, salt);
    let v01 = hash01(x_index, z_index + 1, salt);
    let v11 = hash01(x_index + 1, z_index + 1, salt);

    lerp(lerp(v00, v10, tx), lerp(v01, v11, tx), tz)
}

fn hash01(x_index: i64, z_index: i64, salt: u64) -> f32 {
    let mixed = splitmix64(
        SEED ^ salt
            ^ (x_index as u64).wrapping_mul(0xA24B_AED4_963E_E407)
            ^ (z_index as u64).wrapping_mul(0x9FB2_1C65_1E98_DF25),
    );
    ((mixed >> 40) as f32) * (1.0 / 16_777_216.0)
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Snap a world-space centre to the lattice of `cell`.
pub fn snapped_center(camera: [f32; 2], cell: f32) -> [f32; 2] {
    [
        (camera[0] / cell).floor() * cell,
        (camera[1] / cell).floor() * cell,
    ]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub pos: [f32; 3],
}

/// One clipmap level.
///
/// `surface_*` is the visible heightfield mesh. `skirt_vertices` contains only
/// the added bottom vertices; `skirt_triangles` index into the concatenation
/// `surface_vertices ++ skirt_vertices` so the skirt reuses the ring's inner
/// boundary vertices.
#[derive(Clone, Debug)]
pub struct Level {
    pub level: usize,
    pub cell: f32,
    pub center: [f32; 2],
    pub origin: [f32; 2],
    pub extent: f32,
    pub ring_cells: usize,
    pub surface_vertices: Vec<Vertex>,
    pub surface_triangles: Vec<[u32; 3]>,
    pub skirt_vertices: Vec<Vertex>,
    pub skirt_triangles: Vec<[u32; 3]>,
    pub max_seam_gap: f32,
}

impl Level {
    pub fn vertices(&self) -> usize {
        self.surface_vertices.len() + self.skirt_vertices.len()
    }

    pub fn surface_triangles(&self) -> usize {
        self.surface_triangles.len()
    }

    pub fn skirt_triangles(&self) -> usize {
        self.skirt_triangles.len()
    }

    pub fn triangles(&self) -> usize {
        self.surface_triangles.len() + self.skirt_triangles.len()
    }

    /// Whether the snapped square of this level covers `(x, z)`.
    pub fn contains(&self, x: f32, z: f32) -> bool {
        x >= self.origin[0]
            && x < self.origin[0] + self.extent
            && z >= self.origin[1]
            && z < self.origin[1] + self.extent
    }
}

#[derive(Clone, Debug)]
pub struct Clipmap {
    pub camera: [f32; 2],
    pub levels: Vec<Level>,
    pub max_seam_gap: f32,
}

/// Build the full clipmap for one camera position.
pub fn build(camera: [f32; 2]) -> Clipmap {
    let mut levels = Vec::with_capacity(LEVELS);
    let mut max_seam_gap = 0.0_f32;

    for level_index in 0..LEVELS {
        let cell = BASE_CELL * (1u32 << level_index) as f32;
        let center = snapped_center(camera, cell);
        let extent = SIZE as f32 * cell;
        let origin = [center[0] - extent * 0.5, center[1] - extent * 0.5];
        let level = build_level(level_index, cell, center, origin);
        max_seam_gap = max_seam_gap.max(level.max_seam_gap);
        levels.push(level);
    }

    Clipmap {
        camera,
        levels,
        max_seam_gap,
    }
}

fn build_level(level: usize, cell: f32, center: [f32; 2], origin: [f32; 2]) -> Level {
    let stride = SIZE + 1;
    let mut index_at = vec![None; stride * stride];
    let mut surface_vertices = Vec::with_capacity(stride * stride);
    let is_ring = level > 0;

    for gz in 0..=SIZE {
        for gx in 0..=SIZE {
            let strictly_inside_hole =
                is_ring && gx > HOLE_MIN && gx < HOLE_MAX && gz > HOLE_MIN && gz < HOLE_MAX;
            if strictly_inside_hole {
                continue;
            }

            let x = origin[0] + gx as f32 * cell;
            let z = origin[1] + gz as f32 * cell;
            let index = surface_vertices.len() as u32;
            surface_vertices.push(Vertex {
                pos: [x, height(x, z), z],
            });
            index_at[gz * stride + gx] = Some(index);
        }
    }

    let mut surface_triangles = Vec::with_capacity(SIZE * SIZE * 2);
    for cz in 0..SIZE {
        for cx in 0..SIZE {
            let in_hole =
                is_ring && (HOLE_MIN..HOLE_MAX).contains(&cx) && (HOLE_MIN..HOLE_MAX).contains(&cz);
            if in_hole {
                continue;
            }

            let a = index_at[cz * stride + cx].expect("grid vertex a");
            let b = index_at[cz * stride + cx + 1].expect("grid vertex b");
            let c = index_at[(cz + 1) * stride + cx].expect("grid vertex c");
            let d = index_at[(cz + 1) * stride + cx + 1].expect("grid vertex d");

            // Counter-clockwise when viewed from +Y.
            surface_triangles.push([a, c, d]);
            surface_triangles.push([a, d, b]);
        }
    }

    let mut skirt_vertices = Vec::new();
    let mut skirt_triangles = Vec::new();
    let mut max_seam_gap = 0.0_f32;

    if is_ring {
        let surface_len = surface_vertices.len() as u32;
        let mut bottom_at = vec![None; stride * stride];

        // One bottom vertex per vertex on the inner boundary of the hole.
        for gz in HOLE_MIN..=HOLE_MAX {
            for gx in HOLE_MIN..=HOLE_MAX {
                let on_boundary =
                    gx == HOLE_MIN || gx == HOLE_MAX || gz == HOLE_MIN || gz == HOLE_MAX;
                if !on_boundary {
                    continue;
                }
                let top = index_at[gz * stride + gx].expect("hole boundary vertex");
                let position = surface_vertices[top as usize].pos;
                let bottom = skirt_vertices.len() as u32;
                skirt_vertices.push(Vertex {
                    pos: [position[0], position[1] - SKIRT_DEPTH, position[2]],
                });
                bottom_at[gz * stride + gx] = Some(bottom);
            }
        }

        let mut edges = Vec::with_capacity(SIZE / 2 * 4);
        for i in HOLE_MIN..HOLE_MAX {
            edges.push((i, HOLE_MIN, i + 1, HOLE_MIN));
            edges.push((i, HOLE_MAX, i + 1, HOLE_MAX));
            edges.push((HOLE_MIN, i, HOLE_MIN, i + 1));
            edges.push((HOLE_MAX, i, HOLE_MAX, i + 1));
        }

        for (gx0, gz0, gx1, gz1) in edges {
            let top0 = index_at[gz0 * stride + gx0].expect("skirt top 0");
            let top1 = index_at[gz1 * stride + gx1].expect("skirt top 1");
            let bottom0 = surface_len + bottom_at[gz0 * stride + gx0].expect("skirt bottom 0");
            let bottom1 = surface_len + bottom_at[gz1 * stride + gx1].expect("skirt bottom 1");

            skirt_triangles.push([top0, bottom0, bottom1]);
            skirt_triangles.push([top0, bottom1, top1]);

            // The coarse edge is linearly interpolated; the finer grid samples
            // the true field at the edge midpoint. Their difference is the
            // seam that the skirt has to hide.
            let p0 = surface_vertices[top0 as usize].pos;
            let p1 = surface_vertices[top1 as usize].pos;
            let mid_x = (p0[0] + p1[0]) * 0.5;
            let mid_z = (p0[2] + p1[2]) * 0.5;
            let interpolated = (p0[1] + p1[1]) * 0.5;
            let fine_height = height(mid_x, mid_z);
            max_seam_gap = max_seam_gap.max((fine_height - interpolated).abs());
        }
    }

    Level {
        level,
        cell,
        center,
        origin,
        extent: SIZE as f32 * cell,
        ring_cells: surface_triangles.len() / 2,
        surface_vertices,
        surface_triangles,
        skirt_vertices,
        skirt_triangles,
        max_seam_gap,
    }
}

#[derive(Clone, Debug)]
pub struct LevelReport {
    pub level: usize,
    pub cell: f32,
    pub extent: f32,
    pub ring_cells: usize,
    pub vertices: usize,
    pub surface_triangles: usize,
    pub skirt_triangles: usize,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub camera: [f32; 2],
    pub levels: Vec<LevelReport>,
    pub total_vertices: usize,
    pub total_surface_triangles: usize,
    pub total_skirt_triangles: usize,
    pub total_triangles: usize,
    pub full_resolution_triangles: usize,
    pub reduction_factor: f64,
    pub fraction_of_full_resolution: f64,
    pub max_seam_gap: f32,
    pub skirt_depth: f32,
}

impl Report {
    pub fn new(clipmap: &Clipmap) -> Self {
        let levels: Vec<LevelReport> = clipmap
            .levels
            .iter()
            .map(|level| LevelReport {
                level: level.level,
                cell: level.cell,
                extent: level.extent,
                ring_cells: level.ring_cells,
                vertices: level.vertices(),
                surface_triangles: level.surface_triangles(),
                skirt_triangles: level.skirt_triangles(),
            })
            .collect();

        let total_vertices = levels.iter().map(|level| level.vertices).sum();
        let total_surface_triangles = levels.iter().map(|level| level.surface_triangles).sum();
        let total_skirt_triangles = levels.iter().map(|level| level.skirt_triangles).sum();
        let total_triangles = total_surface_triangles + total_skirt_triangles;

        // A 1 m grid spanning the outermost level.
        let outer_side_cells = clipmap
            .levels
            .last()
            .map_or(0, |level| (level.extent / BASE_CELL).round() as usize);
        let full_resolution_triangles = outer_side_cells * outer_side_cells * 2;

        let reduction_factor = if total_triangles == 0 {
            0.0
        } else {
            full_resolution_triangles as f64 / total_triangles as f64
        };
        let fraction_of_full_resolution = if full_resolution_triangles == 0 {
            0.0
        } else {
            total_triangles as f64 / full_resolution_triangles as f64
        };

        Self {
            camera: clipmap.camera,
            levels,
            total_vertices,
            total_surface_triangles,
            total_skirt_triangles,
            total_triangles,
            full_resolution_triangles,
            reduction_factor,
            fraction_of_full_resolution,
            max_seam_gap: clipmap.max_seam_gap,
            skirt_depth: SKIRT_DEPTH,
        }
    }

    pub fn within_budget(&self) -> bool {
        self.fraction_of_full_resolution < BUDGET_FRACTION
    }

    pub fn skirt_covers_gap(&self) -> bool {
        self.max_seam_gap <= self.skirt_depth
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "far heightfield ring: camera=({:.3}, {:.3})",
            self.camera[0], self.camera[1]
        )?;
        writeln!(
            f,
            "{:>5} {:>8} {:>9} {:>10} {:>10} {:>10} {:>10}",
            "level", "cell(m)", "extent(m)", "ring cells", "vertices", "triangles", "skirt tris"
        )?;
        for level in &self.levels {
            writeln!(
                f,
                "{:>5} {:>8.1} {:>9.1} {:>10} {:>10} {:>10} {:>10}",
                level.level,
                level.cell,
                level.extent,
                level.ring_cells,
                level.vertices,
                level.surface_triangles,
                level.skirt_triangles
            )?;
        }
        writeln!(
            f,
            "totals: vertices={} surface_triangles={} skirt_triangles={} total_triangles={}",
            self.total_vertices,
            self.total_surface_triangles,
            self.total_skirt_triangles,
            self.total_triangles
        )?;
        writeln!(
            f,
            "full-resolution 1 m grid over the outermost extent: {} triangles",
            self.full_resolution_triangles
        )?;
        writeln!(
            f,
            "reduction: {:.2}x, {:.2}% of full resolution (budget < {:.2}%: {})",
            self.reduction_factor,
            self.fraction_of_full_resolution * 100.0,
            BUDGET_FRACTION * 100.0,
            if self.within_budget() { "ok" } else { "over" }
        )?;
        writeln!(f, "max_seam_gap: {:.3} m", self.max_seam_gap)?;
        writeln!(
            f,
            "skirt_depth: {:.3} m (covers gap: {})",
            self.skirt_depth,
            if self.skirt_covers_gap() { "yes" } else { "no" }
        )?;
        Ok(())
    }
}

const LEVEL_TINTS: [[f32; 3]; LEVELS] = [
    [0.55, 0.80, 1.00],
    [0.60, 1.00, 0.65],
    [1.00, 0.95, 0.55],
    [1.00, 0.65, 0.45],
    [0.85, 0.60, 1.00],
];

/// Rasterise a top-down binary PPM. Every pixel picks the innermost snapped
/// square that covers it, shades by height, and tints by level.
pub fn preview_ppm(clipmap: &Clipmap, size: usize) -> Vec<u8> {
    assert!(size > 0, "preview must have at least one pixel");
    let outer = clipmap
        .levels
        .last()
        .expect("a clipmap always has at least one level");
    let min_x = outer.origin[0];
    let min_z = outer.origin[1];
    let span = outer.extent;

    let header = format!("P6\n{size} {size}\n255\n");
    let mut bytes = Vec::with_capacity(header.len() + size * size * 3);
    bytes.extend_from_slice(header.as_bytes());

    for py in 0..size {
        let z = min_z + (py as f32 + 0.5) / size as f32 * span;
        for px in 0..size {
            let x = min_x + (px as f32 + 0.5) / size as f32 * span;
            let level = clipmap
                .levels
                .iter()
                .find(|level| level.contains(x, z))
                .map_or(LEVELS - 1, |level| level.level)
                .min(LEVELS - 1);

            let normalized_height = (height(x, z) / MAX_HEIGHT).clamp(0.0, 1.0);
            let shade = 0.20 + 0.80 * normalized_height;
            for channel in LEVEL_TINTS[level] {
                let value = (shade * channel * 255.0).round().clamp(0.0, 255.0) as u8;
                bytes.push(value);
            }
        }
    }

    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn shared_coordinate_key(value: f32) -> u32 {
        if value == 0.0 { 0 } else { value.to_bits() }
    }

    #[test]
    fn height_is_deterministic_and_bounded() {
        let samples = [
            [0.0, 0.0],
            [12.5, -7.25],
            [-31.75, 22.125],
            [1023.5, -511.25],
        ];
        for [x, z] in samples {
            let first = height(x, z);
            assert_eq!(first.to_bits(), height(x, z).to_bits());
            assert!(
                (-0.01..=MAX_HEIGHT + 0.01).contains(&first),
                "height({x}, {z}) = {first} is outside [0, {MAX_HEIGHT}]"
            );
        }
        assert_ne!(height(0.0, 0.0), height(0.5, 0.0));
    }

    #[test]
    fn all_five_levels_are_present_and_ordered() {
        let clipmap = build([0.0, 0.0]);
        assert_eq!(clipmap.levels.len(), LEVELS);
        for (index, level) in clipmap.levels.iter().enumerate() {
            let expected_cell = BASE_CELL * (1u32 << index) as f32;
            assert_eq!(level.level, index);
            assert_eq!(level.cell, expected_cell);
            assert_eq!(level.extent, SIZE as f32 * expected_cell);
            assert_eq!(level.center, [0.0, 0.0]);
            assert_eq!(level.origin, [-level.extent * 0.5, -level.extent * 0.5]);
        }
    }

    #[test]
    fn triangle_counts_match_the_spec() {
        let clipmap = build([0.0, 0.0]);
        let full_triangles = SIZE * SIZE * 2;
        let ring_triangles = (SIZE * SIZE - (SIZE / 2) * (SIZE / 2)) * 2;
        let skirt_triangles = (SIZE / 2) * 4 * 2;
        let full_vertices = (SIZE + 1) * (SIZE + 1);
        let ring_vertices = full_vertices - (SIZE / 2 - 1) * (SIZE / 2 - 1);

        for level in &clipmap.levels {
            if level.level == 0 {
                assert_eq!(level.surface_triangles.len(), full_triangles);
                assert_eq!(level.surface_vertices.len(), full_vertices);
                assert_eq!(level.skirt_triangles.len(), 0);
                assert_eq!(level.skirt_vertices.len(), 0);
                assert_eq!(level.ring_cells, SIZE * SIZE);
            } else {
                assert_eq!(level.surface_triangles.len(), ring_triangles);
                assert_eq!(level.surface_vertices.len(), ring_vertices);
                assert_eq!(level.skirt_triangles.len(), skirt_triangles);
                assert_eq!(level.skirt_vertices.len(), (SIZE / 2) * 4);
                assert_eq!(level.ring_cells, SIZE * SIZE - (SIZE / 2) * (SIZE / 2));
            }
            assert_eq!(
                level.vertices(),
                level.surface_vertices.len() + level.skirt_vertices.len()
            );
            assert_eq!(
                level.triangles(),
                level.surface_triangles.len() + level.skirt_triangles.len()
            );
        }
    }

    #[test]
    fn snapping_is_stable_within_a_cell_and_shifts_by_one_cell() {
        let base = build([0.0, 0.0]);

        for (index, level) in base.levels.iter().enumerate() {
            let cell = level.cell;

            let nudged = build([0.3 * cell, 0.3 * cell]);
            assert_eq!(
                nudged.levels[index].origin, level.origin,
                "0.3 cell should not shift level {index}"
            );

            let moved = build([1.3 * cell, 1.3 * cell]);
            assert_eq!(
                moved.levels[index].origin[0],
                level.origin[0] + cell,
                "1.3 cells should shift level {index} x by one cell"
            );
            assert_eq!(
                moved.levels[index].origin[1],
                level.origin[1] + cell,
                "1.3 cells should shift level {index} z by one cell"
            );
        }
    }

    #[test]
    fn shared_vertices_have_equal_heights() {
        let clipmap = build([0.0, 0.0]);
        let mut seen: HashMap<(u32, u32), f32> = HashMap::new();
        let mut shared = 0usize;

        for level in &clipmap.levels {
            for vertex in &level.surface_vertices {
                let key = (
                    shared_coordinate_key(vertex.pos[0]),
                    shared_coordinate_key(vertex.pos[2]),
                );
                match seen.get(&key) {
                    Some(expected) => {
                        assert_eq!(
                            expected.to_bits(),
                            vertex.pos[1].to_bits(),
                            "shared vertex at {:?} has mismatched height",
                            (vertex.pos[0], vertex.pos[2])
                        );
                        shared += 1;
                    }
                    None => {
                        seen.insert(key, vertex.pos[1]);
                    }
                }
            }
        }

        assert!(shared > 0, "expected the levels to share boundary vertices");
    }

    #[test]
    fn skirt_depth_covers_the_measured_seam() {
        let clipmap = build([0.0, 0.0]);
        assert!(
            clipmap.max_seam_gap > 0.0,
            "seam measurement should not be trivially zero"
        );
        assert!(
            clipmap.max_seam_gap < SKIRT_DEPTH,
            "max seam gap {:.3} m does not stay below skirt depth {:.3} m",
            clipmap.max_seam_gap,
            SKIRT_DEPTH
        );
        for level in &clipmap.levels {
            assert!(
                level.max_seam_gap < SKIRT_DEPTH,
                "level {} seam gap {:.3} m does not stay below skirt depth {:.3} m",
                level.level,
                level.max_seam_gap,
                SKIRT_DEPTH
            );
        }
    }

    #[test]
    fn skirt_covers_seams_for_several_camera_positions() {
        let cameras = [
            [0.0, 0.0],
            [0.37, -0.81],
            [-4.71, 3.53],
            [15.25, 31.75],
            [-63.3, 127.9],
        ];
        for camera in cameras {
            let clipmap = build(camera);
            assert!(
                clipmap.max_seam_gap < SKIRT_DEPTH,
                "camera {camera:?} gap {:.3} m does not stay below skirt depth {:.3} m",
                clipmap.max_seam_gap,
                SKIRT_DEPTH
            );
        }
    }

    #[test]
    fn geometry_is_under_five_percent_of_full_resolution() {
        let clipmap = build([0.0, 0.0]);
        let report = Report::new(&clipmap);

        assert_eq!(
            report.full_resolution_triangles,
            (SIZE << (LEVELS - 1)) * (SIZE << (LEVELS - 1)) * 2
        );
        assert!(
            report.within_budget(),
            "{:.3}% is not under {:.1}%",
            report.fraction_of_full_resolution * 100.0,
            BUDGET_FRACTION * 100.0
        );
        assert!(
            report.total_triangles < report.full_resolution_triangles / 20,
            "total triangles {} should be less than 5% of {}",
            report.total_triangles,
            report.full_resolution_triangles
        );
        assert!(report.reduction_factor > 20.0);
    }

    #[test]
    fn report_matches_the_built_meshes() {
        let clipmap = build([3.5, -9.25]);
        let report = Report::new(&clipmap);
        assert_eq!(report.levels.len(), LEVELS);

        for (level_report, level) in report.levels.iter().zip(&clipmap.levels) {
            assert_eq!(level_report.level, level.level);
            assert_eq!(level_report.cell, level.cell);
            assert_eq!(level_report.extent, level.extent);
            assert_eq!(level_report.vertices, level.vertices());
            assert_eq!(level_report.surface_triangles, level.surface_triangles());
            assert_eq!(level_report.skirt_triangles, level.skirt_triangles());
        }

        assert_eq!(
            report.total_vertices,
            clipmap.levels.iter().map(Level::vertices).sum::<usize>()
        );
        assert_eq!(
            report.total_surface_triangles,
            clipmap
                .levels
                .iter()
                .map(Level::surface_triangles)
                .sum::<usize>()
        );
        assert_eq!(
            report.total_skirt_triangles,
            clipmap
                .levels
                .iter()
                .map(Level::skirt_triangles)
                .sum::<usize>()
        );
    }

    #[test]
    fn preview_is_a_deterministic_binary_ppm() {
        let clipmap = build([0.0, 0.0]);
        let ppm = preview_ppm(&clipmap, 64);
        let header = b"P6\n64 64\n255\n";
        assert!(ppm.starts_with(header));
        assert_eq!(ppm.len(), header.len() + 64 * 64 * 3);
        assert_eq!(ppm, preview_ppm(&clipmap, 64));
        assert!(ppm[header.len()..].iter().any(|&byte| byte != 0));
    }
}
