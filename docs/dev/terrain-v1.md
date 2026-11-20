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
configured render distance is resident: the default 12 gives 625 columns, and
`[world] render_distance_chunks = 16` gives the spec cap of 1,089. Columns are fine-meshed inside
two columns, drawn as LOD1 through the middle band, and drawn as LOD2 in the far band. Edits, saves,
and repairs use `world/terrain-demo/`; the legacy `world/chunks/` path is untouched.

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
| Full 25x25 ring (625 columns, default `render_distance_chunks = 12`) | **~29 s** |
| Full 33x33 ring (1,089 columns, `render_distance_chunks = 16`) | **~46 s** |
| Steady-state ring footprint, default 12 (`terrain ring` log) | **~72 MB**, **~724,000-776,000 quads** |
| Steady-state ring footprint, cap 16 | **~118 MB**, **~863,000 quads** |
| Process peak working set, default 12 (release, no vsync) | **~476-491 MB** |
| Process peak working set, cap 16 | **~570-686 MB** (run-to-run) |

Every worker owns its own `Terrain` because the pipeline caches behind `RefCell`. One
`Arc<WorldHydro>` solve is shared by every worker, so a lake or river that crosses a column
boundary has one level. Output is keyed and sorted, so worker order cannot change the ring.

The radius is derived from the render distance, `render_distance_chunks.clamp(4, 16)`. The
camera's fog end is `render_distance_chunks * 32` blocks, so the ring ends exactly at the fog and
no resident column sits outside the far plane. The default 12 gives
`(2*12+1)^2 = 625` wanted columns, with the two-column unload margin bounding the resident set at
`(2*14+1)^2 = 841`; `render_distance_chunks = 16` gives the spec ring
(`(2*16+1)^2 = 1,089` wanted, `(2*18+1)^2 = 1,369` resident). Section storage is paletted and
bit-packed, and only the inner two columns are LOD0. The measured steady state is ~71 MB of ring
footprint and ~491 MB peak working set at the default 12, or ~118 MB and ~570-686 MB at the 16 cap.
Nearest-first order keeps the centre playable in under a second while the full ring fills in ~53 s
at 12 or ~46 s at 16 after the narrow border relight (`ENGINE_NO_VSYNC=1`).

## Light

- `demo_column_light` fans out one job per column.
- Edits use `ColumnView`, a borrowed column plus eight border columns, so no section or border is
  copied.
- `relight_span_view` repairs only the band an edit can reach; the edited column repairs from
  `y = 0` because a block moves the sky seed below it, neighbours only around the edit.
- Ring builds use the same narrow repair: a lit neighbour is relit from `y = 0` through the highest
  border cell whose light properties the new column changes, plus `RELIGHT_RADIUS`. A border with
  no light-relevant change leaves the neighbour's light untouched.
- `section_differs` drives the remesh set: every section whose light changed plus the sections
  touched by the edited block's 26-block neighbourhood.

## Hydrology

- One `WorldHydro` solve covers the finite world (512×512 macro cells) and is installed on every
  worker's `Terrain`, so lakes and rivers that cross a 512-block tile seam have one level.
- Measured cost: ~66 ms to sample 262,144 macro heights plus ~98 ms to flood, route, accumulate, and
  find lakes (~165 ms once per seed).
- `hydro_tile` returns a view when the tile is inside the solved grid and falls back to a standalone
  tile solve outside it.
- This is generator version 3: world-mode hydrology plus the per-column biome dither. A version 1
  or 2 world is refused rather than regenerated with different water or biome borders.

## Meshing and LOD

- Fine sections are greedy-meshed with `MeshLod::Lod0` inside a two-column radius, with one extra
  ring of fine meshes kept as a transition buffer. A coarse region is split at the same cutoff:
  LOD1 owns complete `2x2` column squares from radius 4 outward, and LOD2 owns complete `4x4`
  column squares entirely beyond radius 7. The bands stay coherent as the ring shrinks: at the
  default 12 complete far squares remain on the `8..11` band (bases `8` and `-12`); at 8 no
  complete square is clear of the LOD1 radius, so the outer band falls back to LOD1 and LOD2 is
  empty.
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
  extracted from that same relative matrix and both paths cull against it.
- Terrain sections and legacy chunks keep their world-space `i32` keys. Each origin uniform is
  uploaded **once**, when the resident entry is created, as an absolute world-block
  `vec4<i32>`: `xyz` is the origin, and `w` is the section scale (1/2/4) or chunk padding. The
  origin buffers are created with `UNIFORM` and no `COPY_DST`, so `Queue::write_buffer` cannot
  rewrite them.
- Both vertex shaders subtract the frame origin in integer space and only then cast:
  `vec3<f32>(origin.xyz - camera.render_origin.xyz)`. Relative deltas are bounded by the view
  distance, far below `2^24`, so the cast is exact even when the absolute world coordinate is
  not. At `2^30` one block is smaller than an `f32` ULP, but the relative delta is still exactly
  `1`. This is why no absolute world origin is ever cast to `f32`; `terrain_draw.wgsl` adds the
  section-local quad position after the subtraction, and `shader.wgsl` adds only the chunk-local
  cell.
- `camera_position` in the frame uniform is the camera minus the same origin. Both fragment
  shaders compute fog from `length(relative_world - relative_camera)`, which equals the Euclidean
  world distance.
- The legacy water wave used to reconstruct a world coordinate by adding an `f32` render origin.
  It now uses `camera.wave_anchor`: the origin's `0.7x + 0.5z` and `0.6x - 0.4z` phases computed in
  `f64` and wrapped modulo `TAU`. `sin`/`cos` cannot distinguish the wrapped value, so the pattern
  stays world-anchored across a crossing without an absolute `f32` cast.
- Culling uses the same integer subtraction before testing section and chunk AABBs against the
  relative frustum. The block highlight is still uploaded in relative space and is the only buffer
  rebuilt when the origin moves; it is a handful of vertices.
- Crossing a block boundary writes **zero** origin buffers. At the full demo ring the renderer
  held 372 emitting sections plus the legacy scene's 1,388 resident chunks; the old rebase made one
  `write_buffer` call per origin, so a crossing eliminated **1,760** calls. The debug frame log
  reports `origin_buffer_writes=0 avoided_origin_writes=1760` on each crossing. (The ring's 1,089
  resident columns are not all emitting sections; the GPU origin count is per submitted section.)
- No CPU world position changes: picking, collision, streaming, saves, and the ring all stay in
  world space. The CPU camera is still a world-space `Vec3` (`f32`); `RenderView` is the only
  relative representation.

`camera::tests::camera_relative_precision_at_two_to_the_twenty` places the camera at
`(2^20 + 0.5, 64.5, 2^20 + 0.5)`. A nearby section origin is `(32, -32, -32)` relative to the
camera and a chunk is `(0, -32, 0)`; the f32 relative view-projection agrees with an f64
world-space reference to `5.96e-8` in NDC. The same test confirms a raycast returns the world
block, the player lands at world `y = 64`, and that at `2^30` the integer delta is still exact
where an absolute `f32` subtraction would collapse a block to zero.

## Column ring

- `residency::plan_ring` is the pure frame decision: given the player column, resident columns,
  in-flight jobs, pending builds, protected edits, and a budget, it returns a bounded
  nearest-first generate batch, a nearest-first build batch, and a farthest-first evict batch.
  `RingBudget::v1` is two generate, two build, two evict per frame, with 16 jobs in flight.
- Generation jobs run on a fixed fan-out of workers, one `Terrain` each and one shared
  `Arc<WorldHydro>`. A finished column is inserted immediately; its light pass is a second job.
- A build job lights the new column in full. Each already-lit present neighbour reuses its old
  light and repairs from `y = 0` through the highest changed border cell plus `RELIGHT_RADIUS`
  (`ColumnView` + `relight_span_view`): skylight can descend losslessly below a border change,
  while lateral light climbs at most `RELIGHT_RADIUS`. A neighbour whose border light properties
  did not change is not relit at all. It then meshes only the new column plus neighbour sections
  whose border light or border blocks changed. The result is applied only when its 5x5 column
  snapshot is still `Arc`-identical, so a stale job is retried rather than installed.
- Eviction happens beyond `radius + 2` (`residency::UNLOAD_MARGIN`), farthest first. A column with
  an edit whose section save has not been queued is protected, as is any in-flight column.
- Saved sections and recovered journal frames are replayed at startup over the section records
  before streaming starts, so a column generated later already includes its edits.
- Startup is non-blocking: the player is frozen on the centre column until its light lands, then
  moved to the measured surface. Movement is guarded by `TerrainDemo::aabb_ready`.
- Measured after the narrow border relight: the default 12 ring fills in ~29 s (625 columns, ~72 MB,
  ~724,000-776,000 quads, ~476-491 MB peak); the 16 cap fills in ~46 s (1,089 columns, ~118 MB, ~863,000 quads,
  ~570-686 MB peak depending on the run). The per-second `terrain ring` info line reports
  resident/ready/fine/coarse counts and bytes.

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
`crates/terrain-core/tests/golden/`. Current local count: **662 lib tests** across `engine` and
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
