//! Compile-time guard for the public surface kept after visibility batches 1, 2, and 3.
//!
//! `terrain-core` is an internal crate: its public surface is what `engine` and
//! the `terrain-gen` binary need, and nothing else. This test imports exactly the
//! paths the binary imports, plus the batch-2 and batch-3 survivors the engine
//! names, so narrowing one of them by accident fails to compile instead of
//! narrowing silently. The `compile_fail` doctests in `terrain-core`'s crate docs
//! guard the opposite direction: a path the batches made crate-private must stay
//! crate-private.

use std::mem::size_of;
use std::path::PathBuf;

use terrain_core::climate::{Biome, BiomeTable, smoothstep01};
use terrain_core::content::{ContentPack, LIGHT_LEVELS, RenderShape, v1_default};
use terrain_core::description::{
    self, Axis, Description, EmbeddedRef, HYDRO_TILE, KEYS, MACRO_CELL, MAX_TERRAIN_Y, SEA,
    SECTION_SIZE, SECTIONS_PER_COLUMN, TILE_BLOCKS, WORLD_HEIGHT, default_v1, load,
};
use terrain_core::error::{ErrorKind, Stage, TerrainError};
use terrain_core::hash::{mix, sha256};
use terrain_core::header::{self, Header};
use terrain_core::hydro::{BORDER_CELLS, TILE_CELLS, WorldHydro};
use terrain_core::lattice::floor_div;
use terrain_core::lod::{Fine64, Octants};
use terrain_core::mesher2::{
    Material, MaterialProps, MeshLod, Quad, SectionMesh, SkirtEdges, add_skirts, mesh_coarse,
    mesh_from_neighbours, quad_count,
};
use terrain_core::padded::{LOCAL_MAX, LOCAL_MIN, NEIGHBOUR_ORDER, PaddedBlocks, PaddedLight};
use terrain_core::pipeline::{AIR, Terrain, WATER};
use terrain_core::residency::{
    DEMO_VIEW_RADIUS, RingBudget, RingPlan, UNLOAD_MARGIN, demo_view_radius, in_ring, plan_ring,
};
use terrain_core::save::{RepairAction, RepairOutcome, SectionStore};
use terrain_core::section::{self, BlockId, Section, SectionKey, decode_record_at};
use terrain_core::world_save::{MigrationReport, SaveWorker, WorldSave, write_migration_report};

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

    // Types are real, not aliases to zero-sized placeholders.
    assert!(size_of::<Description>() > 0);
    assert!(size_of::<Terrain>() > 0);
    assert!(size_of::<Section>() > 0);
}

#[test]
fn the_batch_2_survivors_stay_public() {
    // Signature-forced and engine-facing types kept when their module was
    // narrowed. `Option` forces the path to resolve without a value.
    let _: Option<WorldHydro> = None;
    let _: Option<ContentPack> = None;
    let _: Option<RenderShape> = None;
    let _: Option<Material> = None;
    let _: Option<MaterialProps> = None;
    let _: Option<MeshLod> = None;
    let _: Option<Quad> = None;
    let _: Option<SectionMesh> = None;
    let _: Option<SkirtEdges> = None;
    let _: Option<PaddedBlocks> = None;
    let _: Option<PaddedLight> = None;
    let _: Option<RingBudget> = None;
    let _: Option<RingPlan> = None;

    // Free functions and constants the engine names directly.
    let _ = LIGHT_LEVELS;
    let _ = LOCAL_MIN;
    let _ = LOCAL_MAX;
    let _ = NEIGHBOUR_ORDER;
    let _ = DEMO_VIEW_RADIUS;
    let _ = UNLOAD_MARGIN;
    let _ = SECTION_SIZE;
    let _ = WORLD_HEIGHT;
    let _ = demo_view_radius;
    let _ = in_ring;
    let _ = plan_ring;
    let _ = add_skirts;
    let _ = mesh_coarse;
    let _ = mesh_from_neighbours;
    let _ = quad_count;

    // Signatures that force a type public: the LOD octant aliases are the
    // parameter types of `lod::downsample{,4}`, and `MeshLod`/`SkirtEdges` are
    // parameters of the skirt and coarse-mesh entry points.
    let _: Option<Octants<'_>> = None;
    let _: Option<Fine64<'_>> = None;
}

#[test]
fn the_batch_3_survivors_stay_public() {
    // Persistence: batch 3 narrowed the journal writer family and the methods
    // that exposed it. These are the methods `engine` and `terrain-gen` call.
    let _: fn(PathBuf, u64) -> Result<WorldSave, TerrainError> = WorldSave::open;
    let _: fn(PathBuf, u64, &str) -> Result<WorldSave, TerrainError> = WorldSave::open_or_create;
    let _: fn(PathBuf, u64, &str) -> Result<WorldSave, TerrainError> = WorldSave::create;
    let _ = WorldSave::root;
    let _ = WorldSave::recovered;
    let _ = WorldSave::read_sections;
    let _ = WorldSave::migrate;
    let _ = SaveWorker::start;
    let _ = SaveWorker::record_batch;
    let _ = SaveWorker::save_section;
    let _ = SaveWorker::flush;
    let _ = SaveWorker::shutdown;

    // Section and section-store methods the engine names directly, plus the
    // test-only constructors that integration tests call (the tests are not
    // edited by a visibility batch, so those paths must stay).
    let _ = Section::get;
    let _ = Section::set;
    let _ = Section::words;
    let _ = Section::palette;
    let _ = Section::content_hash;
    let _ = Section::uniform;
    let _ = Section::from_blocks;
    let _ = Section::from_palette;
    let _ = Section::get_index;
    let _ = Section::is_uniform;
    let _: fn(PathBuf, ContentPack) -> SectionStore = SectionStore::new;
    let _: fn(PathBuf, ContentPack) -> Result<SectionStore, TerrainError> = SectionStore::create;
    let _ = SectionStore::path_for;
    let _ = SectionStore::write;
    let _ = SectionStore::read;
    let _ = SectionStore::key_from_path;
    let _ = SectionStore::repair;

    // Header, description, and content-pack accessors the tool and engine call.
    let _ = Header::read;
    let _ = Header::write;
    let _ = Description::description_hash;
    let _ = Description::save;
    let _ = ContentPack::load;
    let _ = ContentPack::id;
    let _ = ContentPack::records;
    let _ = ContentPack::len;
    let _ = ContentPack::name;

    // The generation facade after `with_pack`, `overlay`, and `macro_height`
    // became crate-private.
    let _ = Terrain::new;
    let _ = Terrain::column;
    let _ = Terrain::counters;
    let _ = Terrain::description_hash;
    let _ = Terrain::materials;
    let _ = Terrain::seed;
    let _ = Terrain::tile;
}
