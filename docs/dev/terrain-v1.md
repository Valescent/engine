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

The demo opens a column-level streaming ring on top of the legacy scene. Every column within the
spec's 16-column view radius is resident (1,089 columns), fine-meshed inside two columns, drawn
as LOD1 through the middle band, and drawn as LOD2 in the far band. Edits, saves, and repairs use
`world/terrain-demo/`; the legacy `world/chunks/` path is untouched.

| Input | Effect |
|---|---|
| Left click | Break the targeted block |
| Right click | Place the selected block |
| `R` / `Q` / `S` | Restore backup / quarantine / salvage a corrupt section when the repair prompt is shown |
| Left/middle/right click in the repair prompt | Same three repair actions |
| `Ctrl+S` | Flush the terrain save worker |

## Modules

The headless terrain stack lives in the `terrain-core` crate; `engine` re-exports it so the
demo can keep using `engine::pipeline`, `engine::section`, and the other module paths.

| File | Responsibility |
|---|---|
| `crates/terrain-core/src/description.rs`, `header.rs`, `section.rs` | World identity, header bytes, section records |
| `crates/terrain-core/src/graph/`, `pipeline.rs`, `height.rs`, `surface.rs`, `strata.rs`, `climate.rs`, `volume.rs` | Generation from description to columns |
| `crates/terrain-core/src/features/`, `hydro/` | Trees/ore/scatter and world-mode hydrology (`WorldHydro`, one solve shared by workers) |
| `crates/terrain-core/src/light2.rs`, `padded.rs` | Column light, incremental repair, shared padded scratch |
| `crates/terrain-core/src/mesher2.rs`, `lod.rs` | Greedy section meshing, coarse downsample, skirts |
| `crates/terrain-core/src/section.rs`, `lod.rs`, `revisions.rs`, `residency.rs`, `editlog.rs` | Section storage, LOD derivation, revisions, residency policy, edit journal |
| `crates/terrain-core/src/save.rs`, `world_save.rs` | Section files, backup rotation, journal file, `WorldSave`, save worker, repair, migration/report |
| `src/lib.rs` | Demo app, ring generation/light/mesh jobs, LOD selection, save/repair wiring |
| `src/renderer.rs`, `src/terrain_draw.wgsl` | Packed-quad draw path, per-section scale, incremental upload |

## Startup

The window opens immediately; nothing is built synchronously. The ring generates the centre
column first, measures its surface, and moves the player there when the first light pass lands.
Physics refuses movement while the player's column has no light.

| Phase | Release |
|---|---:|
| `Terrain::new` + world hydrology solve | ~165 ms |
| 12 worker threads, one `Terrain` each | < 1 ms |
| First column (generation + light + mesh) | ~0.8 s |
| Full 33x33 ring (1,089 columns) | **~56 s** (LOD1-only ~70 s on the same machine) |
| Steady-state ring footprint (`terrain ring` log) | **~118 MB** (LOD1-only ~126 MB) |
| `terrain ring` quads | **~862,000** (LOD1-only ~1,955,000) |
| Process peak working set (RTX 4060, release) | **~570 MB** |

Every worker owns its own `Terrain` because the pipeline caches behind `RefCell`. One
`Arc<WorldHydro>` solve is shared by every worker, so a lake or river that crosses a column
boundary has one level. Output is keyed and sorted, so worker order cannot change the ring.

The radius is the spec's **16** (`(2*16+1)^2 = 1,089` wanted columns; the two-column unload
margin bounds the resident set at `(2*18+1)^2 = 1,369`). Section storage is paletted and
bit-packed, and only the inner two columns are LOD0, so the measured steady state is ~118 MB of
ring footprint (down from ~126 MB with LOD1 only) and ~570 MB peak working set, near the
~600 MB target. Nearest-first order keeps the centre playable in under a second while the full
ring fills in ~56 s (`ENGINE_NO_VSYNC=1`).

## Light

- `demo_column_light` fans out one job per column.
- Edits use `ColumnView`, a borrowed column plus eight border columns, so no section or border is
  copied.
- `relight_span_view` repairs only the band an edit can reach; the edited column repairs from
  `y = 0` because a block moves the sky seed below it, neighbours only around the edit.
- `section_differs` drives the remesh set: every section whose light changed plus the sections
  touched by the edited block's 26-block neighbourhood.

## Hydrology

- One `WorldHydro` solve covers the finite world (512×512 macro cells) and is installed on every
  worker's `Terrain`, so lakes and rivers that cross a 512-block tile seam have one level.
- Measured cost: ~66 ms to sample 262,144 macro heights plus ~98 ms to flood, route, accumulate, and
  find lakes (~165 ms once per seed).
- `hydro_tile` returns a view when the tile is inside the solved grid and falls back to a standalone
  tile solve outside it.
- This is generator version 2: a version 1 world is refused rather than regenerated.

## Meshing and LOD

- Fine sections are greedy-meshed with `MeshLod::Lod0` inside a two-column radius, with one extra
  ring of fine meshes kept as a transition buffer. A coarse region is split at the same cutoff:
  LOD1 owns complete `2x2` column squares from radius 4 outward, and LOD2 owns complete `4x4`
  column squares entirely beyond radius 7. The LOD1 band is therefore four columns wide and the
  LOD2 band covers the outer half of the ring.
- Each quad takes the layer of the face it belongs to: the block's top reference for `+Y`, bottom
  for `-Y`, and side for `±X`/`±Z` and cross quads (`ADR-0015`). `Textures` are presentation-only,
  so changing one never invalidates a saved section.
- `demo_coarse_regions` downsamples every complete 2x2x2 fine group with `lod::downsample`, meshes
  with `mesh_coarse` at Lod1, and appends skirts on all sides. `demo_far_regions` does the same
  for complete `4x4` column squares with `lod::downsample4` (64 fine sections, the same
  most-frequent-solid vote), `mesh_coarse` at Lod2, and Lod2 skirts (two cells = eight blocks).
- A region is active only when all of its columns are outside the fine buffer, and an LOD1 region
  is suppressed when an active LOD2 square covers it, so the three bands never overlap. Coarse
  LOD is rebuilt whole in the background and is disposable. The LOD2 half shares the same
  throttled job (`TERRAIN_DEMO_COARSE_MS`); an edit only sets `far_dirty`, so the edit path never
  pays for a 64-section derivation.
- `TerrainSection` carries `scale` (1/2/4); the shader scales quad origins, widths, heights, face
  offsets, cross quads, and UVs. Culling uses `32 * scale`.
- `section_revision` includes scale, so an origin changing detail level re-uploads.

## Camera-relative rendering

Both draw paths share one convention (spec 14.3):

- Per frame `Camera::render_view()` chooses `render_origin = floor(camera_position)` as an
  `IVec3` and builds the view matrix from `camera_position - render_origin`. The frustum is
  extracted from that same relative matrix.
- Terrain sections keep their world-space `[i32; 3]` keys. The per-section origin uniform is
  `section_world_origin - render_origin`, computed in `i64` and only then cast to `f32`;
  `terrain_draw.wgsl` adds section-local quad coordinates.
- Legacy chunks keep their `ChunkCoord` keys. The chunk origin uniform is
  `chunk_world_origin - render_origin`; `shader.wgsl` adds only chunk-local cell coordinates.
- `camera_position` in the frame uniform is the camera minus the same origin. Both fragment
  shaders compute fog from `length(relative_world - relative_camera)`, which equals the Euclidean
  world distance. The frame uniform also carries the world render origin, used only by the legacy
  water shader to keep its wave phase anchored to world x/z after rebasing.
- Culling subtracts the origin before testing section and chunk AABBs against the relative
  frustum. The block highlight is uploaded in relative space too and is rebuilt when the origin
  moves.
- Crossing a block boundary rewrites the resident origin uniforms in place (`COPY_DST`). No CPU
  world position changes: picking, collision, streaming, saves, and the ring all stay in world
  space. The CPU camera is still a world-space `Vec3` (`f32`); `RenderView` is the only relative
  representation.

`camera::tests::camera_relative_precision_at_two_to_the_twenty` places the camera at
`(2^20 + 0.5, 64.5, 2^20 + 0.5)`. A nearby section origin is `(32, -32, -32)` relative to the
camera and a chunk is `(0, -32, 0)`; the f32 relative view-projection agrees with an f64
world-space reference to `5.96e-8` in NDC. The same test confirms a raycast returns the world
block and the player lands at world `y = 64`.

## Column ring

- `residency::plan_ring` is the pure frame decision: given the player column, resident columns,
  in-flight jobs, pending builds, protected edits, and a budget, it returns a bounded
  nearest-first generate batch, a nearest-first build batch, and a farthest-first evict batch.
  `RingBudget::v1` is two generate, two build, two evict per frame, with 16 jobs in flight.
- Generation jobs run on a fixed fan-out of workers, one `Terrain` each and one shared
  `Arc<WorldHydro>`. A finished column is inserted immediately; its light pass is a second job.
- A build job computes light for the new column and every present neighbour, then meshes only the
  new column plus neighbour sections whose border light or border blocks changed. The result is
  applied only when its 5x5 column snapshot is still `Arc`-identical, so a stale job is retried
  rather than installed.
- Eviction happens beyond `radius + 2` (`residency::UNLOAD_MARGIN`), farthest first. A column with
  an edit whose section save has not been queued is protected, as is any in-flight column.
- Saved sections and recovered journal frames are replayed at startup over the section records
  before streaming starts, so a column generated later already includes its edits.
- Startup is non-blocking: the player is frozen on the centre column until its light lands, then
  moved to the measured surface. Movement is guarded by `TerrainDemo::aabb_ready`.
- Measured: full 33x33 ring in ~45 s, ~126 MB of section bytes, ~590 MB peak process working set; the
  per-second `terrain ring` info line reports resident/ready/fine/coarse counts and bytes.

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
cargo tree -p terrain-core -e normal --no-default-features
```

`terrain-gen` is the `terrain-core` binary; its golden and counter baselines are in
`crates/terrain-core/tests/golden/`. Current local count: **652 lib tests** across `engine` and
`terrain-core` (2 ignored), **3** `light_preview`, **30** `terrain_gen` integration tests.

## Known gaps

- The renderer's texture array is still placeholder art: per-face references resolve to the eleven
  procedural layers by name, and an unknown name falls back to stone. Skirts on coarse meshes inherit
  the boundary face's layer rather than the side layer.
- Coarse regions are aligned squares, so the outermost column on the positive x/z axes has no
  complete region and relies on the fine transition buffer; the rest of the rim is covered.
- Coarse sections carry skirts on all sides, but a dedicated fine/coarse stitch (instead of the
  extra fine transition ring) and a visible loading indicator are not implemented.
- No normative older section record exists, so migration fixtures cannot be added yet.
- The camera and entities are still `f32`/world-space on the CPU. Camera-relative rendering is
  applied, but the spec's `f64` camera migration is not.
