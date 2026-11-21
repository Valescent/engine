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

The headless terrain stack lives in the `terrain-core` crate; `engine` re-exports it under the
public `engine::terrain` facade. Internal demo code keeps using `crate::pipeline`, `crate::section`,
and the other flat crate-root paths, which are `pub(crate)` and not public API.

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
- This is generator version 4: world-mode hydrology, the per-column biome dither, and the retuned
  desert centre. A version 1, 2, or 3 world is refused rather than regenerated with different water
  or biome borders.

## Meshing and LOD

- Fine sections are greedy-meshed with `MeshLod::Lod0` inside a two-column radius, with one extra
  ring of fine meshes kept as the active LOD0 transition buffer (ring `3`). A coarse region is split
  at the same cutoff: LOD1 owns complete `2x2` column squares from radius 4 outward, and LOD2 owns
  complete `4x4` column squares entirely beyond radius 7. The bands stay coherent as the ring
  shrinks: at the default 12 complete far squares remain on the `8..11` band (bases `8` and `-12`);
  at 8 no complete square is clear of the LOD1 radius, so the outer band falls back to LOD1 and LOD2
  is empty.
- **Coverage contract:** every resident column in the visible `(2r+1)` square is covered by exactly
  one active band: LOD0 (ring `3` or less), a complete LOD1 region, a clipped LOD1 region, or a
  complete LOD2 region. The active set has no duplicate section origins and no column drawn by two
  bands.
- Complete regions must be aligned, so two kinds of column are left over and are covered by
  explicit clipped LOD1 regions: the positive row/column of an even-width ring (the outermost `+r`
  base has no `+1` neighbour), and the edge of the LOD0 square where an aligned `2x2` base
  straddles the fine cutoff. A clipped region's mask names the columns it draws; every other cell
  is air, so it never draws over LOD0, a complete LOD1 region, or LOD2. A clipped region keeps its
  scale and its aligned `2x2` origin while that base is outside the LOD0 band. When the aligned
  base itself stays inside LOD0 but the strip it must draw lies outside, the group is tiled into
  `2x2` windows anchored at their smallest drawn column; an L-shaped corner leftover becomes two
  windows. A clipped origin is therefore always outside the LOD0 band, so it never reuses a fine
  section origin. LOD2 is never used for a clipped region.
- Cost of the clipped pass: at radius 12 with an even-aligned player column it emits 32 clipped
  bases (25 on the positive rim, 7 at the LOD0 boundary), 96 active sections (75 rim, 21 boundary),
  about 122k quads and +0.6 MB of ring footprint; at 16 it emits 40 bases (33 rim, 7 boundary), 120
  sections (99 rim, 21 boundary), about 156k quads and +0.8 MB. A fine-owned corner leftover splits
  one aligned base into two anchored windows, so an odd player column can emit one or two more.
  The sections are cached and remeshed only when a mask or a column within one expanded base
  changes, so a full-ring fill does not rebuild the rim on every coarse pass.
- Each quad takes the layer of the face it belongs to: the block's top reference for `+Y`, bottom
  for `-Y`, and side for `±X`/`±Z` and cross quads (`ADR-0015`). `Textures` are presentation-only,
  so changing one never invalidates a saved section.
- `demo_coarse_regions` downsamples every complete 2x2x2 fine group with `lod::downsample`, meshes
  with `mesh_coarse` at Lod1, and appends skirts on all sides. `demo_coarse_regions_masked` is the
  same path with the clipped-region mask: an unmasked or missing column contributes air. It also
  takes the set of columns the active coarse bands own, so a pad cell over a LOD0 column can only
  cull when its whole fine `2x2x2` group is opaque (see the coarse stitch below).
  `demo_far_regions` does the same for complete `4x4` column squares with `lod::downsample4` (64
  fine sections, the same most-frequent-solid vote), `mesh_coarse` at Lod2, and Lod2 skirts (two
  cells = eight blocks).
- **Fine sides of the transition are stitched.** A LOD0 column at ring `3` (the fine cutoff) appends
  a skirt on each side whose column just outside it is in the active coarse band; a fine column
  fully inside the cutoff gets none, and a fine-owned edge (for example the west side of a boundary
  column) keeps its culled wall. The side set comes from band membership, not from the coarse set's
  build state, so a throttled coarse pass cannot move the stitch. The depth is read from the drawn
  LOD1 surface: the adjacent column's top face is derived with the same conservative vote as the
  coarse mesher (air, water, and scatter cells do not count), a flush or higher coarse edge appends
  nothing, and a lower edge appends exactly the drop, capped at `SKIRT_DEPTH`. The skirt is applied
  to each active LOD0 clone during `select_lod`, so the stored fine mesh stays unskirted and a
  repeated selection cannot double a strip. The fine band is the same `7x7` square at every ring
  radius, so the stitch cost does not scale: with `ENGINE_SEED=12345` it is **+119 quads and +952
  bytes** (about 0.014% of the 850,895 active quads at radius 12; the same absolute cost at 16,
  where the active set is larger).
- **The coarse side of the transition is stitched too.** A complete or clipped LOD1 section whose
  mask is entirely coarse-owned masks the one-cell pad it meshes against. A pad cell over a column
  the coarse bands do not own belongs to LOD0, so it counts as opaque only when every fine block of
  its `2x2x2` group is an opaque full cube. The conservative downsample marks the cell solid from a
  single solid block, which snaps it to the group's top plane; that used to cull the coarse
  band-edge wall and leave up to one coarse cell above the drawn fine top open. The all-opaque test
  emits the wall over that cell instead. A flush edge keeps the cull (the whole group is opaque),
  and an interior coarse-to-coarse side, whose columns are owned, is unchanged. The rule is derived
  from band ownership, not from the coarse set's build state. Cost at radius 12 with
  `ENGINE_SEED=12345`: **+742 quads and +5,936 bytes** (about 0.087% of the active quads and 0.008%
  of the 72.1 MB footprint; a live `terrain ring` sample moved 850,891 -> 851,637 active quads).
  It does not scale with the ring radius because the fine band is the same `7x7` square.
- **LOD water is all-or-nothing.** After the solid vote, a coarse cell is water only when every
  cell in its 2x2x2 or 4x4x4 group is water; any air makes it air. A coarse cell's `+Y` face sits
  on its group's top plane, so an any-water group whose missing cells were at the top rendered a
  translucent sheet above the fine surface (one block at Lod1, up to three at Lod2). The
  conservative rule can instead drop the top coarse cell at a band edge: the coarse water top is
  then at most `2` (Lod1) or `3` (Lod2) blocks below the fine top. That step is a small step down,
  not a hole: the coarse region still fills the water volume below, and the fine band keeps its
  own opaque boundary skirt over the terrain. `coarse_water_top_stays_at_or_below_the_fine_surface_lod1`
  and `_lod2` pin the invariant across every surface remainder; `water_is_all_or_nothing_so_the_top_never_rises`
  fails on the old any-water rule.
- **Water draws far to near.** `TerrainDraw::draw_water` sorts the visible water sections by
  squared distance from the frame camera origin to the section centre (ties by world origin)
  before binding anything. Opaque and cutout keep submission order. Water depth writes stay off:
  quads inside one section are not sorted, a written depth could reject a farther fragment of the
  same draw, and the highlight pass after water must read through the surface. Sorting alone makes
  the blend deterministic now that no coarse sheet rises above another surface.
- A region is active only when all of its drawn columns are outside the fine buffer, and an LOD1
  region is suppressed when an active LOD2 square covers it, so the three bands never overlap. A
  throttled coarse set can be one ring layer stale, so `select_lod` also resolves any residual
  claim on one origin deterministically: the finer scale wins, then the smaller origin.
  Coarse LOD is rebuilt whole in the background and is disposable. The LOD2 half shares the same
  throttled job (`TERRAIN_DEMO_COARSE_MS`); an edit only sets `far_dirty`, so the edit path never
  pays for a 64-section derivation.
- `TerrainSection` carries `scale` (1/2/4); the shader scales quad origins, widths, heights, face
  offsets, cross quads, and UVs. Culling uses `32 * scale`.
- `section_revision` includes scale, so an origin changing detail level re-uploads.

## Textures

- The block array holds one 16×16 RGBA8 layer per tile (eleven layers) and a full five-level mip
  chain (16, 8, 4, 2, and 1 texels). Levels are generated at startup by averaging each 2×2 group
  of the level below; level 0 is byte-identical to the procedural tile generator and alpha stays
  255 (`ADR-0016`).
- The sampler magnifies nearest, minifies linear, blends mip levels linearly, repeats in U and V,
  and clamps anisotropy to 1. Close-up art stays crisp while distant terrain stops shimmering.
- The chain adds 3,740 bytes (15,004 bytes total, up from 11,264) and about 18 µs of one-off
  startup work.
- Layers stay isolated: each level is generated from its own layer, and array sampling never
  filters across layers, so there is no atlas padding or UV inset. Greedy UVs and the shader's
  sample call are unchanged.
- Tile artwork, the layer table, and per-face references are still placeholder content; the
  texture pipeline page records the remaining asset-pipeline gap.

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
  whose border light or border blocks changed.
- A build starts only when every column in its read window that the ring will generate is
  resident, so the snapshot cannot change while the job runs. A mesh-only upgrade waits for and
  validates the 3x3 it reads; a full light pass still uses the 5x5. The result is applied only
  when that snapshot is still `Arc`-identical, so a genuinely stale job is retried rather than
  installed, but a cold fill now reports zero discards.
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
- A retry pass on 2026-11-20 waits for each build's in-ring dependency window instead of starting
  it immediately. On a shared host the radius-12 ring issued 256-260 builds instead of 828-874 and
  the radius-16 ring 385-409 instead of 1,386-1,441, with `discarded_stale` going from 25-31% to
  zero. The least-contended fills fell from 35.7 to 27.8 s at radius 12 and from 65.4 to 45.6 s at
  radius 16; medians over repeated runs were 62.5 -> 60.4 s and 82.4 -> 71.5 s. Steady quads and
  bytes are unchanged within run-to-run LOD order (724,137 quads / 71.5 MB at 12, 863,440 /
  118.0 MB at 16). Generation still dominates the fill.

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
`crates/terrain-core/tests/golden/`. Current local count: **701 lib tests** across `engine` and
`terrain-core` (2 ignored), **3** `light_preview`, **30** `terrain_gen` integration tests.

## Known gaps

- The renderer's texture array is still placeholder art: per-face references resolve to the eleven
  procedural layers by name, and an unknown name falls back to stone. Skirts on coarse meshes inherit
  the boundary face's layer rather than the side layer.
- Coarse regions are aligned squares. The positive rim of an even-width ring and the edge of the
  LOD0 square are covered by clipped LOD1 regions whose non-drawn cells are air, so every resident
  ring column is covered once. A clipped region whose aligned base is itself inside the LOD0 band
  is anchored at one of its drawn columns instead, so its `2x2` bounds can still overlap a finer
  section (which emits the geometry there) but its section origin never does.
- The LOD0/coarse transition is stitched on both sides (see Meshing and LOD): a fine column at the
  cutoff skirts each side that faces the active LOD1 band with the step read from the drawn coarse
  surface, and the coarse mesher treats a pad cell over a fine-band column as opaque only when its
  whole fine `2x2x2` group is opaque, so a snapped-up downsample cannot cull the band-edge wall.
  The extra fine transition ring and the clipped LOD1 regions are still the selection mechanism;
  the stitch only closes the dropped surface. A visible loading indicator is also not implemented.
- No normative older section record exists, so migration fixtures cannot be added yet.
- The camera and entities are still `f32`/world-space on the CPU. Camera-relative rendering is
  applied, but the spec's `f64` camera migration is not.
