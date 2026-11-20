# Terrain v1 runtime

**Status:** current on `terrain/build-v1`, 2026-11-20. This page describes how the v1 terrain
pipeline is wired into the demo. The normative contract remains [Terrain spec](../specs/terrain.md);
the save bytes remain [Save format](save-format.md).

## Run it

```console
$env:TERRAIN_DEMO = "1"
$env:ENGINE_STRESS_SCENE = "1"   # only needed when world/seed is absent
cargo run --bin engine
```

The demo opens a tile-streamed 128x128 terrain patch on top of the legacy scene. Edits, saves, and
repairs use `world/terrain-demo/`; the legacy `world/chunks/` path is untouched.

| Input | Effect |
|---|---|
| Left click | Break the targeted block |
| Right click | Place the selected block |
| `R` / `Q` / `S` | Restore backup / quarantine / salvage a corrupt section when the repair prompt is shown |
| Left/middle/right click in the repair prompt | Same three repair actions |
| `Ctrl+S` | Flush the terrain save worker |

## Modules

| File | Responsibility |
|---|---|
| `src/description.rs`, `src/header.rs`, `src/section.rs` | World identity, header bytes, section records |
| `src/graph/`, `src/pipeline.rs`, `src/height.rs`, `src/surface.rs`, `src/strata.rs`, `src/climate.rs`, `src/volume.rs` | Generation from description to columns |
| `src/features/`, `src/hydro/` | Trees/ore/scatter and tile-mode hydrology |
| `src/light2.rs`, `src/padded.rs` | Column light, incremental repair, shared padded scratch |
| `src/mesher2.rs`, `src/lod.rs` | Greedy section meshing, coarse downsample, skirts |
| `src/section.rs`, `src/lod.rs`, `src/revisions.rs`, `src/residency.rs`, `src/editlog.rs` | Section storage, LOD derivation, revisions, residency policy, edit journal |
| `src/save.rs`, `src/world_save.rs` | Section files, backup rotation, journal file, `WorldSave`, save worker, repair, migration/report |
| `src/lib.rs` | Demo app, parallel column generation, light/mesh rebuild, LOD selection, tile streaming, save/repair wiring |
| `src/renderer.rs`, `src/terrain_draw.wgsl` | Packed-quad draw path, per-section scale, incremental upload |

## Startup

The demo builds one 4x4-column tile (128x128 blocks) aligned to a 128-block generation tile.

| Phase | Release |
|---|---:|
| `Terrain::new` | ~0 ms |
| Field tile | ~18 ms |
| Column generation (12 workers, one `Terrain` each) | **~0.82 s** |
| Column light | ~31 ms |
| Fine meshes | ~31 ms |
| Coarse LOD meshes | ~45 ms |
| **Total build** | **~0.9 s** |

Every worker owns its own `Terrain` because the pipeline caches behind `RefCell`. A field tile is
~18 ms, so duplicating it per worker is cheaper than serialising the per-column work. Output is
keyed and sorted, so worker order cannot change the patch.

## Light

- `demo_column_light` fans out one job per column.
- Edits use `ColumnView`, a borrowed column plus eight border columns, so no section or border is
  copied.
- `relight_span_view` repairs only the band an edit can reach; the edited column repairs from
  `y = 0` because a block moves the sky seed below it, neighbours only around the edit.
- `section_differs` drives the remesh set: every section whose light changed plus the sections
  touched by the edited block's 26-block neighbourhood.

## Meshing and LOD

- Fine sections are greedy-meshed with `MeshLod::Lod0`.
- `demo_coarse` downsamples every complete 2x2x2 fine group with `lod::downsample`, meshes with
  `mesh_coarse` at Lod1, and appends skirts on all sides.
- The active set replaces a complete coarse region with one coarse section per 2x2x2 group when the
  player is more than one column away; missing coarse neighbours are treated as air so boundary walls
  are emitted.
- `TerrainSection` carries `scale` (1/2/4); the shader scales quad origins, widths, heights, face
  offsets, cross quads, and UVs. Culling uses `32 * scale`.
- `section_revision` includes scale, so an origin changing detail level re-uploads.

## Tiles and streaming

- Active patch: one 4x4-column tile; saved edits are loaded for its columns.
- `plan_stream` is the pure decision: swap to a cached tile, build the tile underfoot if missing,
  pause physics when no ground tile is ready, prefetch with two columns of lead, and add the diagonal
  when both axes are near an edge.
- One background build at a time; active + three cached tiles; farthest evicted. Measured working
  set: **350 MB** with the demo vs **295 MB** for the legacy stress scene.
- Tile crossing swaps the cached patch, uploads the renderer sections, and keeps the old patch for an
  instant swap back.

## Persistence

`world/terrain-demo/`:

```text
header.bin                 v2 header with migration_state
description.toml           canonical generation parameters
sections/c.<cx>.<cz>.<sy>.sect[.bak|.tmp|.corrupt]
journal/0001.journal       append-only edit frames
migration.journal          only while a migration is in progress
migration-report-<unix>.json
```

- Every edit queues one journal batch and one section write on the `terrain-save` IO thread.
- `save_section` flushes the journal, writes the section (temp -> fsync -> verify -> rotate ->
  rename -> directory fsync), advances the header revision, then trims the covered journal prefix.
- A flush alone never advances the revision; a frame is covered only by a section save.
- Recovered frames newer than the header revision replay on open.
- `world-repair`: `terrain-gen repair --world <dir> --section <cx,cz,sy> --action
  restore|quarantine|salvage`; the demo also offers `R`/`Q`/`S` or click thirds.
- `terrain-gen migrate --world <dir> [--from <n>] [--to <n>]` canonicalises sections, writes a
  migration report, and upgrades v1 headers to v2.
- Repair, migration, and the repeated crash/save/reopen loop are covered by tests.

## Profiling

Debug builds enable the `tracy` default feature and start `tracy-capture`, writing
`reports/tracy/run-<unix>.tracy`. `TRACY_CAPTURE_SECONDS=0` keeps instrumentation without capture;
`TRACY_CAPTURE` overrides the tool path. The terrain path contributes `terrain_sections_uploaded`
(one per edit) plus the existing frame, mesh, upload, and GPU plots. See [Profiling](profiling.md).

## Gates

```console
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo run --locked --quiet --bin terrain-gen -- golden check
```

Current local count: **629 lib tests** (2 ignored), **3** `light_preview`, **24** `terrain_gen`
integration tests.

## Known gaps

- Texture references are still the renderer's name-mapped procedural layers; the content pack does
  not carry per-face texture identities.
- Residency-driven ring streaming (16-column view radius) is not wired; the demo pages tiles.
- No normative older section record exists, so migration fixtures cannot be added yet.
- Camera-relative rendering (spec 14.3) is not applied.
- World-mode hydrology (`hydro::WorldHydro`) is implemented but not wired.
- LOD2, skirts on every fine/coarse transition, and a visible loading indicator are not done.
