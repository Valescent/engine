//! The per-column biome dither (`E3.3`, `E3.5`).
//!
//! A column's surface material must follow the same `biome_pick` the climate
//! tile exposes, the pick must not depend on generator instance or column
//! order, and the share of `second` picks must fall off with distance from a
//! border. Coverage is sampled over the whole finite world so the dither cannot
//! quietly retire a biome.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use terrain_core::climate::DITHER_DOMAIN;
use terrain_core::description::{MACRO_CELL, SEA, TILE_BLOCKS};
use terrain_core::hash::domain;
use terrain_core::pipeline::{AIR, Terrain, WATER};

const SEED: u64 = 0x5EED;

/// The seed set `terrain-gen sweep --tier pr` covers (`PR_SEEDS`): `1..=16`.
/// The coverage floor is per seed, so the test samples every seed the PR sweep
/// claims, not a subset.
const PR_SEEDS: u64 = 16;
/// World half extent in blocks; the finite world is `[-1024, 1024)` on both
/// axes, which is the whole world `calibrate-blend` measures by default.
const WORLD_HALF_EXTENT: i32 = 1024;
/// Coverage sample stride. The floor is an area share, so a coarse stride is
/// enough; 8 blocks is the stride the coverage test has always used and it
/// keeps the thinnest seed (9, desert) clear of sampling noise. Building the
/// field tiles, not the sampling loop, dominates the wall time, so a coarser
/// stride does not pay.
const COVERAGE_STEP: i32 = 8;
/// Enough land samples that the 5% floor needs at least 1,000 hits at stride 8.
const MIN_LAND_SAMPLES: u64 = 20_000;
/// At most eight seeds generate at once. Each seed caches the whole finite
/// world, so this bounds the test at roughly 200 MB of tiles while still
/// using a typical developer machine; slower runners just use fewer workers.
const COVERAGE_WORKERS: usize = 8;
/// `docs/specs/terrain.md` section 8.3: each climate biome present on a seed
/// must cover at least 5% of land (`E3.1`, `E3.2`).
const COVERAGE_FLOOR_PERCENT: f64 = 5.0;
/// The v1 climate biomes in stable id order (`E3.2`).
const BIOME_NAMES: [&str; 4] = ["plains", "forest", "desert", "tundra"];

fn tile_and_local(x: i32, z: i32) -> (i32, i32, i32, i32) {
    let tile_x = x.div_euclid(TILE_BLOCKS);
    let tile_z = z.div_euclid(TILE_BLOCKS);
    (
        tile_x,
        tile_z,
        x - tile_x * TILE_BLOCKS,
        z - tile_z * TILE_BLOCKS,
    )
}

/// The expression `biome_pick` documents, recomputed from public data so the
/// production helper is checked rather than mirrored by a copy of itself.
fn pick(terrain: &Terrain, x: i32, z: i32) -> u8 {
    let (tile_x, tile_z, local_x, local_z) = tile_and_local(x, z);
    let tile = terrain.tile(tile_x, tile_z).expect("a world tile");
    tile.climate.biome_pick(
        local_x.div_euclid(MACRO_CELL),
        local_z.div_euclid(MACRO_CELL),
        x,
        z,
        terrain.seed(),
        domain(DITHER_DOMAIN),
    )
}

fn best(terrain: &Terrain, x: i32, z: i32) -> u8 {
    let (tile_x, tile_z, local_x, local_z) = tile_and_local(x, z);
    let tile = terrain.tile(tile_x, tile_z).expect("a world tile");
    tile.climate.best(
        local_x.div_euclid(MACRO_CELL),
        local_z.div_euclid(MACRO_CELL),
    )
}

fn top_solid(column: &[u16; 384]) -> (i32, u16) {
    let y = column
        .iter()
        .rposition(|&id| id != AIR && id != WATER)
        .expect("a generated column has a solid floor");
    (
        i32::try_from(y).expect("a column index fits i32"),
        column[y],
    )
}

#[test]
fn column_surface_follows_the_dithered_pick() {
    let terrain = Terrain::new(SEED).expect("the v1 description compiles");
    let materials = terrain.materials();

    // A desert/plains border on land near (-872, -100). `best` is desert on both
    // sides but the dither flips two columns to plains; their topsoil follows
    // the pick, not the nearest centre.
    let cases = [
        (-875, -100, 2_u8, materials.sand, 2_u8),
        (-871, -100, 0, materials.grass, 2),
        (-870, -100, 2, materials.sand, 2),
        (-869, -100, 0, materials.grass, 2),
        (-868, -100, 1, materials.grass, 1),
    ];
    let mut overridden = 0;
    for (x, z, expected, topsoil, nearest) in cases {
        assert_eq!(
            terrain.biome_at(x, z).expect("the tile resolves"),
            expected,
            "column pick at ({x}, {z})"
        );
        assert_eq!(pick(&terrain, x, z), expected, "biome_pick at ({x}, {z})");
        assert_eq!(
            best(&terrain, x, z),
            nearest,
            "nearest centre at ({x}, {z})"
        );
        if expected != nearest {
            overridden += 1;
        }

        let column = terrain.column(x, z).expect("the column generates");
        let (top, block) = top_solid(&column);
        assert!(top > SEA, "({x}, {z}) must be land");
        assert_eq!(block, topsoil, "topsoil at ({x}, {z})");
    }
    assert!(
        overridden >= 2,
        "the fixture must exercise the dither overriding the nearest centre"
    );
}

#[test]
fn pick_is_deterministic_across_instances_and_order() {
    let coords: Vec<(i32, i32)> = (-875..-850).map(|x| (x, -98)).collect();
    let forward = Terrain::new(SEED).expect("the v1 description compiles");
    let reverse = Terrain::new(SEED).expect("the v1 description compiles");

    let forward: Vec<(u8, [u16; 384])> = coords
        .iter()
        .map(|&(x, z)| {
            (
                forward.biome_at(x, z).expect("the tile resolves"),
                forward.column(x, z).expect("the column generates"),
            )
        })
        .collect();
    let mut reverse: Vec<(u8, [u16; 384])> = coords
        .iter()
        .rev()
        .map(|&(x, z)| {
            (
                reverse.biome_at(x, z).expect("the tile resolves"),
                reverse.column(x, z).expect("the column generates"),
            )
        })
        .collect();
    reverse.reverse();
    assert_eq!(forward, reverse, "a second instance in reverse order");

    let picks: Vec<u8> = forward.iter().map(|(picked, _)| *picked).collect();
    assert!(
        picks.windows(2).any(|pair| pair[0] != pair[1]),
        "the transect must cross the border"
    );
}

#[test]
fn second_pick_fraction_falls_off_with_distance() {
    let terrain = Terrain::new(SEED).expect("the v1 description compiles");
    let mut bins = [(0_u64, 0_u64); 5];
    for tile_z in -2..1 {
        for tile_x in -8..-5 {
            let tile = terrain.tile(tile_x, tile_z).expect("a world tile");
            for cell_z in 0..32 {
                for cell_x in 0..32 {
                    let nearest = tile.climate.best(cell_x, cell_z);
                    if nearest == tile.climate.second(cell_x, cell_z) {
                        continue;
                    }
                    let q = usize::from(tile.climate.margin(cell_x, cell_z));
                    let bin = q * bins.len() / 256;
                    for dz in 0..MACRO_CELL {
                        for dx in 0..MACRO_CELL {
                            let x = tile_x * TILE_BLOCKS + cell_x * MACRO_CELL + dx;
                            let z = tile_z * TILE_BLOCKS + cell_z * MACRO_CELL + dz;
                            let picked = tile.climate.biome_pick(
                                cell_x,
                                cell_z,
                                x,
                                z,
                                SEED,
                                domain(DITHER_DOMAIN),
                            );
                            bins[bin].0 += 1;
                            if picked != nearest {
                                bins[bin].1 += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    let fractions: Vec<f64> = bins
        .iter()
        .map(|(total, second)| *second as f64 / *total as f64)
        .collect();
    assert!(
        bins.iter().all(|(total, _)| *total >= 1_000),
        "every bin needs a meaningful sample: {bins:?}"
    );
    assert!(
        fractions[0] > 0.30,
        "near-border second share {}",
        fractions[0]
    );
    assert!(
        fractions[4] < 0.10,
        "deep-interior second share {}",
        fractions[4]
    );
    for window in fractions.windows(2) {
        assert!(
            window[0] > window[1],
            "second share must fall with distance: {fractions:?}"
        );
    }
}

/// One seed's land sample and dithered biome counts over the finite world.
struct SeedCoverage {
    seed: u64,
    land: u64,
    counts: [u64; 4],
}

fn seed_coverage(seed: u64) -> SeedCoverage {
    let terrain = Terrain::new(seed).expect("the v1 description compiles");
    let step = usize::try_from(COVERAGE_STEP).expect("a positive stride");
    let mut land = 0_u64;
    let mut counts = [0_u64; 4];
    for z in (-WORLD_HALF_EXTENT..WORLD_HALF_EXTENT).step_by(step) {
        for x in (-WORLD_HALF_EXTENT..WORLD_HALF_EXTENT).step_by(step) {
            let (tile_x, tile_z, local_x, local_z) = tile_and_local(x, z);
            let tile = terrain.tile(tile_x, tile_z).expect("a world tile");
            if tile.height.top(local_x, local_z) <= SEA {
                continue;
            }
            land += 1;
            counts[usize::from(pick(&terrain, x, z))] += 1;
        }
    }
    SeedCoverage { seed, land, counts }
}

#[test]
fn biome_coverage_holds_for_pr_seeds() {
    let started = std::time::Instant::now();
    let mut observed = [false; 4];
    let seeds: Vec<u64> = (1..=PR_SEEDS).collect();

    // Tile generation dominates, so generate a few seeds at a time. Coverage
    // counts are integers and each worker owns its seed, so the result is
    // independent of the worker count.
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(COVERAGE_WORKERS);
    let mut reports = Vec::with_capacity(usize::try_from(PR_SEEDS).expect("a small seed set"));
    for batch in seeds.chunks(workers) {
        let batch_reports: Vec<SeedCoverage> = std::thread::scope(|scope| {
            let handles: Vec<_> = batch
                .iter()
                .map(|&seed| scope.spawn(move || seed_coverage(seed)))
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("a coverage worker panicked"))
                .collect()
        });
        reports.extend(batch_reports);
    }

    for report in &reports {
        let seed = report.seed;
        let land = report.land;
        let counts = report.counts;
        // A stride of 8 over the finite world gives roughly 26k land samples
        // per seed; the 5% floor then needs at least 1,300 hits, so sampling
        // noise cannot move a biome across it on its own.
        assert!(
            land > MIN_LAND_SAMPLES,
            "seed {seed} sampled only {land} land blocks; \
             expected more than {MIN_LAND_SAMPLES} at stride {COVERAGE_STEP}"
        );

        let percentages: Vec<f64> = counts
            .iter()
            .map(|&count| 100.0 * count as f64 / land as f64)
            .collect();
        let present: Vec<usize> = (0..4).filter(|&id| counts[id] > 0).collect();
        eprintln!("seed {seed}: land={land} {percentages:.2?}");

        assert!(
            present.len() >= 3,
            "seed {seed} has only {} of the four climate biomes; \
             coverage is plains {:.2}%, forest {:.2}%, desert {:.2}%, tundra {:.2}% \
             (spec requires at least 3 present)",
            present.len(),
            percentages[0],
            percentages[1],
            percentages[2],
            percentages[3],
        );

        for (id, &count) in counts.iter().enumerate() {
            if count == 0 {
                continue;
            }
            observed[id] = true;
            assert!(
                percentages[id] >= COVERAGE_FLOOR_PERCENT,
                "seed {seed} biome {id} ({}) covers {:.3}% of land \
                 ({count}/{land} samples); floor is {:.2}%",
                BIOME_NAMES[id],
                percentages[id],
                COVERAGE_FLOOR_PERCENT,
            );
        }
    }

    // The spec requires at least three per seed; v1 defines four, so the PR
    // seed set must still exercise every one of them at least once.
    let missing: Vec<&str> = (0..4)
        .filter(|&id| !observed[id])
        .map(|id| BIOME_NAMES[id])
        .collect();
    assert!(
        missing.is_empty(),
        "the PR seed set never covers {}; each v1 climate biome must appear on at least one seed",
        missing.join(", "),
    );

    eprintln!(
        "coverage swept {} seeds at stride {COVERAGE_STEP} in {:.1}s on {workers} workers",
        PR_SEEDS,
        started.elapsed().as_secs_f64(),
    );
}
