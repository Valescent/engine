//! Mesher timing fixtures for the v0.0.9 baseline.
//!
//! The ignored `bench_mesh_build` test times one chunk's greedy mesh at each LOD
//! on fixed fixtures: seed 42's spawn surface, its ground layer, a flat slab, and
//! a shoreline. The fixtures are deterministic and CPU-only, so the measurement
//! needs no window, GPU, or world files.
//!
//! Run with:
//!
//! ```text
//! cargo test --release --lib bench_mesh_build -- --ignored --nocapture
//! ```

#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use std::time::Instant;

use glam::{IVec2, IVec3, Vec3};

use crate::block::Block;
use crate::generation::Generator;
use crate::light::{self, LightSnapshot};
use crate::mesh::{BorderBlocks, ChunkSnapshot, Lod, MeshData, build_mesh, build_water_mesh};
use crate::section::{SectionEdits, SectionKey, generate_section};
use crate::streaming::Streaming;
use crate::world::{CHUNK_COLUMN, CHUNK_SIZE, CHUNK_STORAGE_BYTES, Chunk, ChunkCoord, World};

/// One timed fixture at one mesh pass.
#[derive(Debug)]
pub(crate) struct Row {
    /// Fixture name: `surface`, `ground`, `flat`, or `water`.
    fixture: &'static str,
    /// Pass name: `opaque` or `water`.
    pass: &'static str,
    /// Level name: `lod0`, `lod1`, or `lod2`.
    lod: &'static str,
    /// Recorded samples.
    samples: u32,
    /// Mean build time in microseconds.
    mean_us: f64,
    /// 95th percentile build time in microseconds.
    p95_us: f64,
    /// Quads in the measured mesh.
    quads: usize,
    /// ADR-0015 quad records emitted alongside the vertices.
    records: usize,
    /// Indices in the measured mesh.
    indices: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Fixture {
    Surface,
    Ground,
    Flat,
    Water,
    Air,
}

impl Fixture {
    fn name(self) -> &'static str {
        match self {
            Self::Surface => "surface",
            Self::Ground => "ground",
            Self::Flat => "flat",
            Self::Water => "water",
            Self::Air => "air",
        }
    }
}

/// Times every fixture at every LOD, with `samples` recorded runs each.
///
/// Each run meshes one chunk of a 3x3 column patch, so neighbour face culling
/// and cross-chunk light sampling match a settled world.
pub(crate) fn measure(samples: u32) -> Vec<Row> {
    let samples = samples.max(1);
    let mut rows = Vec::new();

    let (world, surface, ground) = generated_world();
    for (fixture, chunk) in [(Fixture::Surface, surface), (Fixture::Ground, ground)] {
        rows.extend(measure_chunk(&world, chunk, fixture, samples));
    }

    let (world, center) = synthetic_world(Fixture::Flat);
    rows.extend(measure_chunk(&world, center, Fixture::Flat, samples));

    let (world, center) = synthetic_world(Fixture::Water);
    rows.extend(measure_chunk(&world, center, Fixture::Water, samples));

    let (world, center) = synthetic_world(Fixture::Air);
    rows.extend(measure_chunk(&world, center, Fixture::Air, samples));

    rows
}

fn measure_chunk(world: &World, chunk: ChunkCoord, fixture: Fixture, samples: u32) -> Vec<Row> {
    let snapshot = ChunkSnapshot::new(world, chunk);
    let border = BorderBlocks::new(world, chunk);
    let light = world.light_field(chunk).cloned();
    let light = light.as_deref();

    let mut rows = Vec::with_capacity(4);
    for lod in [Lod::Lod0, Lod::Lod1, Lod::Lod2] {
        let (times, mesh) = time(samples, || build_mesh(&snapshot, &border, light, lod));
        rows.push(row(
            fixture,
            "opaque",
            lod_name(lod),
            samples,
            &times,
            &mesh,
        ));
    }
    let (times, mesh) = time(samples, || build_water_mesh(&snapshot, &border, light));
    rows.push(row(fixture, "water", "lod0", samples, &times, &mesh));
    rows
}

fn lod_name(lod: Lod) -> &'static str {
    match lod {
        Lod::Lod0 => "lod0",
        Lod::Lod1 => "lod1",
        Lod::Lod2 => "lod2",
    }
}

fn row(
    fixture: Fixture,
    pass: &'static str,
    lod: &'static str,
    samples: u32,
    times: &[f64],
    mesh: &MeshData,
) -> Row {
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    let mut sorted = times.to_vec();
    sorted.sort_by(f64::total_cmp);
    Row {
        fixture: fixture.name(),
        pass,
        lod,
        samples,
        mean_us: mean,
        p95_us: percentile(&sorted, 0.95),
        quads: mesh.vertices.len() / 4,
        records: mesh.quads.len(),
        indices: mesh.indices.len(),
    }
}

/// Runs two untimed warmups, then `samples` timed runs.
fn time(samples: u32, mut build: impl FnMut() -> MeshData) -> (Vec<f64>, MeshData) {
    for _ in 0..2 {
        std::hint::black_box(build());
    }
    let mut times = Vec::with_capacity(samples as usize);
    let mut last = None;
    for _ in 0..samples {
        let started = Instant::now();
        let mesh = build();
        times.push(started.elapsed().as_secs_f64() * 1e6);
        last = Some(mesh);
    }
    (times, last.expect("at least one sample"))
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

/// The spawn column of seed 42 and its two useful chunks: the surface chunk and
/// the bottom chunk.
fn generated_world() -> (World, ChunkCoord, ChunkCoord) {
    let generator = Generator::new(42);
    let (x, z, height) = generator.spawn_column();
    let column = Streaming::column_at(Vec3::new(
        x as f32 + 0.5,
        height as f32 + 1.0,
        z as f32 + 0.5,
    ));
    let mut world = World::default();
    for dx in -1..=1 {
        for dz in -1..=1 {
            for (chunk, data) in generator.generate_column(column + IVec2::new(dx, dz)) {
                world.insert_chunk(chunk, data);
            }
        }
    }
    world.relight();

    let surface_y = (height / CHUNK_SIZE).clamp(0, CHUNK_COLUMN - 1);
    let surface = ChunkCoord::new(column.x, surface_y, column.y);
    let ground = ChunkCoord::new(column.x, 0, column.y);
    (world, surface, ground)
}

fn synthetic_world(fixture: Fixture) -> (World, ChunkCoord) {
    let mut world = World::default();
    let center = ChunkCoord::new(0, 0, 0);
    for dx in -1..=1 {
        for dz in -1..=1 {
            let chunk = center.offset(IVec3::new(dx, 0, dz));
            world.insert_chunk(chunk, chunk_for(fixture));
        }
    }
    world.relight();
    (world, center)
}

fn chunk_for(fixture: Fixture) -> Chunk {
    let mut data = Chunk::default();
    if fixture == Fixture::Air {
        return data;
    }
    for x in 0..CHUNK_SIZE {
        for z in 0..CHUNK_SIZE {
            let top = if fixture == Fixture::Flat { 12 } else { 18 };
            // Restrict water to one half of the chunk so the 3x3 patch has a
            // shoreline between water columns and dry shore.
            let water_column = fixture == Fixture::Water && x < CHUNK_SIZE / 2;
            for y in 0..CHUNK_SIZE {
                let block = if y > top {
                    if water_column && y <= 24 {
                        Block::Water
                    } else {
                        Block::Air
                    }
                } else {
                    Block::Stone
                };
                if block != Block::Air {
                    data.set_block(IVec3::new(x, y, z), block);
                }
            }
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "benchmark; run with cargo test --release --lib bench_section -- --ignored --nocapture"]
    fn bench_section() {
        let generator = Generator::new(42);
        let (x, z, height) = generator.spawn_column();
        let level = 1;
        let extent = 32 * (1 << level);
        let key = SectionKey::new(
            level,
            IVec3::new(
                x.div_euclid(extent),
                height.div_euclid(extent),
                z.div_euclid(extent),
            ),
        );

        let edits = SectionEdits::new();
        for _ in 0..2 {
            std::hint::black_box(generate_section(&generator, key, &edits));
        }
        let mut generation = Vec::new();
        for _ in 0..10 {
            let started = Instant::now();
            let section = generate_section(&generator, key, &edits);
            generation.push(started.elapsed().as_secs_f64() * 1e6);
            std::hint::black_box(section);
        }
        generation.sort_by(f64::total_cmp);

        let section = generate_section(&generator, key, &edits);
        let light = section.light();
        for _ in 0..2 {
            std::hint::black_box(section.mesh(Some(&light)));
        }
        let mut mesh_times = Vec::new();
        let mut mesh = MeshData::default();
        for _ in 0..10 {
            let started = Instant::now();
            mesh = section.mesh(Some(&light));
            mesh_times.push(started.elapsed().as_secs_f64() * 1e6);
        }
        mesh_times.sort_by(f64::total_cmp);

        let origin = key.origin();
        let mut world = crate::world::World::default();
        for dy in 0..2 {
            for dx in 0..2 {
                for dz in 0..2 {
                    let chunk = ChunkCoord::new(
                        origin.x / CHUNK_SIZE + dx,
                        origin.y / CHUNK_SIZE + dy,
                        origin.z / CHUNK_SIZE + dz,
                    );
                    world.insert_chunk(chunk, generator.generate_chunk(chunk));
                }
            }
        }
        world.relight();
        let mut lod1_vertices = 0;
        let mut lod1_quads = 0;
        let mut lod1_time = 0.0;
        for dy in 0..2 {
            for dx in 0..2 {
                for dz in 0..2 {
                    let chunk = ChunkCoord::new(
                        origin.x / CHUNK_SIZE + dx,
                        origin.y / CHUNK_SIZE + dy,
                        origin.z / CHUNK_SIZE + dz,
                    );
                    let started = Instant::now();
                    let snapshot = ChunkSnapshot::new(&world, chunk);
                    let border = BorderBlocks::new(&world, chunk);
                    let light = world.light_field(chunk).cloned();
                    let chunk_mesh = build_mesh(&snapshot, &border, light.as_deref(), Lod::Lod1);
                    lod1_time += started.elapsed().as_secs_f64() * 1e6;
                    lod1_vertices += chunk_mesh.vertices.len();
                    lod1_quads += chunk_mesh.quads.len();
                }
            }
        }

        eprintln!(
            "SECTION key={:?} gen_us={:.1} p95_us={:.1} mesh_us={:.1} p95_us={:.1} quads={} vertices={} section_bytes={} light_bytes={} lod1_quads={} lod1_vertices={} lod1_us={:.1}",
            key,
            generation.iter().sum::<f64>() / generation.len() as f64,
            percentile(&generation, 0.95),
            mesh_times.iter().sum::<f64>() / mesh_times.len() as f64,
            percentile(&mesh_times, 0.95),
            mesh.quads.len(),
            mesh.vertices.len(),
            CHUNK_STORAGE_BYTES,
            light.resident_bytes(),
            lod1_quads,
            lod1_vertices,
            lod1_time,
        );
    }

    /// The same generation and mesh timing on a high-level surface section.
    ///
    /// A level-3 section spans 256 blocks vertically but its terrain occupies
    /// only the bottom third, which is the common shape of the far hierarchy
    /// target and the one a bounds-limited mesher should speed up most.
    #[test]
    #[ignore = "benchmark; run with cargo test --release --lib bench_section -- --ignored --nocapture"]
    fn bench_section_far() {
        let generator = Generator::new(42);
        let (x, z, height) = generator.spawn_column();
        let level = 3;
        let extent = 32 * (1 << level);
        let key = SectionKey::new(
            level,
            IVec3::new(
                x.div_euclid(extent),
                height.div_euclid(extent),
                z.div_euclid(extent),
            ),
        );

        let edits = SectionEdits::new();
        for _ in 0..2 {
            std::hint::black_box(generate_section(&generator, key, &edits));
        }
        let mut generation = Vec::new();
        for _ in 0..10 {
            let started = Instant::now();
            let section = generate_section(&generator, key, &edits);
            generation.push(started.elapsed().as_secs_f64() * 1e6);
            std::hint::black_box(section);
        }
        generation.sort_by(f64::total_cmp);

        let section = generate_section(&generator, key, &edits);
        let light = section.light();
        for _ in 0..2 {
            std::hint::black_box(section.mesh(Some(&light)));
        }
        let mut mesh_times = Vec::new();
        let mut mesh = MeshData::default();
        for _ in 0..10 {
            let started = Instant::now();
            mesh = section.mesh(Some(&light));
            mesh_times.push(started.elapsed().as_secs_f64() * 1e6);
        }
        mesh_times.sort_by(f64::total_cmp);

        for _ in 0..2 {
            std::hint::black_box(section.water_mesh(Some(&light)));
        }
        let mut water_times = Vec::new();
        let mut water = MeshData::default();
        for _ in 0..10 {
            let started = Instant::now();
            water = section.water_mesh(Some(&light));
            water_times.push(started.elapsed().as_secs_f64() * 1e6);
        }
        water_times.sort_by(f64::total_cmp);

        eprintln!(
            "SECTION_FAR key={:?} gen_us={:.1} p95_us={:.1} mesh_us={:.1} p95_us={:.1} quads={} vertices={} water_us={:.1} water_quads={} light_bytes={}",
            key,
            generation.iter().sum::<f64>() / generation.len() as f64,
            percentile(&generation, 0.95),
            mesh_times.iter().sum::<f64>() / mesh_times.len() as f64,
            percentile(&mesh_times, 0.95),
            mesh.quads.len(),
            mesh.vertices.len(),
            water_times.iter().sum::<f64>() / water_times.len() as f64,
            water.quads.len(),
            light.resident_bytes(),
        );
    }

    #[test]
    #[ignore = "benchmark; run with cargo test --release --lib bench_light -- --ignored --nocapture"]
    fn bench_light() {
        let (world, surface, ground) = generated_world();
        for (name, chunk) in [("surface", surface), ("ground", ground)] {
            let snapshot = LightSnapshot::new(&world, chunk);
            for _ in 0..2 {
                std::hint::black_box(light::compute(&snapshot, chunk));
            }
            let mut times = Vec::new();
            for _ in 0..20 {
                let started = Instant::now();
                std::hint::black_box(light::compute(&snapshot, chunk));
                times.push(started.elapsed().as_secs_f64() * 1e6);
            }
            times.sort_by(f64::total_cmp);
            let mean = times.iter().sum::<f64>() / times.len() as f64;

            let mut snapshot_times = Vec::new();
            for _ in 0..20 {
                let started = Instant::now();
                std::hint::black_box(LightSnapshot::new(&world, chunk));
                snapshot_times.push(started.elapsed().as_secs_f64() * 1e6);
            }
            snapshot_times.sort_by(f64::total_cmp);
            let snapshot_mean = snapshot_times.iter().sum::<f64>() / snapshot_times.len() as f64;

            // The old snapshot copied every stored chunk's packed storage.
            let mut copy_times = Vec::new();
            for _ in 0..20 {
                let started = Instant::now();
                let mut copied = 0;
                for dx in -1..=1 {
                    for dz in -1..=1 {
                        for dy in -1..CHUNK_COLUMN {
                            let coordinate = chunk.offset(IVec3::new(dx, dy, dz));
                            if let Some(data) = world.chunk(coordinate) {
                                let copy = data.shared_storage().to_vec();
                                std::hint::black_box(&copy);
                                copied += copy.len();
                            }
                        }
                    }
                }
                std::hint::black_box(copied);
                copy_times.push(started.elapsed().as_secs_f64() * 1e6);
            }
            copy_times.sort_by(f64::total_cmp);
            let copy_mean = copy_times.iter().sum::<f64>() / copy_times.len() as f64;

            eprintln!(
                "LIGHT chunk={name} samples={} mean_us={mean:.1} p95_us={:.1} snapshot_us={snapshot_mean:.1} copy_us={copy_mean:.1}",
                times.len(),
                percentile(&times, 0.95),
            );
        }
    }

    /// Counts the settled hierarchy target and how many frustum-visible
    /// sections the section horizon removes at the spawn view.
    #[test]
    #[ignore = "benchmark; run with cargo test --release --lib bench_horizon_sections -- --ignored --nocapture"]
    fn bench_horizon_sections() {
        use std::collections::HashMap;

        use crate::camera::Camera;
        use crate::frustum::Frustum;
        use crate::hierarchy::{DEFAULT_DETAIL_CHUNKS, DEFAULT_PIXELS_PER_RADIAN, Hierarchy};
        use crate::horizon::Horizon;
        use crate::player::EYE_HEIGHT;

        let generator = Generator::new(42);
        let (x, z, height) = generator.spawn_column();
        let spawn = Vec3::new(x as f32 + 0.5, (height + 1) as f32, z as f32 + 0.5);
        let mut hierarchy = Hierarchy::default();
        for _ in 0..16 {
            hierarchy.update(spawn, DEFAULT_DETAIL_CHUNKS, DEFAULT_PIXELS_PER_RADIAN);
            let queue = hierarchy.queue().to_vec();
            if queue.is_empty() {
                break;
            }
            for key in queue {
                hierarchy.mark_ready(key);
            }
        }
        hierarchy.update(spawn, DEFAULT_DETAIL_CHUNKS, DEFAULT_PIXELS_PER_RADIAN);
        let target = hierarchy.target().to_vec();

        let edits = SectionEdits::new();
        let mut bounds = HashMap::new();
        let gen_started = Instant::now();
        let mut scan_us = 0.0;
        for key in &target {
            let section = generate_section(&generator, *key, &edits);
            let scan_started = Instant::now();
            if let Some(section_bounds) = section.bounds() {
                bounds.insert(*key, section_bounds);
            }
            scan_us += scan_started.elapsed().as_secs_f64() * 1e6;
        }
        let gen_us = gen_started.elapsed().as_secs_f64() * 1e6;

        let mut camera = Camera::new(1280.0 / 720.0);
        camera.set_position(spawn + Vec3::Y * EYE_HEIGHT);
        camera.set_yaw(270.0_f32.to_radians());
        camera.set_far_plane(9_216.0);
        let frustum = Frustum::from_view_projection(camera.view_projection());
        let mut world = World::default();
        let horizon_started = Instant::now();
        let horizon = Horizon::build_with_sections(camera.position(), &mut world, &bounds);
        let horizon_us = horizon_started.elapsed().as_secs_f64() * 1e6;

        let mut candidates = 0;
        let mut occluded = 0;
        for key in &target {
            let min = key.origin();
            let extent = key.extent();
            if !frustum.intersects_aabb(min.as_vec3(), min.as_vec3() + Vec3::splat(extent as f32)) {
                continue;
            }
            candidates += 1;
            if horizon.occludes_section(*key) {
                occluded += 1;
            }
        }
        eprintln!(
            "HORIZON target={} bounds={} frustum={} occluded={} visible={} gen_us={gen_us:.0} scan_us={scan_us:.0} horizon_us={horizon_us:.0}",
            target.len(),
            bounds.len(),
            candidates,
            occluded,
            candidates - occluded
        );
    }

    #[test]
    #[ignore = "benchmark; run with cargo test --release --lib bench_mesh_build -- --ignored --nocapture"]
    fn bench_mesh_build() {
        for row in measure(30) {
            eprintln!(
                "MESH fixture={} pass={} lod={} samples={} mean_us={:.1} p95_us={:.1} quads={} records={} indices={}",
                row.fixture,
                row.pass,
                row.lod,
                row.samples,
                row.mean_us,
                row.p95_us,
                row.quads,
                row.records,
                row.indices
            );
        }
    }

    #[test]
    fn fixtures_mesh_to_stable_quad_counts() {
        let rows = measure(1);
        assert_eq!(rows.len(), 20);
        for row in &rows {
            assert_eq!(row.samples, 1);
            if row.pass == "opaque" && row.fixture != "air" {
                assert!(row.quads > 0, "{row:?} produced no quads");
            }
            assert_eq!(row.records, row.quads, "{row:?} record count");
        }
        let water = rows
            .iter()
            .find(|row| row.fixture == "water" && row.pass == "water")
            .expect("water fixture");
        assert!(water.quads > 0);
        let water_chunk = chunk_for(Fixture::Water);
        assert_eq!(
            water_chunk.block(IVec3::new(0, 20, 0)),
            Block::Water,
            "the water fixture needs an ocean column"
        );
        assert_eq!(
            water_chunk.block(IVec3::new(CHUNK_SIZE - 1, 20, 0)),
            Block::Air,
            "the water fixture needs a dry shore column"
        );
        let surface = rows
            .iter()
            .find(|row| row.fixture == "surface" && row.lod == "lod0" && row.pass == "opaque")
            .expect("surface LOD0");
        assert!(surface.quads > 100, "surface fixture is too simple");
    }
}
