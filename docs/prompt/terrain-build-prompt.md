# Terrain build prompt

**Purpose:** paste this file (or its contents) as the first message to a fresh agent session in this
repository. It tells the lead agent how to turn the answered terrain questions into code, using
subagents for parallel work.

---

## 0. Your role

You are the **lead engineer** for the terrain subsystem. You do not write most of the code
yourself. You:

1. read the sources of truth,
2. pin the decisions that are still open (or escalate them),
3. define module boundaries and file ownership,
4. dispatch subagents with precise briefs,
5. review and integrate their output,
6. run the verification gates and report honestly.

You are accountable for anything a subagent produces. "The subagent did it" is not an answer.

---

## 0.5 Start in a new branch or worktree — mandatory

Before reading or writing anything:

1. Confirm the working tree is clean (`git status`). If it is not, stop and report what is dirty;
   do not stash, revert, or commit someone else's changes.
2. Create an isolated workspace. Either is acceptable:
   - **Worktree (preferred):** `git worktree add ../valescent-terrain -b terrain/build-v1`, then do
     all work there and treat that directory as the project root for the rest of this session.
   - **Branch:** `git switch -c terrain/build-v1`.
3. Record the branch/worktree path at the top of `docs/design/terrain-build-plan.md`.
4. Never work on `main`/`master`. Never push, never force-push, never rewrite history.
5. Subagents inherit the same rule: they operate inside the worktree, and they do not create
   branches or commits of their own. One branch, many workstreams.
6. Commit only at integration points, one commit per integrated workstream, with the workstream id
   in the message (`terrain: W3 hydrology solver`). Ask the user before the first push.

If the environment provides a worktree tool, use it rather than a manual copy; keep one worktree
for the whole build so paths and incremental build artifacts stay stable.

---

## 1. Read first, in this order

1. `../specs/terrain.md` — **the normative contract.** If it says MUST, implement it exactly.
   Its section numbers are cited by the workstreams below.
2. `../design/terrain-interview.md` — the decision record. Question ids (`A1.1`, `E4.5`, `N3.1`, …)
   are cited by the spec; use them when you need the reasoning, and treat any `- [ ]` as a blocker.
3. `../design/terrain-pipeline.md` — long-range shape, already reconciled with the interview.
4. `../roadmap.md`, `../specs/v0.0.7.md` — current milestone truth where the spec defers.
5. `../decisions/` — accepted ADRs (noise, worker pool, GPU layout, save format, coordinates).
6. `../../src/voxel.rs`, `src/mesh.rs`, `src/mesher.rs`, `src/light.rs`, `src/world.rs`, `src/lib.rs`,
   `src/player.rs` — what exists today: 32³ chunks, greedy meshing, 2 LODs, `VCHK` v3, 15-level light.
7. `../dev/architecture.md`, `../dev/coordinates.md`, `../dev/benchmarks.md`.

Do not start writing code until you have read 1–3 completely and skimmed the rest.

---

## 2. Ground rules

- **An answered question is binding.** Implement it as written, including constants and field names.
- **An unanswered question is not yours to invent.** Leave a typed seam (`// OPEN(A1.5)`), keep the
  behaviour behind a config edge, and append it to `docs/design/terrain-build-plan.md` under
  **Blockers**. Guessing a constant that changes world output is a generator-version event.
- **Determinism rules apply to every stage** (`A2`): no `sin`/`cos`/`tan`/`atan2`/`pow`/`exp`/`ln`,
  no `mul_add`, no fast-math, no unordered float reductions, no `HashMap` iteration order in output
  paths, no NaN/inf. Integer hashing is the only randomness source.
- **Everything expensive is a job** (`K3`, `P1.9`): pure inputs, revision-keyed results, no globals.
- **Small, boring, tested code.** No traits with one implementation, no abstractions "for later",
  no comments explaining what the code says.
- **No new dependencies** without an ADR or an explicit instruction. Available: `bytemuck`, `glam`,
  `micropool`, `wgpu`, `thiserror`, `tracing`, `fastnoise-lite` (per ADR-0012).
- **Never edit a file another workstream owns.** One writer per file.
- **Do not modify** `../design/terrain-interview.md` answers. If implementation reveals a
  contradiction, stop that workstream and report it.

---

## 3. Settled constants (v1)

| Name | Value | Source |
|---|---|---|
| `WORLD_HEIGHT` | 384 (`0..=383`) | C4.3 |
| `SEA` | 128 | C1.2 |
| `MAX_TERRAIN_Y` | 352 | C1.5 |
| `DEEP_Y` | 64 (+4 transition) | G1.4 |
| `SECTION_SIZE` | 32 | I1.3 |
| `SECTIONS_PER_COLUMN` | 12 | I1.3 |
| `MACRO_CELL` | 4 | B4.1 |
| `TILE_BLOCKS` | 128 (32×32 cells, 33×33 samples, +1 border = 35×35) | B4.2 |
| `HYDRO_TILE` | 512 (+64 border) | D8.1 (confirm) |
| `VIEW_RADIUS` | 16 columns, unload at +2, mesh ring `R-1` | K1.1, K1.4 |
| `LOD0/LOD1/LOD2` | 1/2/4 blocks per cell, radii 16/32/48 | N1.1, O3 |
| `CAVE_MIN_ROOF` | 4 | F2.5 |
| `DETAIL_DOWN/UP` | 24 / 8 | F3.3 |
| `RIVER_MIN_CELLS` | 300 | D3.1 |
| `FOG_END` | `LOD2_RADIUS_BLOCKS - 64` | O3.1 |

All of these are world-description parameters except `SECTION_SIZE`, `SECTIONS_PER_COLUMN`, and
renderer/streaming config (`A1.3`).

---

## 4. Settled contracts (types, not suggestions)

```rust
// Section storage (I1.1, I1.2)
enum Section { Uniform(BlockId), Paletted { palette: Vec<BlockId>, bits: u8, data: Box<[u64]> } }
fn section_index(x: u32, y: u32, z: u32) -> usize { (x | z << 5 | y << 10) as usize }

// Height: the only quantiser (C3.1)
fn top_solid_y(h: f32) -> i32 { h.floor() as i32 }

// Derived height tile (E4.6)
struct HeightTile { /* 130x130 i32 */ }

// Lattice (B4.5)
fn macro_cell(x: i32) -> (i32, f32) // floor_div(x,4), rem_euclid(x,4) as f32 / 4.0

// Revisions (I3.2)
// section_rev: u64 per section, border_rev per face, column_rev derived as max

// Errors (S1.1)
struct TerrainError { stage: Stage, kind: ErrorKind, coord: Option<Coord>, context: Vec<Frame> }

// Mesh record (M3.1)
// one u64 per quad: x:5 y:5 z:5 | w1:5 h1:5 | normal:3 | ao:8 | sky:4 blk:4 | tile:16 | tint:4
```

If a workstream needs to change one of these, it reports to you first.

---

## 5. Work breakdown

### Wave 0 — you, serial, before any subagent

1. Pick the remaining pins that block parallel work: module layout and which crate each module
   lives in. The hash is already pinned in the build plan (in-tree SHA-256, SplitMix64 streams) —
   do not reopen it without an ADR.
2. Write `docs/design/terrain-build-plan.md`: workstreams, file ownership, dependency order,
   acceptance commands, blockers.
3. Add the `description.toml` schema skeleton and the `TerrainError` enum. These are shared by
   everything and must exist before parallel work.
4. Confirm the build is green (`cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`).

### Wave 1 — parallel, no shared files

| ID | Workstream | Owns | Read-only | Done when |
|---|---|---|---|---|
| W1 | Section storage | `src/section.rs` | `voxel.rs` | pack/unpack at 1/2/4/8/16 bits, palette growth, Uniform promote/demote, unknown-name round-trip, index/inverse property test |
| W2 | Lattice, noise, curves, graph | `src/lattice.rs`, `src/noise.rs`, `src/graph/**` | `block.rs` | `macro_cell` at negatives, curve monotone cubic, node enum, compiler rejects cycles/unknown/range/chained warp/banned edge, canonical hash |
| W3 | Hydrology solver | `src/hydro/**` | W2 outputs | priority-flood with `EPS=1/64`, D8 ties, accumulation, per-basin lakes, unit tests on hand-made grids |
| W4 | Height, surface, strata | `src/height.rs`, `src/surface.rs`, `src/strata.rs` | W2, W3 | `HeightTile` 130×130, `stone_at`, 9-slot precedence with golden columns, slope drop from `HeightTile` |
| W5 | 3D detail and caves | `src/volume.rs` | W4 | detail band, cheese+spaghetti, entrance mask, span scan, `CAVE_TAP_BUDGET`/`DETAIL_TAP_BUDGET` enforcement |
| W6 | Mesher | `src/mesher2.rs` (or extend `mesher.rs` if isolated) | W1 | greedy key with AO+light, U-profile irrelevant here, 8-byte records, water/air only, cutout range, LOD1 skip |
| W7 | Lighting | `src/light2.rs` | W1 | per-column BFS, water `sky_atten`, incremental == full recompute (fuzz) |
| W8 | Features | `src/features/**` | W1, W4 | 32³ regions, 3×3×3 gather, masks, ore veins, tree templates + jittered grid, scatter placement |
| W9 | CLI and test harness | `src/bin/terrain-gen.rs`, `tests/**` | all crates | `gen/golden/seams/sweep/verify/replay/describe-diff`, `--threads`/`--order` invariance |
| W10 | Diagnostics | `src/metrics2.rs` | — | pinned counter names/units, per-class memory accounting, overlay read model |

Rules for Wave 1:

- Each subagent gets **one** workstream, its own files, and the contracts it may call.
- No subagent edits `../../Cargo.toml`, `src/lib.rs`, or another workstream's files. You do those.
- Every subagent must return: files changed, commands run, test results, open questions, risks.
- If a subagent needs a type from another workstream that does not exist yet, it defines the
  minimal local trait-free struct in its own module and reports the duplication so you can unify.

### Wave 2 — you, serial integration

1. Column pipeline in the fixed order (`F1.5`): `HeightTile` → fill → detail → carve → spans →
   features. This is the heart; do it yourself.
2. Section revisions, border revisions, and the invalidation graph (`I3.2`, `K3.1`).
3. Residency and scheduling: ring, priorities, cancellation, budgets (`K1`–`K4`).
4. Saves and journal: section files, `baseline_hash`, edit log, group commit (`J1`–`J4`).
5. LOD derive + skirts (`N1`, `N3`).
6. Renderer wiring: arenas, indirect draws, passes, fog, camera-relative (`M3`, `O1`–`O3`).

### Wave 3 — hardening

DoD script from `R1.13`, soak and leak test (`S1.7`), panic/hang injection (`S1.2`, `S1.3`),
corrupt-section fixture (`S1.5`), and the perf gates from `P1.7`/`P1.3`.

---

## 6. Subagent brief template

Give every subagent exactly this shape. Keep it short; they have fresh context.

```
Workstream: W<N> — <name>
Repository: <path>. Read docs/design/terrain-interview.md section <X> and the contracts it cites.
Goal: <one paragraph, no alternatives.>
You own: <exact file list. Do not create other files.>
You may call: <existing types/functions, with their signatures.>
Do not edit: <file list.>
Implement exactly these decisions: <question ids + one-line restatement each.>
Out of scope: <explicitly, so it does not invent.>
Tests you must write: <unit/property/golden list, named.>
Acceptance: cargo fmt --check; cargo clippy -- -D warnings; cargo test -p <crate> <filter>;
            plus <specific assertion>.
If blocked: stop, do not guess. Return the question id and the two options you see.
Report: files changed, commands + results, decisions you had to make, open questions, risks.
```

---

## 7. Things that will bite you (check these explicitly)

1. **`top_solid_y` is the only quantiser.** No other `round`, `ceil`, or `as i32` on a height.
2. **Slope reads `HeightTile` before 3D detail and caves** (`E4.3`, `E4.6`); cave walls and span
   tops use `F4.1` rules instead. Do not merge the two paths.
3. **Water emits only water/air faces** (`M2.2`); solids draw their own faces against water.
4. **Light is never saved or hashed**, and incremental relight must equal a full recompute (`L3.3`).
5. **LOD is derived and disposable**, never authority, never saved (`N1.3`).
6. **Sections are the save truth**; there is no baseline regeneration path (`J4.5`).
7. **`Unloaded` is solid for physics** (`I3.3`) — do not "fix" that into air.
8. **`f32` field output must stay in `[0,1]`** with a clamp-fire counter that tests assert is zero.
9. **Every generation loop has a fixed upper bound** (`S1.3`); a loop you cannot bound is a design bug.
10. **`HashMap` in an output path** is a determinism bug even when it "looks stable".

---

## 8. Blockers you must not silently decide

The interview is **fully answered** (334/334); treat every `- [x]` as binding. Nothing is left to
guess, so any deviation is a spec change, not an improvisation. These stay off-limits:

- **Constants that change world output** and are not in the settled table (§3) or the description
  schema. Adding one is a description change and a version bump.
- **Anything the interview defers explicitly**: erosion (`D5`), GPU generation (`B5.3`), occlusion
  culling (`O1.3`), cross-platform equality, fault scarps (`G2`), noodle caves and ravines (`F2.1`),
  in-engine authoring (`Q1.6`), tree growth (`H3.6`), seasons (`E1.4`), water table (`D7.3`),
  per-region parameters (`T1.9`).
- **Anything that would break a stated invariant** (fixed loops, `[0,1]` fields, `Unloaded` solid,
  light never saved, LOD never authority) — stop and report, do not "fix" it locally.
- **Save-format bytes.** They are pinned in `docs/dev/save-format.md`; implementing the missing
  writer is allowed, changing a byte is a format version bump.

If a subagent needs to deviate from any of these, it stops, names the section or question id, and
returns the two options it sees. You decide or escalate; the subagent never guesses.

---

## 9. Verification gates

Run after every integration, and paste the output into your report:

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run --bin terrain-gen -- golden check
cargo run --bin terrain-gen -- seams --window 5x5 --seeds 3
cargo run --bin terrain-gen -- sweep --tier pr
```

Non-negotiables:

- identical hashes for `--threads 1` and `--threads N`, and for `--order forward|reverse|shuffled`;
- no frame over 20 ms while walking on the reference path once the renderer is wired;
- edit p95 ≤ 50 ms once edits are live.

---

## 10. Reporting back to the user

Produce, at the end of each wave:

1. **Shipped** — workstreams complete, with the question ids they implement.
2. **Evidence** — commands run, pass/fail, hashes, benchmark numbers.
3. **Blockers** — question ids, options, and your recommendation.
4. **Deviations** — anything implemented differently from the interview, with the reason and the
   doc that needs updating.
5. **Next** — the smallest next slice that produces a runnable result.

Never report a workstream as done because it compiles. Done means its tests exist, pass, and are
run by CI or by `terrain-gen`.
