# terrain-core visibility audit

**Status:** Batch 1 applied on `feature/visibility-narrow`. It narrowed 325 of the
500 candidates across the 18 planned modules; the reachable public surface is now
**564 items**, down from 889.

| Batch | Items narrowed | Reachable public items | State |
|---|---:|---:|---|
| Audit baseline | 0 | 889 | merged (`a6720f0`) |
| Batch 1 | 325 | 564 | applied |
| Batches 2-3 | 175 | 389 | remaining |

Two items that §5 lists in batch 1 stayed public because a kept public signature
names them: `curve::Curve` (reachable through `pipeline::Curves`) and
`revisions::RequestKey` (reachable through `residency::ring`). Moving either would
require narrowing the public owner in the same change, which is batch 2 work. In
exchange, `features::scatter::{RenderShape::occludes, RenderShape::blocks_skylight}`
narrowed with their type, so the batch still totals 325 and the recount still lands
on 564.

**Scope below:** the audit and the batch plan as merged. It implements decision 3
of [Modular structure](modular-structure.md#8-decisions-2026-11-20): `terrain-core`
is an internal crate, so its public surface should be what `engine` and
`terrain-gen` need, and nothing else.

**Non-goals:** No narrowing in this change. No edit to `engine`'s facade in this
change. No change to the `terrain-gen` command surface, save formats, or golden
outputs.

## 1. Method

### 1.1 Counting

The count comes from rustdoc's own view of the crate, not from `grep` over `pub`
tokens. A JSON build of the library was produced with:

```console
$env:RUSTC_BOOTSTRAP = "1"
cargo rustdoc -p terrain-core --lib -- -Z unstable-options --output-format json
```

On a POSIX shell, prefix the `cargo rustdoc` line with `RUSTC_BOOTSTRAP=1` instead.

The root module was traversed through public modules and public `pub use`
re-exports. The item kinds counted are free functions, structs, enums, constants,
type aliases, traits, associated constants, and inherent methods. The following
are deliberately **not** counted:

- struct fields and enum variants, which are not independently nameable items;
- methods in trait impls (`Clone::clone`, `Display::fmt`, and so on), which cannot
  be narrowed without removing the impl;
- items in private submodules that no public re-export reaches.

At this tip the reachable surface is **889 items**: 137 free functions, 404
inherent methods, 186 constants, 100 structs, 40 enums, 16 associated constants,
5 type aliases, and 1 trait. The owner's earlier figure of roughly 892 was taken
before the module splits; the splits moved helpers into private submodules and did
not remove the public re-exports, so the recount lands within a few items of it.

### 1.2 Consumer classes

Each item was assigned to exactly one direct class by searching the consumer
corpora with path-aware matching (qualified paths, `use` trees with nested braces,
aliases, and glob imports):

| Class | Corpus | Evidence rule |
|---|---|---|
| engine | `src/**/*.rs` outside the `pub use terrain_core::…` and `pub mod terrain` re-export blocks | a qualified path through a terrain-core module, or an import that resolves to the item |
| tool | `crates/terrain-core/src/bin/terrain-gen/**/*.rs` | same |
| tests-only | `crates/terrain-core/tests/**/*.rs` and root `tests/**/*.rs` | same |
| unused | none of the above | no textual use |

Comments were stripped before searching. `#[cfg(test)] mod tests` blocks inside
`crates/terrain-core/src/**` were removed from the external corpus: those tests
compile against the crate itself and may use `pub(crate)`. Engine's own inline
tests were **not** removed, because `pub(crate)` in `terrain-core` is invisible to
`engine` even in a test build.

Textual use alone undercounts. Two patterns needed a second pass:

1. **Inferred receiver types.** `terrain-gen` obtains a `HydroTileRef` from
   `Terrain::hydro_tile` and then calls `tile.river_width(…)` and
   `tile.lake_level(…)` without naming the type. The method search cannot see
   that from the method name alone.
2. **Public signatures.** `ContentPack::props` returns `&BlockRecord`;
   `WorldSave::journal_mut` returns `&mut JournalWriter<File>`; `Header` has a
   `pub migration_state: MigrationState`. An external caller can use those types
   and their methods without ever writing the type name.

The second pass is a reachability closure over the rustdoc JSON. Starting from
every item with direct engine or tool use, it follows `resolved_path` edges, trait
bounds, struct field types, and enum variant payloads; when a type becomes
reachable, all of its inherent methods become reachable too. This is the
`signature-forced` column in the tables. It is the column that must be kept even
when no caller mentions the name today.

`tests-only` means a direct test reference that is **not** already forced by a
public signature. A test that names an item still forces the item public if the
test is retained; those items are called out separately in §4.4.

### 1.3 Limits

- **Text is not a type checker.** A name can be shadowed, and a method call can be
  resolved through a local type. The signature closure removes the most important
  false negatives, but the first batch is still verified with the compiler recipe
  in §7 before it is committed.
- **Re-exports have two visibility decisions.** Most items are declared `pub` in a
  private submodule and re-exported by `pub use` in the module root. Narrowing
  only the definition leaves a public re-export of a private item; narrowing only
  the re-export leaves an `unreachable_pub` warning. Both lines must change in the
  same edit.
- **Doc links are not consumers.** A doc link such as `[Type]` does not require
  the type to be public, but it does produce a rustdoc warning once the target is
  private. Those warnings are handled in §8, not by keeping the item.
- **Trait impls and orphan rules are not modelled by the count.** A public trait
  bounds a public generic type more tightly than a free function does. The one
  such case here, `editlog::JournalSink`, is kept public because
  `JournalWriter<S: JournalSink>` stays public.
- **Macros and generics.** No exported macros exist. Generic instantiations are
  counted once at the definition; a public type parameter's bounds are part of
  the signature closure.
- **Feature combinations.** The audit examined the default workspace build.
  `rayon-pool` and `tracy` do not add public items to `terrain-core`.

## 2. Result

| Measure | Count |
|---|---:|
| Reachable public items | 889 |
| Used directly by `engine` | 129 |
| Used directly by `terrain-gen` | 43 |
| Referenced only by integration tests | 8 |
| Forced by a kept public signature | 209 |
| **Safe to narrow to `pub(crate)`** | **500** |
| **Must stay public** | **389** |

The 389 that stay public are 172 direct external uses plus 209 signature-forced
items plus 8 strict tests-only items. The 500 narrow candidates are 56% of the
surface. A further 23 items are named by integration tests; 15 of those are
already in the 209 signature-forced items, and 8 are the strict tests-only items
above.

The important shape is that most of the surface is not used at all outside the
crate. The pipeline, hydrology, surface, volume, feature, graph, climate, noise,
metrics, strata, and revision internals are all consumed by `pipeline` or
`world_save` inside `terrain-core`; they never cross the crate boundary.

## 3. Per-module counts

`Signature` means reachable from a kept public signature, not directly named by a
consumer. `Narrow` is the candidate set. Each row sums to the module total.

| Module | `pub` items | Engine | Tool | Tests-only | Signature | Narrow |
|---|---:|---:|---:|---:|---:|---:|
| `climate` | 48 | 0 | 5 | 1 | 9 | 33 |
| `content` | 35 | 11 | 0 | 0 | 15 | 9 |
| `curve` | 6 | 0 | 0 | 0 | 0 | 6 |
| `description` | 39 | 4 | 12 | 1 | 5 | 17 |
| `editlog` | 36 | 7 | 0 | 0 | 22 | 7 |
| `error` | 15 | 4 | 0 | 0 | 11 | 0 |
| `features` | 42 | 0 | 0 | 0 | 0 | 42 |
| `features::ore` | 2 | 0 | 0 | 0 | 0 | 2 |
| `features::scatter` | 11 | 0 | 0 | 0 | 2 | 9 |
| `features::tree` | 19 | 0 | 0 | 0 | 0 | 19 |
| `graph` | 18 | 0 | 0 | 0 | 0 | 18 |
| `graph::node` | 22 | 0 | 0 | 0 | 0 | 22 |
| `hash` | 7 | 0 | 2 | 1 | 0 | 4 |
| `header` | 21 | 0 | 6 | 2 | 7 | 6 |
| `height` | 18 | 0 | 0 | 0 | 6 | 12 |
| `hydro::flood` | 3 | 0 | 0 | 0 | 0 | 3 |
| `hydro::flow` | 17 | 0 | 0 | 0 | 0 | 17 |
| `hydro::grid` | 22 | 0 | 2 | 0 | 0 | 20 |
| `hydro::lake` | 4 | 0 | 0 | 0 | 1 | 3 |
| `hydro::world` | 27 | 2 | 0 | 0 | 19 | 6 |
| `lattice` | 5 | 0 | 1 | 0 | 0 | 4 |
| `light2` | 76 | 29 | 0 | 1 | 23 | 23 |
| `lod` | 5 | 2 | 0 | 0 | 2 | 1 |
| `mesher2` | 62 | 20 | 0 | 0 | 28 | 14 |
| `metrics2` | 39 | 0 | 0 | 0 | 7 | 32 |
| `noise` | 13 | 0 | 0 | 0 | 0 | 13 |
| `padded` | 22 | 11 | 0 | 0 | 4 | 7 |
| `pipeline` | 29 | 6 | 6 | 0 | 13 | 4 |
| `residency` | 45 | 7 | 0 | 0 | 1 | 37 |
| `revisions` | 18 | 0 | 0 | 0 | 0 | 18 |
| `save` | 30 | 1 | 4 | 0 | 14 | 11 |
| `section` | 29 | 12 | 1 | 2 | 10 | 4 |
| `strata` | 8 | 0 | 0 | 0 | 0 | 8 |
| `surface` | 32 | 0 | 0 | 0 | 0 | 32 |
| `volume` | 35 | 0 | 0 | 0 | 0 | 35 |
| `world_save` | 29 | 13 | 4 | 0 | 10 | 2 |
| **Total** | **889** | **129** | **43** | **8** | **209** | **500** |

Two rows deserve a note because they look emptier than the code suggests:

- `error` has no narrow candidates. Every error type is either named directly by
  `engine`/`terrain-gen` or is a field or return type of `TerrainError` or
  `ErrorKind` (`Severity`, `Disposition`, `Frame`, `Coord`, `SectionFault`).
- `pipeline` has only four narrow items because `Terrain` is the facade type. Its
  public methods, and the types they return, are forced by the signature closure.

## 4. What must stay public and why

### 4.1 The `engine::terrain` facade

`src/lib.rs` exposes every terrain-core module under `pub mod terrain`:

```rust
pub mod terrain {
    pub use terrain_core::{
        climate, content, curve, description, editlog, error, features, graph, hash,
        header, height, hydro, lattice, light2, lod, mesher2, metrics2, noise, padded,
        pipeline, residency, revisions, save, section, strata, surface, volume, world_save,
    };
}
```

The module declarations themselves stay `pub`. This audit narrows items inside
the modules, not the facade. The eventual curated facade in `engine` can remove
module re-exports once every module has been narrowed; that is a separate change
and is not part of this plan.

The facade doctest in `src/lib.rs` names only `pipeline::Terrain` and
`error::TerrainError`, both of which stay public.

### 4.2 The `terrain-gen` imports

The binary imports these public paths today. Every named item must stay public:

```text
terrain_core::climate::{Biome, BiomeTable, smoothstep01}
terrain_core::content::{v1_default, ContentPack}
terrain_core::description::{self, Axis, Description, EmbeddedRef, HYDRO_TILE,
    KEYS, MACRO_CELL, MAX_TERRAIN_Y, SEA, SECTIONS_PER_COLUMN, TILE_BLOCKS,
    default_v1, load}
terrain_core::error::{ErrorKind, Stage, TerrainError}
terrain_core::hash::{mix, sha256}
terrain_core::header::{self, Header}
terrain_core::hydro::{BORDER_CELLS, TILE_CELLS}
terrain_core::lattice::floor_div
terrain_core::pipeline::{AIR, Terrain, WATER}
terrain_core::save::{RepairAction, RepairOutcome, SectionStore}
terrain_core::section::{self, BlockId, Section, SectionKey, decode_record_at}
terrain_core::world_save::{MigrationReport, WorldSave, write_migration_report}
```

The tool also calls these methods, which is why the owning types cannot be
narrowed: `Terrain::{new, column, counters, description_hash, hydro_tile}`,
`SectionStore::{new, repair}`, `WorldSave::{migrate, root}`,
`Header::{read, write}`, `Description::{description_hash, save}`,
`section::decode_record_at`, and `HydroTileRef::{river_width, lake_level}`.

### 4.3 Facade methods and their return types

These types are used directly by `engine`, so their whole public method surface is
kept in this pass. Narrowing a rarely called method here would force a second
audit of the types it returns.

| Type | Methods kept | Reason |
|---|---:|---|
| `pipeline::Terrain` | 15 | the supported generation entry point |
| `section::Section` | 17 | section reads and writes used by the engine |
| `save::SectionStore` | 16 | repair and section storage used by the tool |
| `world_save::WorldSave` | 17 | world open, save, and migration |
| `world_save::SaveWorker` | 5 | background saves |
| `error::TerrainError` | 5 | error type crossing the boundary |

Returned types (`pipeline::{TileData, HydroTileRef}`, `content::BlockRecord`,
`metrics2::Counters`, `light2::ColumnLight`, `mesher2::SectionMesh`, and their
nested field types) are forced by the closure. The first batch therefore leaves
them, and the methods that return them, alone.

### 4.4 Integration tests

The strict tests-only items are:

| Item | Test |
|---|---|
| `climate::DITHER_DOMAIN` | `crates/terrain-core/tests/blend.rs` |
| `description::SECTION_VOLUME` | `tests/terrain_gen/{record,repair,support}.rs` |
| `hash::domain` | `crates/terrain-core/tests/blend.rs` |
| `header::{HASH_ALGORITHM, crc32}` | `tests/terrain_gen/support.rs` |
| `light2::default_table` | `tests/facade.rs`, `tests/light_preview.rs` |
| `section::{decode_record, encode_record}` | `tests/terrain_gen/{record,support}.rs` |

A further set of methods is forced public by the signature closure but is only
*called* by tests: `Section::{from_blocks, get_index, is_uniform, from_palette}`,
`SectionStore::{create, path_for, write, read}`,
`Terrain::{materials, seed, biome_at}`, `WorldSave::create`, and
`ContentPack::load`. Their owners stay public, so they stay public too; the tests
are the reason they cannot be narrowed without a test change.

## 5. First batch: 325 items

Ordering is by risk, lowest first: leaf modules with no public type graph, then
self-contained data types, then the deeper generation stages. Inherent impls
follow the types they belong to. Traits and the remaining generic surfaces are
left for the last batch.

Apply each module as one commit. In each file, change the definition **and** the
module-root `pub use` re-export together; leave the `pub mod` declaration alone.

### `noise` (13)

Free items: `NEUTRAL`, `MEASURED_MAX_2D`, `MEASURED_MAX_3D`, `NOISE_BOUND_2D`,
`NOISE_BOUND_3D`, `unit`, `simplex2_2d`, `simplex2_3d`, `fbm_2d`, `ridged_2d`,
`billow_2d`, `warp_2d`, `warp_halo_cells`.

### `lattice` (4)

Free items: `rem_euclid`, `macro_cell`, `sample_pos`, `bilinear`. Keep `floor_div`
for the tool.

### `hydro::flood` (3)

Free items: `EPS`, `priority_flood`, `priority_flood_from`.

### `hydro::flow` (17)

Free items: `D8`, `ORDER`, `SQRT_2`, `receivers`, `accumulate`, `RAMP_CELLS`,
`MAX_WIDTH`, `W_SCALE`, `river_width`, `smoothstep`, `bed_depth`,
`river_influence`, `clamp_bed`, `river_level`, `monotone_levels`.
Inherent items: `D8::{dist, offset}`.

### `curve` (6)

Free items: `Curve`.
Inherent items: `Curve::{is_linear, linear, new, points, sample}`.

### `revisions` (18)

Free items: `SECTION_COUNT`, `FACE_COUNT`, `Face`, `ColumnRevisions`,
`SectionKey`, `RequestKey`.
Inherent items: `ColumnRevisions::{border_rev, bump_border, bump_section,
bump_sections, column_rev, is_dirty_since, new, section_key, section_rev,
world_revision}`, `Face::ALL`, `RequestKey::accepts`.

### `metrics2` (32)

Free items: `Unit`, `MetricSpec`, `METRICS`, `record_latency`, `set_gauges`,
`MemoryClass`, `MemoryLedger`, `ShedStep`, `RADIUS_CUT`, `MIN_VIEW_RADIUS`,
`RESTORE_WINDOWS`, `ShedState`, `shed`, `restore`, `overlay_lines`.
Inherent items: `MemoryClass::{ALL, name}`, `MemoryLedger::{cap, class_bytes,
largest_classes, new, pressure, release, reserve, used}`, `ShedState::{allows,
count_restore_window, effective_radius, new, restore_after, set_effective_radius,
step}`. Keep `Counters` public: `Terrain::counters` returns it.

### `strata` (8)

Free items: `Strata`, `ColumnStrata`.
Inherent items: `ColumnStrata::offset`, `Strata::{column, column_offset, new,
stone_at, stone_in}`.

### `height` (12)

Free items: `SIDE`, `CELLS`, `MACRO_SIDE`, `MACRO_CELLS`, `index`, `top_solid_y`,
`BlockGrid`, `RiverCarve`, `build`.
Inherent items: `BlockGrid::{get, new}`, `RiverCarve::new`.

### `features::ore` (2)

Free items: `ORE_MAX_EXTENT`, `blocks`.

### `features::scatter` (9)

Free items: `PATCH_CELL`, `SCALE`, `DENSITY`, `RenderShape`, `ScatterBlock`,
`V1_SCATTER`, `density`, `patch`, `present`.

### `features::tree` (19)

Free items: `TREE_CELL`, `MIN_DIST`, `TREE_MAX_DROP`, `TREE_WATER_MARGIN`,
`LEAF_MAX_DIST`, `TREE_MAX_EXTENT`, `CanopyShape`, `Part`, `Layer`, `Placement`,
`DENSITY`, `density`, `species_of`, `species_name`, `allowed`, `accepts`,
`priority`, `beats`, `place`.

### `features` (42)

Free items: `ApplyContext`, `ApplyReport`, `ApplyStats`, `WRITE_ORDER`,
`WRITE_PASSES`, `apply`, `apply_counted`, `apply_section`,
`apply_section_counted`, `stage_for`, `Candidate`, `FeatureKind`, `FeatureSpec`,
`GATHER_RADIUS`, `GATHER_REGIONS`, `REGION_SIZE`, `candidates_reach`, `discover`,
`gather`, `gather_origins`, `region_contains`, `region_of`, `region_origin`,
`region_slot`, `uniform_writable`, `v1_specs`, `validate_masks`, `validate_spec`,
`COLUMN_BANK`, `COLUMN_RIVER`, `COLUMNS`, `ColumnField`, `FOOTPRINT`, `Mask`,
`OreBlock`, `Palette`.
Inherent items: `ApplyContext::section_origin`, `ColumnField::{biome, flags,
slope, top}`, `FeatureSpec::masks`.

### `graph::node` (22)

Free items: `Lattice`, `Node`, `NodeSpec`, `Params`.
Inherent items: `NodeSpec::{add, arity, billow, clamp, constant, curve, fbm,
floats, fractal, input, lerp, mul, noise2d, params_match, remap, ridged, terrace,
warp}`.

### `graph` (18)

Free items: `Compiled`, `MAX_NODES`, `MAX_DEPTH`, `Graph`.
Inherent items: `Compiled::{eval_2d, order, slot_count, warp_halo_cells}`,
`Graph::{canonical_bytes, compile, hash, is_empty, len, new, node, output, push,
set_output}`.

### `climate` (33)

Free items: `CELL_COUNT`, `Climate`, `HUMIDITY_DOMAIN`, `MARITIME`, `MAX_CELL`,
`MIN_CELL`, `MacroCells`, `PADDED_CELLS`, `SHADOW_NORM`, `SHADOW_RANGE_CELLS`,
`SHADOW_STRENGTH`, `SIDE`, `SLOPE_K`, `Shadow`, `TEMP_DOMAIN`, `TEMP_NOISE_AMP`,
`WIND_DIR`, `index`, `padded_index`, `shadow`, `ClimateParams`, `altitude`,
`blend_ramp_blocks`, `latitude`, `H_SCALE`, `T_SCALE`.
Inherent items: `Climate::{new, solve, solve_at}`, `ClimateParams::{v1_default,
validate}`, `MacroCells::{get, new}`. Keep `Biome`, `BiomeTable`, and
`smoothstep01` for the tool.

### `surface` (32)

Free items: `BiomeRules`, `ColumnFacts`, `DITHER_SHARE`, `Layer`, `MAX_BIOMES`,
`Order`, `Predicate`, `Rule`, `SLOTS`, `STONE_DROP`, `SurfaceStack`,
`SurfaceTable`, `T_SNOW`, `snow_jitter`, `SEABED_CLAY_SHARE`,
`SEABED_DITHER_SHARE`, `SEABED_SHELF_LAST_DEPTH`, `SEABED_SHORE_LAST_DEPTH`,
`SEABED_SLOPE_LAST_DEPTH`, `SeabedMaterials`, `T_COLD_SHELF`, `T_COLD_SHORE`.
Inherent items: `SurfaceStack::{block_at, depth_blocks, layer_at}`,
`SurfaceTable::{fired_slot, new, resolve, resolve_column, seed, validate_table,
with_seabed}`.

### `volume` (35)

Free items: `BANK_CLEARANCE`, `CAVE_TAP_BUDGET`, `CaveSample`, `MOUTH_SLOPE_MIN`,
`SpanSkip`, `cell_skipped`, `lattice_cell`, `skippable_band`,
`DETAIL_TAP_BUDGET`, `DetailSample`, `FLOAT_MIN`, `band_fade`, `Span`, `SpanKind`,
`SpanRule`, `open_to_sky`, `resolve_spans`, `span_rule`, `COLUMN_HEIGHT`,
`LATTICE_CELLS`, `VolumeParams`, `ColumnInput`, `VolumeStats`, `Volume`.
Inherent items: `Volume::{apply, cave_density, detail_sample, entrance, is_mouth,
mouth_open, new, params, roof}`, `VolumeParams::{v1_default, validate}`.

After this batch the recount should be **564** reachable public items
(889 - 325).

## 6. Later batches (175 items)

Batch 2 covers the mixed modules. These contain at least one item the rest of the
crate needs, so they need per-item care:

| Module | Narrow | Examples |
|---|---:|---|
| `residency` | 37 | `Budget`, `Class`, `Membership`, `Admit`, `Applied`, `ClassCaps`, `Request` |
| `light2` | 23 | `LightSlot`, `LightStats`, `NibbleArray`, `QUEUE_CAPACITY` |
| `hydro::grid` | 20 | `HydroTile` and its constants; keep `TILE_CELLS` and `BORDER_CELLS` |
| `description` | 17 | `parse`, `from_toml_str`, `WorldHeader`, layout constants |
| `mesher2` | 14 | `mesh`, `mesh_from_face_neighbours`, normal constants, `Side::ALL` |
| `save` | 11 | `OpenedJournal`, `MigrationRecord`, journal constants |
| `content` | 9 | `record_hash`, `render_hash`, `KIND_BLOCK` and record constants |
| `editlog` | 7 | `crc32`, `decode_batches`, `encode_batch`, replay helpers |
| `padded` | 7 | `PAD`, `PADDED_WIDTH`, `PADDED_VOLUME`, `LOCAL_SIZE`, `index` |
| `header` | 6 | `LoadStep`, `load_order`, `refuse_reason`, header constants |
| `hydro::world` | 6 | `OwnedWorldTile` and its accessors; `WorldTileView` is forced by `WorldHydro::tile_view` |
| `pipeline` | 4 | `Curves` and its three methods; `TerrainMaterials` and `HydroTileRef` are forced by public methods |
| `hash` | 4 | `hash64`, `hash2`, `hash3`, `hash01` |
| `section` | 4 | `SECTION_RECORD_MAGIC`, `SECTION_RECORD_VERSION`, `section_coord`, `section_index` |
| `hydro::lake` | 3 | `find_lakes`, lake constants |
| `world_save` | 2 | `MIGRATION_REPORT_PREFIX`, `DESCRIPTION_FILE` |
| `lod` | 1 | `SKIRT_DEPTH` |

Batch 3 is the method-level cleanup that this audit deliberately did not take:

- `WorldSave::journal_mut` and `WorldSave::{begin_migration, migrate_sections,
  resume_migration, finish_migration, migrate_with_report, advance_revision,
  flush}`. Narrowing `journal_mut` would remove `JournalWriter` and
  `JournalSink` from the public surface as a unit.
- `Terrain::{with_pack, overlay, macro_height}`. These are public today but no
  external caller uses them; narrowing them may also release `Curves` and
  `TerrainMaterials`.
- `Description` helpers not used by the tool, `Section` helpers that are only
  used internally or by unit tests, and `ContentPack` convenience methods.

Traits and generic bounds come last: `JournalSink` is public today only because
`JournalWriter<S: JournalSink>` is public. Narrowing the writer and the trait in
the same commit is the only way to avoid a `private_bounds` warning. There are no
external implementations of the trait in this workspace, so no orphan-rule
constraint blocks the change.

## 7. Verification recipe

Run from the workspace root. The order is cheap checks first, then the full build.

```console
cargo fmt --all -- --check
typos
mdbook build
cargo check -p terrain-core --all-targets
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets
cargo doc -p terrain-core --no-deps
cargo test --workspace
```

`cargo check --workspace --all-targets` is the decisive gate: it compiles the
library, the `terrain-gen` binary, every integration test, and the engine. A
missing `pub` on a method reached through an inferred receiver type fails here.
`cargo clippy` catches the `unreachable_pub` warnings that mean a re-export and
its definition were narrowed in different commits.

Recount the reachable surface after each batch:

```console
$env:RUSTC_BOOTSTRAP = "1"
cargo rustdoc -p terrain-core --lib -- -Z unstable-options --output-format json
python -c "import json; d=json.load(open('target/doc/terrain_core.json', encoding='utf-8')); print(sum(1 for i in d['index'].values() if i.get('crate_id')==0 and i.get('visibility')=='public' and next(iter(i['inner'])) in {'function','struct','enum','constant','type_alias','trait','assoc_const'}))"
```

Expected counts: 889 now, 564 after the first batch, 389 after all 500 narrow
candidates. Treat a larger number as a missed narrowing and a smaller number as a
removed item that the closure thought was reachable.

Once the first batch is merged, tighten the doc gate:

```console
RUSTDOCFLAGS="-D warnings" cargo doc -p terrain-core --no-deps
```

This must not be enabled before the existing warnings in §8 are fixed.

## 8. Risks

1. **Re-export and definition drift.** The largest mechanical risk is changing one
   of the two visibility sites. `unreachable_pub` catches a `pub` item whose
   `pub use` was narrowed; a public re-export of a `pub(crate)` item is a hard
   error. Apply each module in one commit and grep the module root for `pub use`
   before committing.
2. **Rustdoc links to private items.** `cargo doc -p terrain-core` already emits
   11 warnings at this tip: unresolved links to `super::RIVER_MIN_CELLS`,
   `Stage::Hydrology`, `ErrorKind::InvariantViolated`, and `JournalWriter` /
   `JournalWriter::flush`, plus public docs that link to the private submodules
   `pipeline::{terrain, stages, hydro}` and `Terrain::footprint`. Narrowing adds
   more links of the same kind. Fix them by changing the link text to code spans
   or by linking to a public replacement; do not keep an item public only to
   satisfy a doc link.
3. **Private types in public signatures.** The signature closure is what prevents
   this, but it depends on rustdoc recording the edge. Pay special attention to
   generic bounds (`JournalSink`), associated constants typed with a narrowed
   type, and `impl Trait` return types. Compile the tool and the engine after
   every module.
4. **Traits and impls.** `JournalSink` is implemented for `File` and `Vec<u8>`
   inside the crate. Narrowing it is source-compatible for this workspace but
   would break any downstream implementation; `publish = false` makes that a
   non-issue. Trait-impl methods such as `Clone::clone` and `Display::fmt` are not
   narrowable and are excluded from the 889 count.
5. **Tests that import internals.** The integration tests named in §4.4 are the
   only external test consumers. They must either keep their imports or move to a
   public equivalent in the same change. The terrain-core inline `#[cfg(test)]`
   modules are not a constraint: they can use `pub(crate)` and were excluded from
   the external corpus.
6. **Inferred receivers.** `HydroTileRef::{river_width, lake_level}` is the case
   that proves text search is insufficient. Any future method-level audit must
   start from the return types of public methods, not from method names in the
   caller.
7. **Facade expectations.** `engine::terrain` currently documents all 28 modules
   as public. After narrowing, many modules will be public but empty. That is
   valid Rust and keeps `engine` compiling, but the facade documentation and
   `docs/dev/public-api.md` should be updated when `engine` replaces the wildcard
   re-exports with the curated set.

## References

- The Rust Reference, "Visibility and privacy".
  <https://doc.rust-lang.org/reference/visibility-and-privacy.html>
- The Rust Reference, "Trait implementation coherence" (orphan rules).
  <https://doc.rust-lang.org/reference/items/implementations.html#trait-implementation-coherence>
- The Cargo Book, "Workspaces".
  <https://doc.rust-lang.org/cargo/reference/workspaces.html>
- D. Drysdale, *Effective Rust*, Item 22 "Minimize visibility".
  <https://www.lurklurk.org/effective-rust/visibility.html>
- `rustdoc` JSON output.
  <https://doc.rust-lang.org/rustdoc/unstable-features.html#json-output>
