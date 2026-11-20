# Terrain build plan

**Branch:** `terrain/build-v1` (created from `main` at `de46916`; the branch carries the
uncommitted design-doc edits because a worktree would not).

**Authority:** [Terrain interview](terrain-interview.md). Question ids (`A1.1`, `D3.2`, `N3.1`)
refer to that page. This plan does not answer questions; it records when and where they are
implemented, and which ones are still open.

## 0. Wave 0 pins

These are the decisions Wave 1 needs before it can start in parallel. They are recorded here so a
later reader can see exactly what was assumed.

| Pin | Value | Why |
|---|---|---|
| Hash for identity (`B3.8`) | **SHA-256, implemented in-tree**, `src/hash.rs`. | The repo forbids new dependencies without an ADR. `sha2` and `xxhash-rust` are both new deps; an in-tree SHA-256 is ~90 lines, has published test vectors, and yields the `[u8; 32]` `description_hash` that `T1.2` already specifies. To be replaced by a crate only behind an ADR; the *output bytes* are the contract, not the implementation. |
| Hash for streams (`A3.1`) | **SplitMix64 finaliser over a fixed `u64` word sequence**, `hash64(seed, domain, coords)`. `hash01` = top 24 bits `/ 2^24` (exact in `f32`). | Deterministic, integer-only, ~5 ops per word, no dependency, no platform variance. Pinned forever once a world ships. |
| Crate layout | Workspace with `engine` (app, renderer, camera, player, legacy world) and `crates/terrain-core` (generation through persistence). | The headless tool must not link `wgpu`/`winit`; the crate boundary makes that enforceable. |
| Binary | `crates/terrain-core/src/terrain_gen.rs`, target `terrain-gen`. | The tool lives with the code it exercises and keeps its name across the workspace. |
| `Section`/`BlockId` | `BlockId = u16`; `Section` is the `I1.1` enum verbatim. | `I1.1`. |
| Terrain modules are `pub` | New modules expose `pub` items; existing modules stay `pub(crate)`. | A second binary target in one package is a separate crate and needs a `pub` path. Deviation from the `pub(crate)` house style, forced by the tool requirement in `Q1.9`. |

### Known deviations from the prompt

1. **`terrain-gen` links `wgpu`.** `Q1.9` requires a CI crate-graph check that the headless binary
   does not link window/GPU/renderer code. That check is impossible in a single package. Deferred:
   split `terrain-core` out of `engine` when the binary is wired, then re-enable the check. Recorded
   in Blockers.
2. **No `toml` dependency.** `A1.1` wants a TOML description; the repo has no TOML crate. The
   description reader is a strict parser for the flat `key = value` subset plus `[meta]`, rejecting
   everything else rather than guessing. The on-disk text stays TOML-compatible.
3. **Worktree not used.** Uncommitted design docs would not survive a worktree checkout.

## 1. File ownership

One writer per file. `src/lib.rs`, `Cargo.toml`, and `docs/**` belong to the lead.

| Workstream | Owns | Reads |
|---|---|---|
| W0 (lead) | `src/hash.rs`, `src/error.rs`, `src/description.rs`, `src/lib.rs`, `Cargo.toml` | everything |
| W1 Section storage | `src/section.rs` | `src/voxel.rs` |
| W2 Lattice, noise, curves, graph | `src/lattice.rs`, `src/noise.rs`, `src/curve.rs`, `src/graph/mod.rs`, `src/graph/node.rs`, `src/graph/compile.rs` | `src/block.rs`, `src/hash.rs` |
| W3 Hydrology | `src/hydro/mod.rs`, `src/hydro/flood.rs`, `src/hydro/flow.rs`, `src/hydro/lake.rs` | `src/lattice.rs`, `src/hash.rs` |
| W4 Height, surface, strata | `src/height.rs`, `src/surface.rs`, `src/strata.rs` | W2, W3 outputs |
| W4c Climate, biomes | `src/climate.rs` | W2 outputs |
| W5 3D detail and caves | `src/volume.rs` | `src/height.rs` |
| W6 Mesher | `src/mesher2.rs` | `src/section.rs` |
| W7 Lighting | `src/light2.rs` | `src/section.rs` |
| W8 Features | `src/features/mod.rs`, `src/features/ore.rs`, `src/features/tree.rs`, `src/features/scatter.rs` | `src/section.rs`, `src/height.rs` |
| W9 CLI and harness | `crates/terrain-core/src/terrain_gen.rs`, `crates/terrain-core/tests/**` | all |
| W10 Diagnostics | `src/metrics2.rs` | — |

## 2. Dependency order

```text
W0  (serial)
 ├── W1 ──────────────┐
 ├── W2 ── W3 ── W4 ──┼── W5
 │        │      │    ├── W6 (needs W1)
 │        │      │    ├── W7 (needs W1)
 │        │      └────┴── W8 (needs W1, W4)
 ├── W10 (independent)
 └── W9  (last; needs the public surface of everything it drives)
```

Wave 1 dispatch order: `W1, W2, W10` first (no dependency on each other and none on an answered
contract that does not exist yet). `W3, W4` second. `W5, W6, W7` third. `W8` fourth. `W9` last.
Workstreams still run concurrently where the graph allows; the order is about *file readiness*, not
scheduling strictness.

## 3. Acceptance commands

Per workstream:

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test <module filter>
```

Per integration (Wave 2) and per release candidate:

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run --bin terrain-gen -- golden check
cargo run --bin terrain-gen -- seams --window 5x5 --seeds 3
cargo run --bin terrain-gen -- sweep --tier pr
```

Non-negotiables copied from the prompt: identical hashes for `--threads 1` vs `--threads N` and
for `--order forward|reverse|shuffled`; no frame over 20 ms while walking once the renderer is
wired; edit p95 ≤ 50 ms once edits are live.


## 4. Remaining work

No open design questions remain. The interview is closed (333/334, with `J4.4` now answered by a
complete format), and the spec records accepted limitations in §23. What follows is the work that is
still unimplemented, in dependency order. Nothing here blocks the terrain engine from running.

| # | Work | State | Next action |
|---|---|---|---|
| 1 | Persistence writer: section files, backup rotation, journal IO thread, `Migration`, `world-repair` | Section writer/reader, backup rotation, `WorldSave` load order, `SaveWorker` IO thread, demo open/save, journal replay, prefix trimming, the three repair actions, `terrain-gen repair`, the in-app repair prompt (keys and mouse), the header v1→v2 upgrade, section canonicalisation, `terrain-gen migrate`, and the migration report are done | Add a decoder for an older section record format |
| 2 | Persistence tests: migration fixtures (v1/v2/v3), corruption fuzz, kill-and-resume | Section round trip, rotation, temp recovery, torn tail, corrupt frame, world load order, worker ordering, revision advancement, journal replay, prefix trimming, repair actions, v1 header upgrade, migration resume/report, corruption fuzz, journal truncation at every byte, and repeated crash/save/reopen pass | Add migration fixtures for older record formats |
| 3 | Frozen `versions.toml` + deprecation dialog | Policy specified (T1.12); no table or UI | Needed before the first generator bump |
| 4 | Panic/deadline wrapper around jobs (`S1.2`, `S1.3`) | Jobs are plain functions; no watchdog | Wrap with `catch_unwind` and the soft deadline |
| 5 | Demo integration: real light, texture refs, collision, streaming, LOD | `TERRAIN_DEMO` uses real column light, collision, edits, per-edit saves, and distance LOD0/LOD1/LOD2; tiles stream with prefetched neighbours; per-face texture references resolve to procedural layers (`ADR-0015`) | An asset pipeline to replace the placeholder layers |
| 6 | World-mode hydrology wiring | `Terrain::hydro_tile` returns a view of one `WorldHydro` solve, shared by every worker; generator version 2 | None |
| 7 | Camera-relative rendering (§14.3) | Specified; camera is world-space | Camera-wide change; the draw path already matches the current convention |
| 8 | `terrain-gen map`, report bundle, `calibrate-blend`, PR counter gate | `map --seed --from --size --cell` (ASCII surface map, `--json`); `verify --report <dir>` writes `manifest.json`, `verify.json`, and `verify.txt`; `calibrate-blend` measures W_blocks per biome pair; `counters --tier pr` gates a deterministic baseline in CI | Tune the biome centres/weights so the measured medians reach 20-28 and p90 reaches <=64 |
| 9 | Split `terrain-core` from `engine` | Deferred; `terrain-gen` links `wgpu` today | Enables the `Q1.9` crate-graph check |

Ordering rationale: persistence first because it is the only thing that can lose player work; the rest
are capability or polish items that do not change the format.
## 5. Status

Measured on `terrain/build-v1` on 2026-11-19: `cargo fmt --check` and
`cargo clippy --all-targets -- -D warnings` clean; `cargo test` reports **629 unit tests**
(2 ignored) plus 3 `light_preview` and 24 `terrain_gen` integration tests passing.
`cargo test --all-targets` is not a gate: `benches/pool_sim.rs`
deliberately exits 1 when a scenario exceeds its budget.

| Workstream | State |
|---|---|
| W0 hash, errors, description | done (`src/hash.rs`, `src/error.rs`, `src/description.rs`); SHA-256 + SplitMix64, canonical bytes, strict schema |
| W1 section storage | done (`src/section.rs`); `Uniform`/`Paletted`, bit widths, `solid` count deviation recorded |
| W2 lattice, noise, curves, graph | done; measured `NOISE_BOUND_2D = 1.019_993_3`, `3D = 1.019_797_9` |
| W3 hydrology | done: one world-mode solve, shared by workers |
| W4a strata | done; `stone_at`/`stone_in` equivalence tested |
| W4b height, surface | done except sea-floor bands (in progress) |
| W4c climate, biomes | done except the v1 table (in progress) |
| W5 volume, caves | done except the answered depth windows (in progress) |
| W6 mesher | done; open integration item: 26-neighbour AO input |
| W7 lighting | done, including the bit-identity repair gate |
| W8 features | done except the uniform-air section bug |
| W9 CLI/harness | `gen`, `golden`, `seams`, `sweep`, `describe-diff` real; `verify`/`replay` in progress; `map` not started |
| W10 diagnostics | done; pinned metrics, ledger cap, shed ladder, overlay model |
| W12 revisions | done (`src/revisions.rs`) |
| W13 padding | done (`src/padded.rs`); one padded layout for blocks and light |
| Residency, streaming | `src/residency.rs` owns the pure ring policy (`plan_ring`, `RingBudget`); the demo drives it over a column ring with LOD0/LOD1/LOD2 coarse meshes |
| Edit log, header | done as records and framing (`src/editlog.rs`, `src/header.rs`); journal IO thread and save writer pending |
| Pipeline | done (`src/pipeline.rs`): `F1.5` order, graph, tile caches, column/section, features wired |
| LOD | derive and skirts done (`src/lod.rs`, `src/mesher2.rs`); the demo derives LOD1 for complete 2x2 regions and LOD2 for complete 4x4 regions away from the player, with per-section scale in the draw path |
| Renderer | not wired: new quad path in progress |
| Wave 3 (hardening) | not started: saves, migration, fuzz, soak |

## 6. Active workstreams (2026-11-19)

One writer per file. The lead owns `src/lib.rs`, `Cargo.toml`, `docs/**`, and integration; every row
below is a delegated workstream on files nobody else edits. A workstream is done only when its
gates pass on a tree that compiles.

| Id | Workstream | Files | Status |
|---|---|---|---|
| W14 | Renderer quad path: vertex-pulled 8-byte records, opaque + water, camera-relative | `src/renderer.rs`, `src/terrain_draw.wgsl` | **done** — naga-validated shader, `decode_quad` tests; not yet called from the app |
| W15 | `terrain-gen verify` + `replay` + `repair` | `crates/terrain-core/src/terrain_gen.rs`, `crates/terrain-core/tests/terrain_gen.rs` | **done** — 24 tests; canonical v2 record via `decode_record_at`; one shared `baseline_hash`; repair actions |
| W16 | Content ids + v1 biome table + goldens | `src/content.rs`, `src/pipeline.rs` | **done** |
| W17 | Close `OPEN` markers: sea-floor bands, cave windows, humidity order, world-mode hydrology | `src/surface.rs`, `src/volume.rs`, `src/climate.rs`, `src/hydro/mod.rs`, `src/hydro/lake.rs` | **done** - world mode solved once and wired through `hydro_tile` (generator version 2); bands now wired in the pipeline |
| W18 | Strict `description.toml` reader/emitter | `src/description.rs` | **done** — 22 tests |
| W19 | Mesher: 26-neighbour AO + skirt quads | `src/mesher2.rs` | **done** — 30 tests; record layout unchanged |
| W20 | Features: uniform-air sections, water margin, no growth | `src/features/mod.rs`, `src/features/tree.rs` | **done** — gate also wired in `pipeline.rs` |
| W21 | Journal commit policy + writer | `src/editlog.rs` | **done** — 19 tests |
| W22 | Block-id resolution + surface stack wiring | `crates/terrain-core/src/pipeline.rs`, `crates/terrain-core/tests/golden/terrain-v1.golden` | **done** — `TerrainMaterials`, `with_pack`, seabed/snow/bedrock wired, goldens regenerated once |
| W23 | Canonical section record encode/decode | `src/section.rs` | **done** — v2 `TSEC` layout, 22 tests; bin adoption in progress |
| W24 | Terrain demo wiring (`TERRAIN_DEMO=1`) | `src/lib.rs`, `src/renderer.rs` | **done** — column ring at the spec radius 16 (1,089 columns), real column light, collision, async edits, per-edit saves; LOD0 within two columns, LOD1 through the middle band, LOD2 far; per-face texture refs (`ADR-0015`); first column ~0.8 s, full ring ~56 s, ~118 MB ring footprint, ~570 MB peak, ~862k quads; ~24 ms edit rebuild |
| W25 | Residency-driven column ring | `src/residency.rs`, `src/lib.rs` | **done** — `plan_ring` (bounded generate/build/evict), neighbour relight and border remesh, `Arc` snapshot validation, saved/journal replay before streaming, player frozen until the centre column is ready |

Queued next:

1. **Texture art.** Per-face references and the mesher wiring are done (`ADR-0015`);
   `demo_texture_layer` still maps names onto eleven procedural placeholder layers.
2. **Persistence follow-ups:** a decoder for an older section record format, and a richer world-repair dialog.
3. **Blend calibration data.** The `calibrate-blend` tool and the PR counter gate are done.
   Measured medians at `blend_width = 0.05` are 43-72 blocks against the 20-28 target, and p90 is
   117-278 against <=64; halving the width lowers the median to ~31 but not p90. Tuning the biome
   centres and weights is a generation change (version bump + goldens).

Verification at the 2026-11-20 ring merge point: `fmt`, `clippy`, `cargo test` (645 lib, 2 ignored,
3 light_preview, 24 integration) and `golden check` exit 0. Start the demo with `TERRAIN_DEMO=1`
and `cargo run --bin engine`; it needs `world/seed` (or `ENGINE_STRESS_SCENE=1`).

## 7. Review notes on landed work

Recorded while the tree is still in flux. Each is either a follow-up or something the final gate must
prove; none is silently accepted.

| Workstream | Note | Action |
|---|---|---|
| W16/W22 | `core:snow` is a full opaque cube; §10.5 now says so and the layer variant is deferred (§22) | No action; the layer is a later content + render-shape change |
| W22 ids | Numeric `STONE`/`DIRT`/`GRASS`/`SAND` are gone; `TerrainMaterials` resolves every name and `Terrain::with_pack` exists. `AIR`/`WATER` remain only for the tool and are pinned by a test | Delete the two tool constants when `terrain-gen` reads ids from the pack |
| W22 error | The agent added `ErrorKind::ManifestIncompatible { id, reason }` to `src/error.rs`, outside its stated ownership, because the variant did not exist | Verified present with `Fatal`/`RefuseLoad`; keep |
| W20 | `uniform_writable` is now the pipeline gate (the lead applied the call-site change), so a canopy crossing a section top and an ore entering uniform stone both work | None; covered by tests |
| W14 renderer | Draw path is additive and naga-validated; `TERRAIN_DEMO=1` now builds a 96×96 patch (45 emitting sections, ~2.35 s) and submits it once. Opaque culling is off (cross-quads); origins are world-space because the camera has no floating origin | Review on a real run; revisit camera-relative when §14.3 lands |
| W19 cracks | White specks in caves were **T-junction rasterisation cracks**, not lighting and not the highlight. Cell coverage is exact (`every_exposed_face_is_covered_once`: 0 holes, 0 overlaps per face plane); the gap was sub-cell. Fixed by expanding quads 0.001 blocks in the vertex shader (`M3.1` note) | Do not remove the overlap; a future shadow pass must account for it |
| W24 demo | Real `light2` per column (pack-driven `sky_atten`/`opacity`/emission), collision against the patch, spawn on the patch surface; per-face texture references resolve to the renderer's 11 procedural layers; distance LOD1/LOD2 and tile streaming are wired | Texture art and residency-driven ring streaming are the follow-ups |
| W15 CLI | `verify`/`replay` are real and use the canonical record; unknown names are warnings, corrupt records are per-section drift, exit codes unchanged. `MAX_KNOWN_BLOCK` was hardcoded to 8 and rejected ids ≥15; it now derives from the pack | None |
| Goldens | All six fixtures changed when ids were resolved (expected: `Section::content_hash` is over ids); two updates produced identical bytes | One more regeneration only if a later change alters output |
| W23 section record | Re-encoding a **degraded** section would replace original unknown names with `?unknown:<id>` (`T1.6`) | Papered over by policy, not code: a world with unknown names loaded is **read-only** (no edits, no journal, no saves), so the round trip stays lossless. A name-override map is optional polish, not required |
| W23 section record | Unknown ids encode as the diagnostic name `?unknown:<id>`, unreachable by that policy | Same; only reachable if the read-only rule is bypassed |
| W21 journal | Writer and policy land; the IO thread that drives them does not exist. `flush_if_due` does at most one `write_all`; a partial append is session-fatal and recovered on reopen; the 10 s barrier only fires during a flush | Wire when the save writer lands |
| W17 hydro | v1 ships world mode: one 512x512-cell solve, shared by every worker, wired through `hydro_tile`. Tile mode remains for tiles outside the solved grid | A generator version bump is required for any further change to levels |
| W17 constants | `T_COLD_SHORE`, `T_COLD_SHELF`, and the clay-patch share are code constants; §2.1 now allows a generator-pinned constant | Changing any of them is a generator version bump |
| W18 description | `[meta]` must be the last table; earlier placement yields a "missing key" error | Document "meta last" or improve the error |
| W18 description | Accepts legacy bare strings/refs alongside quoted | Keep both; quoted is canonical |



## 8. Remaining roadmap (2026-11-20)

The demo now has column-ring streaming, asynchronous first-column startup, ~24 ms edits, LOD0/LOD1/LOD2,
persistence, migration reports, and repair. The remaining items are larger than a single patch;
each entry says why and what the first safe step is.

Landed since the 2026-11-20 review: world-mode hydrology (generator version 2), camera-relative
rendering with a 2^20 precision test, the spec-radius column ring (1,089 columns), LOD2 in the far
band (measured ~862k quads and ~118 MB versus ~1.955M and ~126 MB for LOD1 only), `calibrate-blend`
plus the PR counter gate, and the `terrain-core` split with a CI crate-graph check.

| Item | Why it is not a small change | First safe step |
|---|---|---|
| Texture art pipeline | `ADR-0015` and the per-face references are done; the renderer still builds eleven procedural placeholder layers, and a real pack needs an atlas format, mipmap policy, and visual baselines. | Define the atlas/mipmap policy, then generate layers from authored tiles. |
| Biome blend data | `calibrate-blend` reports medians 43-72 at `blend_width = 0.05` (target 20-28) and p90 117-278 (target <=64); `0.025` gives 26-39 and p90 117-161. | Tune the biome centres/weights, then add the gradient-normalising term, with a generator version bump and fresh goldens. |
| Ring memory headroom | 1,089 columns cost ~118 MB of ring footprint and ~570 MB peak against a ~600 MB target. | Add a compression or LOD-residency pass before the rim grows. |
| Migration fixtures for older section formats | There is no normative older section record. Inventing v1 bytes would contradict `docs/dev/save-format.md`, whose version 2 record is the only layout the decoder knows. | Add a format bump + ADR defining v1, then fixtures and `migration.journal` transforms against it. Until then, `terrain-gen migrate` canonicalises v2 records and upgrades headers. |
| LOD transition stitching | LOD2 is wired into the ring's coarse selection; coarse sections skirt all four sides, and the LOD0 edge still uses the extra fine transition ring rather than a stitch. The positive-axis rim column has no complete aligned coarse region. | Add a dedicated fine/coarse stitch and cover the positive rim column with an explicit partial region. |