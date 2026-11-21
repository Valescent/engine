//! Compile-time guard for the public surface kept after visibility batch 1.
//!
//! `terrain-core` is an internal crate: its public surface is what `engine` and
//! the `terrain-gen` binary need, and nothing else. This test imports exactly the
//! paths the binary imports, so narrowing one of them by accident fails to
//! compile instead of narrowing silently. The `compile_fail` doctest in
//! `terrain-core`'s crate docs guards the opposite direction: a path that batch 1
//! made crate-private must stay crate-private.

use std::mem::size_of;

use terrain_core::climate::{Biome, BiomeTable, smoothstep01};
use terrain_core::content::{ContentPack, v1_default};
use terrain_core::description::{
    self, Axis, Description, EmbeddedRef, HYDRO_TILE, KEYS, MACRO_CELL, MAX_TERRAIN_Y, SEA,
    SECTIONS_PER_COLUMN, TILE_BLOCKS, default_v1, load,
};
use terrain_core::error::{ErrorKind, Stage, TerrainError};
use terrain_core::hash::{mix, sha256};
use terrain_core::header::{self, Header};
use terrain_core::hydro::{BORDER_CELLS, TILE_CELLS};
use terrain_core::lattice::floor_div;
use terrain_core::pipeline::{AIR, Terrain, WATER};
use terrain_core::save::{RepairAction, RepairOutcome, SectionStore};
use terrain_core::section::{self, BlockId, Section, SectionKey, decode_record_at};
use terrain_core::world_save::{MigrationReport, WorldSave, write_migration_report};

#[test]
fn the_tool_imports_stay_public() {
    // Types the binary names. `Option` is only here to force the path to resolve
    // without constructing a value.
    let _: Option<Biome> = None;
    let _: Option<BiomeTable> = None;
    let _: Option<ContentPack> = None;
    let _: Option<Axis> = None;
    let _: Option<Description> = None;
    let _: Option<EmbeddedRef> = None;
    let _: Option<ErrorKind> = None;
    let _: Option<Stage> = None;
    let _: Option<TerrainError> = None;
    let _: Option<Header> = None;
    let _: Option<BlockId> = None;
    let _: Option<Section> = None;
    let _: Option<SectionKey> = None;
    let _: Option<RepairAction> = None;
    let _: Option<RepairOutcome> = None;
    let _: Option<SectionStore> = None;
    let _: Option<MigrationReport> = None;
    let _: Option<WorldSave> = None;
    let _: Option<Terrain> = None;

    // Functions the binary calls.
    let _ = smoothstep01;
    let _ = v1_default;
    let _ = default_v1;
    let _ = load;
    let _ = mix;
    let _ = sha256;
    let _ = floor_div;
    let _ = decode_record_at;
    let _ = write_migration_report;

    // Constants and module paths the binary names.
    let _ = HYDRO_TILE;
    let _ = KEYS;
    let _ = MACRO_CELL;
    let _ = MAX_TERRAIN_Y;
    let _ = SEA;
    let _ = SECTIONS_PER_COLUMN;
    let _ = TILE_BLOCKS;
    let _ = BORDER_CELLS;
    let _ = TILE_CELLS;
    let _ = AIR;
    let _ = WATER;
    let _ = header::HASH_ALGORITHM;
    let _ = description::MACRO_CELL;
    let _ = section::decode_record_at;

    // The types are real, not aliases to zero-sized placeholders.
    assert!(size_of::<Description>() > 0);
    assert!(size_of::<Terrain>() > 0);
    assert!(size_of::<Section>() > 0);
}
