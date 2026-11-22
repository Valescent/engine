//! Tests for the fixed-order column pipeline.
//!
//! These were the `tests` module of `pipeline.rs`; every test and assertion is
//! preserved by the split.

#![allow(clippy::cast_possible_truncation)]

use std::collections::BTreeSet;
use std::sync::Arc;

use super::stages::v1_biomes;
use super::*;
use crate::content::{self, ContentPack};
use crate::description::{MAX_TERRAIN_Y, SEA, TILE_BLOCKS};
use crate::error::ErrorKind;
use crate::features::{self, ApplyContext, ColumnField, Palette};
use crate::hash::domain;
use crate::metrics2;
use crate::section::{BlockId, Section};

fn terrain() -> Terrain {
    Terrain::new(0x5EED).unwrap()
}

/// `E3.2`: four climate biomes, ids `0..=3`, at the answered centres.
#[test]
fn v1_biomes_are_the_answered_four() {
    let table = v1_biomes();

    // The probes are the four answered centres, in id order. The argmin in
    // climate space must return that centre's own id, so this pins both the
    // centres and the id order at once.
    let centres = [
        (0_u8, 0.55_f32, 0.45_f32),
        (1, 0.50, 0.75),
        (2, 0.84, 0.15),
        (3, 0.10, 0.35),
    ];
    let mut seen = [false; 4];
    for (id, temp, humid) in centres {
        let (best, second, _margin) = table.nearest_two(temp, humid, 0.05);
        assert_eq!(best, id, "probe ({temp}, {humid})");
        assert_ne!(second, best, "probe ({temp}, {humid}) has no rival");
        seen[usize::from(id)] = true;
    }
    assert!(seen.iter().all(|&hit| hit), "a biome is missing");

    // `BiomeTable` exposes no length accessor, so the exact record count is
    // observed through the derived `Debug` shape: one `Biome {` per centre.
    // A fifth climate biome (for example `ocean` or `beach`) would show up.
    let rendered = format!("{table:?}");
    assert_eq!(rendered.matches("Biome {").count(), 4, "{rendered}");
}

#[test]
fn hydro_tiles_read_the_whole_world_solve() {
    let terrain = terrain();
    let inside = terrain.hydro_tile(0, 0).unwrap();
    assert!(
        matches!(inside, HydroTileRef::World(_)),
        "an in-grid tile must use world mode"
    );

    // The solved grid is -4..3 tiles; 4 is wholly outside it.
    let outside = terrain.hydro_tile(4, 4).unwrap();
    assert!(
        matches!(outside, HydroTileRef::Tile(_)),
        "a tile outside the world falls back to tile mode"
    );
}

#[test]
fn an_installed_world_hydro_is_shared_by_workers() {
    let terrain = terrain();
    let world = terrain.solve_world_hydro().unwrap();
    let other = Terrain::new(0x5EED)
        .unwrap()
        .with_world_hydro(Arc::clone(&world));

    let (x, z) = (7, -13);
    assert_eq!(
        terrain.hydro_tile(0, 0).unwrap().river_width(x, z),
        other.hydro_tile(0, 0).unwrap().river_width(x, z)
    );
}

#[test]
fn bedrock_is_solid_everywhere() {
    let terrain = terrain();
    let bedrock = terrain.materials().bedrock;
    for (x, z) in [(0, 0), (37, -91), (-512, 640), (2047, 2047)] {
        let column = terrain.column(x, z).unwrap();
        assert_eq!(column[0], bedrock, "({x},{z})");
    }
}

#[test]
fn tops_are_inside_the_world() {
    let terrain = terrain();
    for x in -70..70 {
        let column = terrain.column(x, 13).unwrap();
        let top = column.iter().rposition(|&id| id != AIR && id != WATER);
        let top = top.unwrap_or(0) as i32;
        assert!(top <= MAX_TERRAIN_Y, "top {top} above MAX_TERRAIN_Y at {x}");
        assert!(top >= 1);
    }
}

#[test]
fn water_never_exceeds_sea_unless_it_is_a_lake() {
    let terrain = terrain();
    let water = terrain.materials().water;
    for x in -80..80 {
        let column = terrain.column(x, 40).unwrap();
        let highest_water = column
            .iter()
            .rposition(|&id| id == water)
            .map_or(0, |y| y as i32);
        assert!(
            highest_water <= SEA,
            "water at {highest_water} above sea at x={x}"
        );
    }
}

#[test]
fn generation_is_deterministic() {
    let a = terrain().column(123, -45).unwrap();
    let b = terrain().column(123, -45).unwrap();
    assert_eq!(a, b);
}

#[test]
fn counters_do_not_change_generation() {
    const SEED: u64 = 0x5EED;
    let terrain = Terrain::new(SEED).expect("the v1 description compiles");
    let (x, z) = (123, -45);
    let first = terrain.column(x, z).expect("the column generates");

    // Heavy use: every recorded stage, a warm cache, hundreds of samples.
    for z in -20..20 {
        for x in -20..20 {
            let _ = terrain.column(x, z).expect("the column generates");
        }
    }
    let _ = terrain.section(1, 4, 0).expect("the section generates");
    assert!(
        terrain.counters().get("ready_columns").unwrap_or(0) > 0,
        "the heavy pass recorded no gauge, so the test is vacuous"
    );

    // Clear every pinned counter, then regenerate the same column. The tile
    // cache is warm here and the counters are zero, so this is the "cleared"
    // arm against the "heavy" arm.
    for spec in metrics2::METRICS {
        assert!(terrain.counters().set(spec.name, 0), "{}", spec.name);
    }
    let second = terrain.column(x, z).expect("the column generates");

    // A cold generator that has recorded nothing is the third arm.
    let fresh = Terrain::new(SEED).expect("the v1 description compiles");
    let third = fresh.column(x, z).expect("the column generates");

    assert_eq!(first, second, "clearing counters changed a column");
    assert_eq!(first, third, "a cold generator produced another column");

    // Sections are what the goldens pin; compare their content hashes too.
    let warmed = terrain.section(1, 4, 0).expect("the section generates");
    let cold = fresh.section(1, 4, 0).expect("the section generates");
    assert_eq!(
        warmed.content_hash(),
        cold.content_hash(),
        "diagnostics changed a section"
    );
}

#[test]
fn overlay_has_one_line_per_group() {
    let terrain = terrain();
    let _ = terrain.column(0, 0).expect("the column generates");

    let lines = terrain.overlay();
    assert_eq!(lines.len(), 8);
    let prefixes = [
        "revisions: ",
        "queues: ",
        "jobs: ",
        "memory: ",
        "lod: ",
        "light: ",
        "edits: ",
        "shed: ",
    ];
    for (line, prefix) in lines.iter().zip(prefixes) {
        assert!(line.starts_with(prefix), "expected {prefix}, got {line}");
    }
    assert_eq!(lines, terrain.overlay(), "the overlay is not stable");
    assert!(
        lines[1].contains("ready_columns=16"),
        "one 128-block tile is 16 columns: {}",
        lines[1]
    );
    assert!(lines[4].contains("lod0_columns=0"), "no section yet");
    assert!(lines[7].contains("step=Full"));
    assert!(lines[7].contains("radius=16"));

    let _ = terrain.section(0, 4, 0).expect("the section generates");
    let after = terrain.overlay();
    assert!(
        !after[4].contains("lod0_columns=0"),
        "the LOD0 gauge did not move: {}",
        after[4]
    );
    assert!(after[0].contains("applied=0"), "residency is not wired yet");
}

#[test]
fn every_block_id_is_a_known_block() {
    let terrain = terrain();
    let pack = content::v1_default();
    for x in 0..40 {
        let column = terrain.column(x, 7).unwrap();
        for (y, &id) in column.iter().enumerate() {
            assert!(pack.props(id).is_some(), "unknown id {id} at ({x},{y})");
        }
    }
}

#[test]
fn sections_assemble_from_the_same_columns() {
    let terrain = terrain();
    let section = terrain.section(0, 4, 0).unwrap();
    assert!(section.solid_count() > 0);
    let column = terrain.column(5, 9).unwrap();
    assert_eq!(section.get(5, 15, 9), column[4 * 32 + 15]);
}

#[test]
fn the_world_edge_is_ocean() {
    let terrain = terrain();
    let half = terrain.half_extent;
    for (x, z) in [
        (half - 1, 0),
        (-half + 1, 0),
        (0, half - 1),
        (0, -half + 1),
        (half - 1, half - 1),
    ] {
        let column = terrain.column(x, z).unwrap();
        let top = column.iter().rposition(|&id| id != AIR).unwrap_or(0);
        assert!(
            top <= SEA as usize,
            "edge column ({x},{z}) rises to {top}, above sea"
        );
    }
}

#[test]
fn the_edge_mask_does_not_reach_inland() {
    let terrain = terrain();
    let inland = terrain.macro_height(0.0, 0.0);
    let masked = inland - (SEA as f32);
    let deep = terrain.macro_height(terrain.half_extent as f32 - 1.0, 0.0);
    assert!(deep < inland, "the boundary must sit below the centre");
    assert!(
        masked.is_finite() && deep.is_finite(),
        "the mask only moves base"
    );
}

#[test]
fn tiles_are_cached_and_stable() {
    let terrain = terrain();
    let first = terrain.tile(1, -2).unwrap();
    let second = terrain.tile(1, -2).unwrap();
    assert!(std::rc::Rc::ptr_eq(&first, &second));
}

/// Decodes a section into the `x | z << 5 | y << 10` block array.
fn decode(section: &Section) -> Box<[BlockId; crate::description::SECTION_VOLUME]> {
    let mut blocks: Box<[BlockId; crate::description::SECTION_VOLUME]> =
        vec![AIR; crate::description::SECTION_VOLUME]
            .into_boxed_slice()
            .try_into()
            .expect("the scratch array matches the section volume");
    section.write_blocks(&mut blocks);
    blocks
}

/// The pre-detail top of one world column, from the tile cache.
fn column_top(terrain: &Terrain, x: i32, z: i32) -> i32 {
    let tile_x = crate::lattice::floor_div(x, TILE_BLOCKS);
    let tile_z = crate::lattice::floor_div(z, TILE_BLOCKS);
    terrain
        .tile(tile_x, tile_z)
        .expect("the tile builds from the seed")
        .height
        .top(x - tile_x * TILE_BLOCKS, z - tile_z * TILE_BLOCKS)
}

/// Owns the column arrays an [`ApplyContext`] borrows.
struct Fixture {
    tops: Vec<i32>,
    slope: Vec<i32>,
    biome: Vec<u8>,
    flags: Vec<u8>,
    regions: [[i32; 3]; features::GATHER_REGIONS],
    ores: [features::OreBlock; 2],
    host: [BlockId; 1],
    scatter: [BlockId; 1],
    materials: TerrainMaterials,
}

impl Fixture {
    fn new(origin: [i32; 3], top: i32, biome: u8) -> Self {
        let materials = TerrainMaterials::from_pack(&content::v1_default())
            .expect("the built-in pack resolves");
        Self {
            tops: vec![top; features::COLUMNS],
            slope: vec![0; features::COLUMNS],
            biome: vec![biome; features::COLUMNS],
            flags: vec![0; features::COLUMNS],
            regions: features::gather_origins(features::region_of(origin)),
            ores: [
                features::OreBlock {
                    name: "ore_iron",
                    id: materials.ore_iron,
                },
                features::OreBlock {
                    name: "ore_copper",
                    id: materials.ore_copper,
                },
            ],
            host: [materials.stone],
            scatter: [materials.plant],
            materials,
        }
    }

    fn ctx(&self, seed: u64) -> ApplyContext<'_> {
        let origin = self.regions[features::region_slot([0, 0, 0])];
        ApplyContext {
            seed,
            palette: Palette {
                air: self.materials.air,
                water: self.materials.water,
                bedrock: self.materials.bedrock,
                log: self.materials.wood,
                leaf: self.materials.leaves,
                boulder: self.materials.boulder,
            },
            ores: &self.ores,
            host_rock: &self.host,
            columns: ColumnField {
                origin: [
                    origin[0] - features::REGION_SIZE,
                    origin[2] - features::REGION_SIZE,
                ],
                tops: &self.tops,
                slope: &self.slope,
                biome: &self.biome,
                flags: &self.flags,
            },
            regions: self.regions,
            scatter: &self.scatter,
        }
    }
}

#[test]
fn footprint_and_surface_share_the_column_pick() {
    let terrain = terrain();
    let footprint = terrain.footprint(0, 0).unwrap();
    assert_eq!(
        footprint.origin,
        [-features::REGION_SIZE, -features::REGION_SIZE]
    );
    let mut compared = 0;
    for local_z in 0..features::FOOTPRINT {
        for local_x in 0..features::FOOTPRINT {
            let x = footprint.origin[0] + local_x;
            let z = footprint.origin[1] + local_z;
            let slot = local_x + features::FOOTPRINT * local_z;
            let slot = usize::try_from(slot).expect("a footprint slot fits usize");
            assert_eq!(
                footprint.biome[slot],
                terrain.biome_at(x, z).unwrap(),
                "footprint differs from the column pick at ({x}, {z})"
            );
            compared += 1;
        }
    }
    assert_eq!(
        compared,
        usize::try_from(features::FOOTPRINT * features::FOOTPRINT).unwrap()
    );
}

#[test]
fn features_actually_generate() {
    const SEED: u64 = 0x5EED;

    fn counts() -> (usize, usize) {
        let terrain = Terrain::new(SEED).expect("the v1 description compiles");
        let materials = *terrain.materials();
        let mut logs = 0_usize;
        let mut ores = 0_usize;
        for section_z in 0..2 {
            for section_x in 0..2 {
                for section_y in 0..5 {
                    let section = terrain
                        .section(section_x, section_y, section_z)
                        .expect("the section generates");
                    let blocks = decode(&section);
                    for z in 0..32 {
                        for x in 0..32 {
                            let world_x = section_x * 32 + x;
                            let world_z = section_z * 32 + z;
                            let top = column_top(&terrain, world_x, world_z);
                            for y in 0..32 {
                                let index = crate::section::section_index(
                                    u32::try_from(x).unwrap_or(0),
                                    u32::try_from(y).unwrap_or(0),
                                    u32::try_from(z).unwrap_or(0),
                                );
                                let id = blocks[index];
                                if id == materials.ore_iron || id == materials.ore_copper {
                                    ores += 1;
                                }
                                if id == materials.wood && section_y * 32 + y > top {
                                    logs += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        (logs, ores)
    }

    let first = counts();
    let second = counts();
    assert!(
        first.0 > 0,
        "no tree log above the surface stack: {first:?}"
    );
    assert!(first.1 > 0, "no ore block was generated: {first:?}");
    assert_eq!(first, second, "the feature count changed between runs");
}

#[test]
fn features_are_thread_and_order_independent() {
    const SEED: u64 = 0x5EED;

    // `--threads` gives each worker its own `Terrain`; cache and processing
    // order differ, but the section must not. Warm the same section in
    // opposite orders from two instances and compare its content hash.
    let forward = Terrain::new(SEED).expect("the v1 description compiles");
    for section_y in 0..5 {
        let _ = forward
            .section(1, section_y, 0)
            .expect("the section generates");
    }
    let first = forward
        .section(1, 4, 0)
        .expect("the section generates")
        .content_hash();

    let reverse = Terrain::new(SEED).expect("the v1 description compiles");
    for section_y in (0..5).rev() {
        let _ = reverse
            .section(1, section_y, 0)
            .expect("the section generates");
    }
    let second = reverse
        .section(1, 4, 0)
        .expect("the section generates")
        .content_hash();

    assert_eq!(first, second, "cache order changed the section");
}

#[test]
fn canopy_is_not_truncated_at_a_section_top() {
    use crate::features::{Candidate, FeatureKind, tree};
    use crate::hash::hash64;

    const SEED: u64 = 0x5EED;
    let specs = features::v1_specs();
    let upper_origin = [0, 160, 0];
    let foot = 159;

    // A tree whose base is the last block of the lower section, so its trunk
    // and canopy cross into the all-air section above.
    let candidate = (0..8192_u64)
        .find_map(|id| {
            let variant = hash64(SEED, domain("tree:oak"), &[id.cast_signed()]);
            let candidate = Candidate {
                id,
                region: features::region_of([16, foot, 16]),
                kind: FeatureKind::Tree,
                origin: [16, foot, 16],
                variant,
            };
            (tree::allowed(tree::species_of(&candidate), 3) && tree::accepts(SEED, &candidate, 3))
                .then_some(candidate)
        })
        .expect("no accepted forest tree");

    let fixture = Fixture::new(upper_origin, foot - 1, 3);
    let ctx = fixture.ctx(SEED);
    let air = fixture.materials.air;
    let wood = fixture.materials.wood;
    let leaf = fixture.materials.leaves;

    // With nothing in reach the all-air section is still free (`H2.2`).
    let mut empty = Section::uniform(air);
    let report = features::apply_section_counted(&mut empty, &[], &ctx, &specs);
    assert_eq!(report.probes, 0, "an unreachable section was processed");

    // With this candidate the section must be processed, and its blocks above
    // the seam must be present.
    let mut canopy = Section::uniform(air);
    let report = features::apply_section_counted(&mut canopy, &[candidate], &ctx, &specs);
    assert!(report.probes > 0, "the all-air section was skipped");
    assert!(
        (1..32).any(|y| canopy.get(16, y, 16) == wood),
        "the trunk above the seam was truncated"
    );
    let mut leaves = 0;
    for y in 0..32 {
        for z in 0..32 {
            for x in 0..32 {
                leaves += u32::from(canopy.get(x, y, z) == leaf);
            }
        }
    }
    assert!(leaves > 0, "the canopy above the seam was truncated");
    assert!(report.stats.writes > 0, "the tree wrote nothing");
}

/// `§2.6`: every id the pipeline emits resolves from a stable name, and the
/// resolved value is exactly the pack's id.
#[test]
fn every_material_name_resolves_to_the_pack_id() {
    let pack = content::v1_default();
    let materials = TerrainMaterials::from_pack(&pack).expect("the built-in pack resolves");
    let cases = [
        ("core:air", materials.air),
        ("core:stone", materials.stone),
        ("core:dirt", materials.dirt),
        ("core:grass", materials.grass),
        ("core:sand", materials.sand),
        ("core:gravel", materials.gravel),
        ("core:clay", materials.clay),
        ("core:snow", materials.snow),
        ("core:ice", materials.ice),
        ("core:water", materials.water),
        ("core:bedrock", materials.bedrock),
        ("core:leaves", materials.leaves),
        ("core:wood", materials.wood),
        ("core:plant", materials.plant),
        ("core:lamp", materials.lamp),
        ("core:boulder", materials.boulder),
        ("core:ore_copper", materials.ore_copper),
        ("core:ore_iron", materials.ore_iron),
    ];
    for (name, id) in cases {
        assert_eq!(pack.id(name), Some(id), "{name}");
    }
}

/// A pack without `core:stone` refuses generation by name, not by number.
#[test]
fn a_missing_stone_record_is_manifest_incompatible() {
    let mut records = content::v1_default().records().to_vec();
    records.retain(|record| record.name != "core:stone");
    let pack = ContentPack::load(records).expect("a pack without stone is still valid");

    let error = TerrainMaterials::from_pack(&pack).expect_err("stone is required");
    match &error.kind {
        ErrorKind::ManifestIncompatible { id, .. } => assert_eq!(id, "core:stone"),
        other => panic!("expected ManifestIncompatible, found {other:?}"),
    }
}

/// Behavioural proof that no numeric literal survives: the generated
/// stone, bedrock, and sand are the pack's ids.
#[test]
fn sections_emit_the_pack_ids() {
    let pack = content::v1_default();
    let terrain = Terrain::new(7).expect("the v1 description compiles");
    let materials = *terrain.materials();
    assert_eq!(pack.id("core:stone"), Some(materials.stone));
    assert_eq!(pack.id("core:bedrock"), Some(materials.bedrock));
    assert_eq!(pack.id("core:sand"), Some(materials.sand));

    // Stone and bedrock appear in the lowest sections at the origin.
    let mut seen = BTreeSet::new();
    for section_y in 0..3 {
        let section = terrain
            .section(0, section_y, 0)
            .expect("the section generates");
        for y in 0..32 {
            for z in 0..32 {
                for x in 0..32 {
                    seen.insert(section.get(x, y, z));
                }
            }
        }
    }
    assert!(seen.contains(&materials.stone), "no stone at the origin");
    assert!(
        seen.contains(&materials.bedrock),
        "no bedrock at the origin"
    );

    // Find one sand column, then prove the section that holds it emits the
    // pack's sand id as well.
    let mut found = None;
    for x in (-1024..=1024).step_by(16) {
        let column = terrain.column(x, 0).expect("the column generates");
        if let Some(y) = column.iter().position(|&id| id == materials.sand) {
            found = Some((x, y as i32, 0));
            break;
        }
    }
    let (x, y, z) = found.expect("the v1 world has no sand at seed 7");
    let section = terrain
        .section(
            crate::lattice::floor_div(x, 32),
            crate::lattice::floor_div(y, 32),
            crate::lattice::floor_div(z, 32),
        )
        .expect("the sand section generates");
    let mut has_sand = false;
    for yy in 0..32 {
        for zz in 0..32 {
            for xx in 0..32 {
                has_sand |= section.get(xx, yy, zz) == materials.sand;
            }
        }
    }
    assert!(has_sand, "the section around a sand column has no sand");
}

/// The two id constants the `terrain-gen` caller imports agree with the
/// built-in pack. Generation itself reads [`TerrainMaterials`].
#[test]
fn the_tool_id_constants_match_the_pack() {
    let pack = content::v1_default();
    assert_eq!(pack.id("core:air"), Some(AIR), "core:air must sort first");
    assert_eq!(pack.id("core:water"), Some(WATER));
}

/// A pack supplied through `with_pack` is the same world as `new` when it is
/// the built-in pack.
#[test]
fn with_pack_matches_new_for_the_built_in_pack() {
    const SEED: u64 = 0x5EED;
    let built_in = Terrain::new(SEED).expect("the v1 description compiles");
    let explicit =
        Terrain::with_pack(SEED, &content::v1_default()).expect("the built-in pack resolves");

    for (x, z) in [(0, 0), (5, 7), (123, -45), (224, 16)] {
        assert_eq!(
            built_in.column(x, z).expect("the column generates"),
            explicit.column(x, z).expect("the column generates"),
            "({x},{z})"
        );
    }
    assert_eq!(
        built_in
            .section(1, 4, 0)
            .expect("the section generates")
            .content_hash(),
        explicit
            .section(1, 4, 0)
            .expect("the section generates")
            .content_hash(),
    );
}

/// The column memo is byte-identical to the pre-optimisation generator.
///
/// Values were captured from `Terrain::new(12345)` before the memo existed.
/// A memo hit must return exactly the column the direct generator builds,
/// and a warmed section must equal the same section from a fresh instance
/// with an empty memo. Absolute output is pinned by the golden fixture.
#[test]
fn memoized_columns_and_sections_match_direct_generation() {
    const SEED: u64 = 12_345;
    const COLUMNS: [(i32, i32); 5] = [(0, 0), (1, -1), (16, 16), (-33, 52), (123, -45)];

    let terrain = Terrain::new(SEED).expect("the v1 description compiles");
    // Warm the memo for every coordinate before any section is built, so the
    // section path below is served by the shared column.
    for &(x, z) in &COLUMNS {
        let _ = terrain.column(x, z).expect("the column generates");
    }

    for &(x, z) in &COLUMNS {
        let memoized = terrain.column(x, z).expect("the column generates");
        assert_eq!(
            memoized,
            terrain
                .generate_column(x, z)
                .expect("the direct column generates"),
            "column ({x}, {z}) must match the direct generator"
        );

        // A fresh instance starts with an empty memo, so its sections take
        // the compute path; the warmed one takes the shared-column path.
        let cold = Terrain::new(SEED).expect("the v1 description compiles");
        for sy in 0..12 {
            let warm = terrain
                .section(x.div_euclid(32), sy, z.div_euclid(32))
                .expect("the warmed section generates");
            let cold_section = cold
                .section(x.div_euclid(32), sy, z.div_euclid(32))
                .expect("the cold section generates");
            assert_eq!(
                warm.content_hash(),
                cold_section.content_hash(),
                "section ({x}, {sy}, {z}) must not depend on the memo"
            );
        }
    }
}

/// Absolute output pin: one fixed seed's column and section bytes.
///
/// The memo test compares warm and cold paths, not a fixed value. This
/// pins the bytes themselves, so a restructured pipeline that changes
/// generation fails here even when its warm and cold paths still agree.
#[test]
fn column_and_section_digests_are_pinned() {
    const SEED: u64 = 0x5EED;
    let terrain = Terrain::new(SEED).expect("the v1 description compiles");

    let column = terrain.column(123, -45).expect("the column generates");
    let mut bytes = Vec::with_capacity(column.len() * 2);
    for id in column {
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    let digest = crate::hash::sha256(&bytes);
    let mut rendered = String::new();
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut rendered, "{byte:02x}").expect("writing to a string cannot fail");
    }
    assert_eq!(
        rendered, "ce1b27cf714b23a3917562573723513b96ca93188278f1c9d738a46a0559e0d0",
        "column (123, -45) digest changed"
    );

    let section = terrain.section(1, 4, 0).expect("the section generates");
    assert_eq!(
        section.content_hash(),
        7_992_660_242_972_054_931,
        "section (1, 4, 0) content hash changed"
    );
}
