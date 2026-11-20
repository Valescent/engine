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

    // A desert/plains border on land near (-870, -98). `best` is desert on both
    // sides but the dither flips two columns to plains; their topsoil follows
    // the pick, not the nearest centre.
    let cases = [
        (-870, -98, 0_u8, materials.grass, 2_u8),
        (-864, -96, 2, materials.sand, 2),
        (-863, -96, 0, materials.grass, 2),
        (-861, -96, 2, materials.sand, 2),
        (-860, -96, 0, materials.grass, 0),
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

#[test]
fn biome_coverage_holds_for_pr_seeds() {
    for seed in [1_u64, 2, SEED] {
        let terrain = Terrain::new(seed).expect("the v1 description compiles");
        let mut land = 0_u64;
        let mut counts = [0_u64; 4];
        for z in (-1024..1024).step_by(8) {
            for x in (-1024..1024).step_by(8) {
                let (tile_x, tile_z, local_x, local_z) = tile_and_local(x, z);
                let tile = terrain.tile(tile_x, tile_z).expect("a world tile");
                if tile.height.top(local_x, local_z) <= SEA {
                    continue;
                }
                land += 1;
                counts[usize::from(pick(&terrain, x, z))] += 1;
            }
        }

        assert!(land > 20_000, "seed {seed} sampled too little land");
        let present = counts.iter().filter(|&&count| count > 0).count();
        assert!(present >= 3, "seed {seed} has only {present} biomes");
        for (id, count) in counts.iter().enumerate() {
            if *count == 0 {
                continue;
            }
            assert!(
                *count * 20 >= land,
                "seed {seed} biome {id} covers {:.2}% of land",
                100.0 * *count as f64 / land as f64
            );
        }
    }
}
