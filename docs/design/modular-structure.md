# Modular structure

**Status:** Proposal for review. It changes no code.

**Kind:** Design note. The [terrain spec](../specs/terrain.md) is normative, and the
[architecture overview](../dev/architecture.md) records the current tree. Where this page and the
spec disagree, the spec wins. Where it disagrees with the executable source, the source wins and this
page is a bug.

**Scope:** The workspace layout, crate graph, module boundaries, dependency direction, visibility,
test seams, and size budgets of `engine` and `terrain-core`. It is grounded in the tree at the
`feature/modular-plan` tip: 58 Rust files under `src/` and `crates/terrain-core/src/`, 51,719 lines,
and 97 commits of history.

**Non-goals:** No behaviour change, no save-format change, no generator output change, no renderer
change, no performance work, and no rename of a module that the terrain spec pins. No new crate is
proposed for this milestone.

## 1. What "modular" means here

A module is a responsibility assignment that hides one design decision likely to change
([Parnas, 1972](https://dl.acm.org/doi/10.1145/361598.361623)). A tree is modular to the extent that:

1. changing one hidden decision touches one module;
2. source dependencies point one way, so a lower module never needs to know its callers;
3. the interface is smaller than the implementation, so a reader can use the module without reading
   its body.

File count, folder depth, and layer names are not evidence. A 200-line file with a leaky interface can
be less modular than a 1,200-line file with one. The rubric in §3 is written so each criterion can be
checked against the tree, not argued from taste.

The measured baseline is not a small codebase: `src/lib.rs` is 5,581 lines and changed in 36 of the
last 97 commits. That file, not the folder names, is the main structural problem this plan addresses.

## 2. What the evidence actually says

The sources below are not equal. Information hiding, the dependency rule, and the Rust visibility
rules are strong consensus with decades of use or a compiler behind them. Deep versus shallow
modules is a well-argued opinion, not a measurement. The size-defect literature is genuinely
contested. The labels below say which is which, so the plan does not present taste as evidence.

### 2.1 Decompose by hidden decisions, not by steps (strong consensus)

Parnas compares a KWIC index split by processing step (input, circular shift, alphabetize, output)
with one split by hidden decision (line storage, circular shifter, alphabetizer). The step
decomposition forces every module to know the line-storage format, so a storage change touches every
module. His conclusion is direct: "It is almost always incorrect to begin the decomposition of a
system into modules on the basis of a flowchart" (Parnas 1972, §Conclusion).

**Operational rule:** the spec's stage order (`height -> surface -> strata -> volume -> features ->
hydro`) is a *contract for the output*, not a module diagram. Keep the order in one orchestrator,
`pipeline`, and let each stage module own its data and intermediate representation. Do not create
`stage_height`, `stage_climate`, and `stage_volume` modules that pass raw tables to each other. A
change to the cave threshold should touch `volume`, not `height` and `pipeline`.

The same test applies to the engine. A change to how a column ring result is applied should touch the
ring module, not `App`, the renderer, and `main`. Today it touches `src/lib.rs` because the ring, the
patch, the save worker, the repair prompt, and the frame loop share one file.

### 2.2 Cohesion and coupling are diagnoses, not instructions (definitions are consensus; the advice is a heuristic)

The terms come from Stevens, Myers, and Constantine, "Structured Design" (IBM Systems Journal, 1974),
which ranks cohesion and coupling into levels
([Stevens, Myers, and Constantine, 1974](https://doi.org/10.1147/sj.132.0115)). Martin's
package metrics turn coupling into instability `I = Ce / (Ca + Ce)`, abstractness `A`, and distance
from the main sequence `D = |A + I - 1|`
([Martin, 1994](https://linux.ime.usp.br/~joaomm/mac499/arquivos/referencias/oodmetrics.pdf)).

"High cohesion, low coupling" is not actionable on its own: it does not say *which* changes belong
together, and almost any split can be defended as "more cohesive". The actionable version is:

- one hidden decision per module, so the module has one reason to change;
- stable modules (many dependents, like `description`, `section`, `content`, `error`) depend on
  almost nothing and stay data-only;
- unstable modules (`pipeline`, `save`, `world_save`) may depend on many, and nothing below them may
  depend on them;
- measure *change* coupling, not only static imports. Gall, Hajek, and Jazayeri show that release
  history exposes logical dependencies that static analysis misses
  ([Gall et al., 1998](https://ieeexplore.ieee.org/document/738508)).

**Operational rule:** a module that changes with every feature is a hotspot even if its import list
looks clean. `src/lib.rs` is exactly that: 36 of 97 commits touch it, and 18 of those also touch
`src/renderer.rs`. That is the empirical case for splitting it.

### 2.3 Deep modules, information leakage, and errors (opinion, widely used)

Ousterhout defines complexity as anything that makes a system hard to understand and modify, and
attributes it to dependencies and obscurity. A deep module "provides a lot of functionality but has a
very simple interface"; a shallow module pushes complexity upward. Information leakage occurs when
one design decision is reflected in multiple modules
([Ousterhout, *A Philosophy of Software Design*](https://web.stanford.edu/~ouster/cgi-bin/book.php)).
His error advice, "define errors out of existence", means designing interfaces so the exceptional case
is not an exception at all.

**Operational rules:**

- `section` exposes `get`, `set`, and `baseline_hash`, not its palette and word layout. A caller that
  needs `Section::words()` to do its job is a leak.
- `TerrainDemo` should expose `block_at`, `is_solid`, `edit`, and `tick`, not 30 fields that every
  free function in `lib.rs` reaches into. Privacy of fields is not depth if the same file owns all
  the readers.
- Out-of-world reads already return bedrock or air rather than an error (spec §4.3). Keep that
  pattern: `open_or_create` folds "missing" into "create", and `block_at` returns `Option` only where
  "outside the patch" is a real domain answer.
- Prefer a total function at the domain edge and a typed error at the adapter edge. Do not return
  `Result<_, String>` from a helper to avoid choosing which layer owns the failure.

### 2.4 The dependency rule, without the layer cake (direction is consensus; layer count is opinion)

Martin's Clean Architecture states one durable rule: source-code dependencies point inward, and an
inner circle must not name anything from an outer circle
([Martin, 2012](https://blog.cleancoder.com/uncle-bob/2012/08/13/the-clean-architecture.html)). The
four circles are explicitly schematic: Martin says "you may find that you need more than just these
four" (same page). The useful part is the direction, not the taxonomy.

The critiques matter because this repository is small enough that ceremony would dominate:

- Ousterhout's exchange with Martin rejects the "2-4 line method" rule as producing shallow,
  entangled methods: the advice "encourages programmers to create teeny-tiny methods that suffer from
  both shallow interfaces and entanglement"
  ([Ousterhout and Martin, 2024-2025](https://github.com/johnousterhout/aposd-vs-clean-code)).
- Cockburn says the number of ports is "largely a matter of taste" and that there is "no particular
  damage in choosing the wrong number of ports"
  ([Cockburn, 2005](https://alistair.cockburn.us/hexagonal-architecture/)). That is a warning against
  turning every dependency into a trait.
- Fowler's YAGNI and Rule of Three say the second implementation, not the first guess, is the signal
  to abstract ([YAGNI](https://martinfowler.com/bliki/Yagni.html),
  [Rule of Three](https://martinfowler.com/bliki/RuleOfThree.html)).
- Metz's "wrong abstraction" describes the maintenance cost of keeping one: duplication is often
  cheaper than an abstraction that has to serve two incompatible callers
  ([Metz, 2016](https://sandimetz.com/blog/2016/1/20/the-wrong-abstraction)).

SOLID is not equally useful here. The single-responsibility principle maps cleanly to "one hidden
decision". The open-closed, interface-segregation, and dependency-inversion principles are already
satisfied by module privacy and the crate boundary; applying them again as traits would duplicate the
boundary. The Liskov principle is irrelevant without inheritance. That is the selective reading this
plan uses: SRP as a design question, the rest as tools only when a second implementation exists.

**Operational rule:** keep the inward dependency rule (`terrain-core` never knows about `engine`, and
never names `wgpu` or `winit`), drop the layer folders, and require a second real implementation
before adding a trait. `MeshPool` qualifies: it has a micropool and a rayon backend. `Renderer` does
not: there is one `wgpu` renderer.

### 2.5 Ports and adapters: use the ones that already exist (pattern is proven; port count is opinion)

Cockburn's motivation is testability and headless operation: an application should run "without
either a UI or a database" and be driven by a test harness
([Cockburn, 2005](https://alistair.cockburn.us/hexagonal-architecture/)). This repository already has
that shape:

- `terrain-core` is the inside: no window, GPU, audio, or renderer dependency.
- `engine` is the windowed adapter.
- `terrain-gen` and `ENGINE_HEADLESS=1` are the headless adapters.
- The save layer is the file-system adapter.

**Operational rule:** do not add a `Renderer` trait, a `WorldStore` trait, or a `Clock` trait. A port
helps when a second adapter is real; it adds ceremony when the second adapter is hypothetical. The
crate boundary is the port. Keep `headless` as an adapter of the app loop, and keep `terrain-core`
free of `winit`, `wgpu`, `tracy-client`, and `wgpu-profiler`; CI enforces that boundary with
`cargo tree` ([testing](../dev/testing.md)).

Screaming architecture adds one useful check: the top level should say what the system *is*
([Martin, 2011](https://blog.cleancoder.com/uncle-bob/2011/09/30/Screaming-Architecture.html)). Today
the top level says `world`, `mesh`, `light`, `generation`, `streaming`, and `renderer`, which is a
voxel engine. After this plan it should also say `demo` and `app`, because the v1 terrain demo is a
first-class use case rather than a branch inside `run`.

### 2.6 Rust-specific rules (tool-enforced consensus)

| Source | Rule this repository should enforce |
|---|---|
| [Cargo workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html) | Keep members flat under `crates/`; `default-members` decides what a bare `cargo build` builds. This workspace already lists both members, so a root build compiles the engine and the headless crate. |
| [Rust Reference: visibility](https://doc.rust-lang.org/reference/visibility-and-privacy.html) | Private by default; `pub(crate)` inside a crate; `pub` only at the crate boundary. A `pub use` can short-circuit privacy, so re-exports are API decisions, not conveniences. |
| [Effective Rust, Item 22](https://www.lurklurk.org/effective-rust/visibility.html) | "As little as possible"; visibility changes are hard to undo. `pub` is a compatibility commitment even when `publish = false`, because in-repo consumers and docs depend on it. |
| [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/checklist.html) | Casing, `as_`/`to_`/`into_`, getter names; types are nouns and booleans read as assertions. Private fields, meaningful error types, `Debug` on public types. A public struct with public fields pins its representation forever. |
| [Cargo features](https://doc.rust-lang.org/cargo/reference/features.html) | Features are additive and must not change the public API in a non-additive way. A feature that changes save bytes or generator output is a design error. |
| [Orphan rules](https://doc.rust-lang.org/reference/items/implementations.html#trait-implementation-coherence) | You cannot implement a foreign trait for a foreign type. Keep third-party types (`wgpu`, `winit`) out of public signatures, or expect a newtype at every boundary. |
| [Fast Rust builds](https://matklad.github.io/2021/09/04/fast-rust-builds.html) | The shape of the crate graph matters more than the number of crates; generics at crate boundaries duplicate monomorphizations. Split a crate for a real boundary, not for a line count. |
| [The Rust Book, ch. 7.2](https://doc.rust-lang.org/book/ch07-02-defining-modules-to-control-scope-and-privacy.html) | `mod foo;` (with the semicolon) declares a file module and keeps `lib.rs` a table of contents. New submodules can use `foo.rs` plus `foo/bar.rs`; `foo/mod.rs` is also valid, and existing directories should not be churned just to change style. |
| [Clippy module lints](https://rust-lang.github.io/rust-clippy/master/index.html#mod_module_files) | `mod_module_files` and `self_named_module_files` are style lints, not correctness lints. Pick one convention per tree and do not mix it mid-directory. |

**Operational rule for `lib.rs`:** it should contain crate docs, `mod` declarations, and `pub use`.
No frame loop, no demo patch, no tests. The current file is 5,581 lines because it is the crate root,
the app, the demo, and the demo test harness at once. Effective Rust's Item 25 makes the dependency
side explicit: fewer, better-understood dependencies beat a large graph that nobody can hold in their
head ([Effective Rust, Item 25](https://www.lurklurk.org/effective-rust/dep-graph.html)).

### 2.7 Empirical evidence is real but narrow

- Koru, Zhang, El Emam, and Liu studied four large object-oriented products and found that "defect
  proneness increased with module size but at a smaller rate. Therefore, smaller modules were
  proportionally more defect prone"
  ([Koru et al., 2009](https://ieeexplore.ieee.org/document/4693715)). This is the opposite of a
  per-line "large files are 10x buggier" rule. Large modules have more defects in absolute terms, but
  the case for splitting is comprehension, review, and change locality, not a defect multiplier.
- Relative code churn predicts defect density better than absolute size
  ([Nagappan and Ball, 2005](https://www.microsoft.com/en-us/research/publication/use-of-relative-code-churn-measures-to-predict-system-defect-density/)).
  A file that changes constantly is a better warning sign than a file that is merely long.
- Large file size co-occurs with code smells and high dependency, so size alone is a weak causal
  claim ([An Empirical Study of the Impact of Bad Designs on Defect Proneness](https://ieeexplore.ieee.org/document/8118515)).
- Change coupling from release history finds dependencies that static imports miss
  ([Gall et al., 1998](https://ieeexplore.ieee.org/document/738508)).

The honest conclusion: file and function budgets in this plan are **review heuristics with a measured
baseline**, not natural laws. That is why §5.6 states the number, the current distribution, and the
exception process instead of claiming a defect threshold.

### 2.8 What we explicitly reject

- **A folder per layer** (`domain/`, `application/`, `infrastructure/`, `presentation/`) with a trait
  per layer. There is one renderer, one window, and one save format. The dependency rule is worth
  keeping; the layer cake is not.
- **A trait for every dependency.** `MeshPool` earns its trait with two backends; `Renderer` does
  not. A trait with one implementation is a shallow module that makes the call graph harder to read.
- **One type per file.** It multiplies module boilerplate without hiding a decision.
- **`mod.rs` everywhere.** New submodules should use `foo.rs` plus `foo/bar.rs`. Existing
  `features/mod.rs`, `graph/mod.rs`, and `hydro/mod.rs` stay; churning them is not a refactor.
- **A crate per subsystem.** The two-crate graph already isolates the headless stack and lets the
  renderer change without rebuilding `terrain-core`. Splitting `terrain-core` into ten crates would
  add public surfaces and monomorphization boundaries without a second consumer.
- **`common` or `utils`.** It is the absence of a decision, not a module.
- **`pub` for tests.** Tests live beside the code and can see private items; widening the surface to
  test it is a leak.
- **Rewriting the v0 and v1 stacks into one "clean" stack.** That is a behaviour change and a
  milestone, not a structure change. This plan makes the seam explicit instead.

## 3. The rubric and the current score

Scale: **4** enforced by a mechanism, **3** consistent with a documented exception, **2** mostly but
with a real counterexample, **1** ad hoc, **0** absent or opposite.

| # | Criterion | Check | Score | Evidence |
|---:|---|---|---:|---|
| 1 | Headless crate boundary | `terrain-core` normal deps exclude `winit`, `wgpu`, `tracy-client`, `wgpu-profiler` | 4 | Its only normal dependencies are `fastnoise-lite` and `thiserror`; CI runs `cargo tree` and fails on a leak ([testing](../dev/testing.md)). |
| 2 | Dependency direction inside `terrain-core` | A stage never imports `pipeline`, `save`, or `world_save`; only `pipeline` imports all stages | 3 | `pipeline` imports every generation stage; no stage imports it in code. `metrics2 -> pipeline` exists only in a doc comment. |
| 3 | Cycle freedom | The module graph has no back edge | 1 | `section <-> content` (`BlockId` and `ContentPack`); `light <-> mesh` (`ChunkSnapshot` and `LightField`); crate root `<-> headless` (`crate::App`). |
| 4 | Public surface | `pub` only at a crate boundary; `pub(crate)` inside | 1 | `terrain-core`: 927 `pub` items, 29 `pub(crate)`. `engine`: 9 `pub`, 350 `pub(crate)`, but it re-exports 28 terrain modules, while [public-api](../dev/public-api.md) lists 5 public items and says there are no public modules. |
| 5 | Module depth and ownership | One module owns each state; callers use an interface smaller than the body | 2 | `pipeline` is deep (29 `pub` items hiding every stage). `src/lib.rs` owns startup, patch, ring, edits, saves, repair, the app loop, and the demo tests; `App` has 39 fields spanning both stacks. |
| 6 | File budget | No new file over 1,000 lines; exceptions listed | 1 | 18 of 58 files exceed 1,000; 4 exceed 2,000; the maximum is 5,581. |
| 7 | Function budget | Functions under 100 lines except documented dispatch/table functions | 3 | 18 of 2,462 functions exceed 100 lines (0.7%); median 10, p90 37. 10 `#[allow(clippy::too_many_lines)]` sites remain, 8 in `src/lib.rs`. |
| 8 | Error boundary | One typed error per boundary; no panics on disk or user data | 3 | `TerrainError`, `SaveError`, `RendererError`, and `ConfigError` are typed and `thiserror`-derived. `RunError` aggregates six, and exposes `winit`, `wgpu`, and `io` types directly. |
| 9 | Test seams | Tests beside code; the default suite needs no GPU or window | 2 | 406 `#[test]` attributes in `terrain-core`, 260 in `engine`; 1,351 integration-test lines. But 1,299 test lines are trapped in `src/lib.rs`, and `App::new` is `#[cfg(test)]`, so demo tests construct the app. |
| 10 | Feature isolation | A feature gates one subsystem and changes no output bytes | 3 | `rayon-pool` appears only in `src/mesher.rs` (4 sites). `tracy` appears in `metrics.rs`, `renderer.rs`, and the `run` entry. `default = ["tracy"]` makes a local default build compile a C++ client, so CI must pass `--no-default-features`. |
| 11 | Change locality | A feature touches one subsystem, not the crate root | 1 | `src/lib.rs` changed in 36 of 97 commits and co-changed with `src/renderer.rs` in 18 of them. |
| 12 | Build isolation | A renderer change does not rebuild `terrain-core` | 4 | Two crates, one direction; `terrain-core` compiles without GPU or window dependencies. |

**Total: 28/48.** The strong parts are the crate boundary, the build isolation, the function-size
discipline, and the typed errors. The weak parts are the crate root, the two cycles in the engine, the
`section`/`content` cycle, and a public surface that is almost the whole headless crate.

### 3.1 Dependency edges and cycles

Measured from `crate::` paths in all 58 files:

```text
terrain-core:
  section -> content, description, error, hash, header
  content -> error, hash, section          # cycle: section <-> content
  pipeline -> climate, content, curve, description, error, features, graph,
              hash, height, hydro, lattice, noise, section, strata, surface, volume
  mesher2 -> description, lod, padded, revisions, section
  world_save -> content, description, editlog, error, hash, header, save, section

engine:
  light -> mesh::ChunkSnapshot             # cycle: light <-> mesh
  mesh -> light::LightField
  headless -> crate::App                   # cycle: root <-> headless
  renderer -> mesher2::Quad (via the engine re-export)
```

The `light <-> mesh` cycle is a data-structure placement problem: `ChunkSnapshot` belongs with the
chunk data in `world`, not in `mesh`. The `section <-> content` cycle is a type-placement problem:
`BlockId` is a content concept, but it lives in `section`. Both are small fixes with large effects on
the graph.

### 3.2 The 20 largest Rust files

These counts are the plan snapshot (commit `48ad37f`). The live check is
`scripts/size-budgets.sh`; at `b98c30e` it reports **22 files over 1,000 lines** (29 over 600) and
`src/lib.rs` at **6,900** lines, because the ring, LOD2, and camera work landed after this table was
measured. Treat the ranking, not the exact numbers, as the durable part.

| Lines | File | Role |
|---:|---|---|
| 5,581 | `src/lib.rs` | crate root, startup, demo patch/ring/save/repair, app loop, demo tests |
| 4,366 | `crates/terrain-core/src/terrain_gen.rs` | acceptance CLI: gen, golden, seams, sweep, calibrate, verify |
| 2,497 | `crates/terrain-core/src/mesher2.rs` | greedy section mesher, water, LOD, material table |
| 2,367 | `src/renderer.rs` | `wgpu` device, pipelines, buffers, draws, uploads |
| 1,942 | `crates/terrain-core/src/light2.rs` | column light, BFS, incremental repair |
| 1,898 | `crates/terrain-core/src/volume.rs` | detail band, caves, spans, ore placement |
| 1,846 | `crates/terrain-core/src/pipeline.rs` | fixed-order column pipeline, world hydro handle |
| 1,783 | `crates/terrain-core/src/features/mod.rs` | feature discovery and apply order |
| 1,625 | `crates/terrain-core/src/description.rs` | description schema, parser, validation |
| 1,511 | `crates/terrain-core/src/surface.rs` | surface rules, sea floor, precedence |
| 1,504 | `crates/terrain-core/src/section.rs` | section storage, codec, baseline hash |
| 1,431 | `crates/terrain-core/src/climate.rs` | temperature, humidity, biome selection |
| 1,395 | `src/world.rs` | v0 chunk map, VCHK save/load, seed |
| 1,386 | `crates/terrain-core/src/editlog.rs` | edit journal, replay, trim |
| 1,143 | `crates/terrain-core/src/residency.rs` | residency policy, ring budgets |
| 1,126 | `crates/terrain-core/tests/terrain_gen.rs` | integration harness for the CLI |
| 1,094 | `src/mesh.rs` | v0 greedy mesher, LOD downsample |
| 1,094 | `crates/terrain-core/src/world_save.rs` | world save, backup, repair, migration |
| 1,076 | `crates/terrain-core/src/content.rs` | content pack, stable names, ids |
| 988 | `crates/terrain-core/src/hydro/mod.rs` | hydrology tiles, flood, flow, lakes |

The median file is 556 lines; p90 is 1,898. A 1,000-line budget is therefore roughly 1.8x the median,
not an arbitrary "clean code" number.

### 3.3 Largest functions

| Lines | Function | File |
|---:|---|---|
| 330 | `verify_json` | `terrain_gen.rs` |
| 188 | `poll_ring` | `src/lib.rs` |
| 169 | `Renderer::new` | `src/renderer.rs` |
| 151 | `value` | `terrain_gen.rs` |
| 148 | `run` | `src/lib.rs` |
| 142 | `every_exposed_face_is_covered_once` | `src/lib.rs` (test) |
| 140 | `build_terrain_demo_extent` | `src/lib.rs` (test helper) |
| 128 | `replay_saved_sections` | `src/lib.rs` |
| 122 | `default_table` | `mesher2.rs` |
| 118 | `demo_rebuild` | `src/lib.rs` |

Function size is mostly healthy: the median is 10 lines and p90 is 37. The exceptions are
concentrated in the same places as the file-size problem, which suggests the file split and the
function split are the same job.

### 3.4 Change coupling

From `git log --name-only` over the last 97 commits (the whole available history):

- `src/lib.rs` changed 36 times, more than any source file.
- When it changed, `src/renderer.rs` changed too 18 times, `src/world.rs` 13 times, and
  `src/mesher.rs` 11 times.
- `CHANGELOG.md` changed 45 times, but documentation churn is expected and is not a design signal.

This is the strongest repo-specific evidence in the page. `src/lib.rs` is not merely long; it is the
place where independent changes meet. Splitting it is not cosmetic.

## 4. Target structure

### 4.1 Crate graph

```text
engine (root package: lib + bin `engine`)
├── terrain-core (lib + bin `terrain-gen`)
│   ├── fastnoise-lite
│   └── thiserror
├── wgpu, winit, wgpu-profiler, tracy-client
├── glam, bytemuck, micropool, profiling, tracing
└── (optional, not now) terrain-demo
```

`terrain-core` stays headless and feature-free. `engine` owns every window, GPU, and file-system
adapter. The optional `terrain-demo` crate is a trigger-gated future step (§7), not part of this
milestone: the demo module boundary is worth proving inside `engine` before paying for a new public
surface.

### 4.2 Engine tree

```text
src/
├── main.rs                 # thin shell: calls engine::run()
├── lib.rs                  # crate docs, mod list, pub use; no logic
├── startup.rs              # run(), config, seed, world choice, env switches
├── app.rs                  # App, ApplicationHandler, frame loop, renderer upload adapter
├── app/
│   ├── input.rs            # key/mouse mapping, capture, hotbar
│   └── frame.rs            # FrameAccumulator, per-second stats
├── demo/                   # v1 terrain demo; no winit, no wgpu
│   ├── mod.rs              # DemoRuntime: patch + ring + save + repair
│   ├── patch.rs            # TerrainDemo: resident columns, block access, spawn
│   ├── build.rs            # incremental light + mesh rebuild
│   ├── lod.rs              # LOD0/1/2 selection, coarse and far sets
│   ├── stream.rs           # RingStream, ColumnPool, generation/build jobs
│   ├── edit.rs             # edit queue, rebuild scheduling, upload throttle
│   ├── persist.rs          # save worker, journal replay, repair prompt, read-only
│   └── content.rs          # material table, solid table, texture layer map
├── v0/                     # frozen legacy stack; removed when the demo is default
│   ├── mod.rs              # docs + module list
│   ├── world.rs, light.rs, mesh.rs, mesher.rs
│   ├── generation.rs, streaming.rs, voxel.rs
├── renderer.rs
├── camera.rs, player.rs, raycast.rs, sky.rs, overlay.rs, frustum.rs
├── block.rs, config.rs, headless.rs, metrics.rs
```

The `v0/` grouping is **decided** (Section 8): the seven modules move under `src/v0/` in an early,
behaviour-preserving stage while they are still the default run path. The boundary is naming plus a
single `v0/mod.rs`, not a new crate, so the move is cheap and does not change behaviour.

### 4.3 `terrain-core` tree

The spec pins the flat top-level module names (`light2`, `mesher2`, `pipeline`, `section`, and the
rest) in §0.1, item 5. Keep every path. Split the *insides* of the large files into submodules; the
parent path does not change.

```text
crates/terrain-core/
├── Cargo.toml
├── src/
│   ├── lib.rs                     # 28 pub mod declarations; crate docs
│   ├── error.rs, hash.rs, header.rs
│   ├── description.rs             # -> description/{schema,parse,hash}.rs
│   ├── content.rs, section.rs     # section -> section/{storage,codec,baseline}.rs
│   ├── curve.rs, noise.rs, lattice.rs, graph/
│   ├── climate.rs, height.rs, strata.rs
│   ├── surface.rs                 # -> surface/{rules,seafloor}.rs
│   ├── volume.rs                  # -> volume/{detail,caves,spans,ores}.rs
│   ├── features/mod.rs            # -> features/{discover,apply,masks}.rs
│   ├── hydro/{mod,flood,flow,lake}.rs
│   ├── pipeline.rs                # -> pipeline/{terrain,stages,hydro}.rs
│   ├── light2.rs                  # -> light2/{column,bfs,repair,table}.rs
│   ├── padded.rs, lod.rs, revisions.rs, residency.rs, metrics2.rs
│   ├── mesher2.rs                 # -> mesher2/{greedy,water,cutout,lod,skirt,material}.rs
│   ├── editlog.rs, save.rs, world_save.rs
│   └── bin/terrain-gen/           # was terrain_gen.rs
│       ├── main.rs, gen.rs, golden.rs, seams.rs, sweep.rs, calibrate.rs, verify.rs
└── tests/
    ├── blend.rs
    ├── terrain_gen.rs
    └── golden/
```

For a single-file module, use `mesher2.rs` plus `mesher2/` (no `mod.rs`). For directories that already
exist (`features/`, `graph/`, `hydro/`), keep `mod.rs`; the cost of moving is larger than the style
gain.

### 4.4 Ownership: owns, hides, exposes

| Module | Owns | Hides | Exposes |
|---|---|---|---|
| `startup` | config, seed, world choice, environment switches | file layout, seed file, stress mode, demo mode | `run() -> Result<(), RunError>` |
| `app` | window, renderer, frame order, input state | `winit`/`wgpu` lifecycle, redraw policy, cursor capture | `App` (crate), `upload_terrain_demo` |
| `demo` | patch, ring, save worker, repair prompt, pending rebuild | column maps, job structs, revisions, throttles, read-only policy | `DemoRuntime::open/tick/edit/save/repair/stats` |
| `demo::patch` | resident columns, light, baselines, spawn | `Arc` sharing, map keys, section addressing | `block_at`, `is_solid`, `column_ready` |
| `demo::build` | incremental rebuild algorithm | 3x3 light scope, 26-neighbour remesh, material resolution | `demo_rebuild(...) -> DemoRebuild` |
| `demo::lod` | active section set and LOD bands | band maths, 2x2/4x4 grouping, hysteresis | `select_lod`, `sections()` |
| `demo::stream` | ring jobs and workers | pool, snapshots, in-flight set, cancellation | `RingStream::new/tick` |
| `demo::edit` | edit queue and rebuild scheduling | coalescing, throttle, upload budget | `edit`, `poll_rebuild` |
| `demo::persist` | save worker, replay, repair | journal order, baseline check, read-only rule | `save`, `repair`, `read_only` |
| `renderer` | `wgpu` resources | pipelines, layouts, texture array, depth | `render`, `upload_*` |
| `v0::world` | chunk map and VCHK | file format, light cache, seed file | `World`, `ChunkCoord`, `SaveError` |
| `v0::light` | light BFS and padded field | queue algorithm, nibble packing | `LightField` |
| `v0::mesh` | greedy mesher | vertex packing, LOD downsample | `build_mesh`, `MeshData` |
| `terrain-core` foundation | `error`, `hash`, `description`, `header`, `content`, `section` | error taxonomy, SHA-256, schema, header bytes, stable ids, section storage | the curated types behind the 28 module paths |
| `terrain-core` generation | `curve`, `noise`, `lattice`, `graph`, `climate`, `height`, `strata`, `surface`, `volume`, `hydro`, `features` | each stage's maths and intermediate data | stage functions consumed by `pipeline` |
| `terrain-core` orchestration | `pipeline` | stage order, shared `WorldHydro`, caches | `Terrain`, `Terrain::section` |
| `terrain-core` derived | `light2`, `lod`, `mesher2`, `revisions`, `residency`, `metrics2` | light, LOD, mesh, revision, budget policy | typed results and counters |
| `terrain-core` persistence | `editlog`, `save`, `world_save` | journal, section files, backup, repair, migration | `WorldSave`, `SaveWorker`, `RepairAction` |

### 4.5 Where the demo's responsibilities go

`src/lib.rs` currently holds the demo's five responsibilities plus startup and the app loop in one
file. The split is by hidden decision, not by line range:

| Responsibility | Today | Target | Why it moves |
|---|---|---|---|
| Startup and world choice | lines 109-432 | `startup.rs` | It is the only code that reads env vars and the seed file; the app should not know how a world is chosen. |
| Patch state and block access | lines 434-957 | `demo/patch.rs` | It owns the resident columns, baselines, spawn measurement, and readiness. |
| Incremental light and mesh | lines 981-1125, 1440-1939 | `demo/build.rs` | Pure work over column maps; it is the easiest part to test headlessly. |
| LOD selection | lines 845-896, 1601-1869 | `demo/lod.rs` | It owns the band maths and the active section set. |
| Ring streaming and workers | lines 1976-2444 | `demo/stream.rs` | It owns job lifetime, snapshots, and the worker pool. |
| Edits and rebuild scheduling | lines 2546-2585 plus App methods 3229-3385 | `demo/edit.rs` | It owns coalescing and throttles, which are policy, not frame loop. |
| Save, replay, repair | lines 2586-2760 plus App methods 3145-3208 | `demo/persist.rs` | It owns journal replay, baseline checks, read-only mode, and the repair prompt. |
| Renderer upload | lines 1960-1975 | `app.rs` | It is the only demo code that names `Renderer`; keeping it in the app preserves the one-way edge. |
| App loop and input | lines 2762-4254 | `app.rs`, `app/input.rs`, `app/frame.rs` | It owns `winit` and `wgpu`, which nothing in `demo` may see. |
| Demo orchestration | `App` fields 2770-2780 | `demo/mod.rs` | One `DemoRuntime` field replaces six fields and their invariants. |

### 4.6 The `DemoRuntime` seam

The interface is deliberately small. It is a sketch of names and ownership, not code to land in this
milestone.

```rust
// src/demo/mod.rs
pub(crate) struct DemoRuntime {
    // patch, ring, save worker, repair prompt, pending rebuild
}

impl DemoRuntime {
    pub(crate) fn open(config: DemoConfig) -> Result<(Self, DemoReport), DemoError>;
    pub(crate) fn tick(&mut self, now: Instant) -> DemoFrame;
    pub(crate) fn edit(&mut self, cell: IVec3, id: BlockId) -> bool;
    pub(crate) fn save(&mut self);
    pub(crate) fn repair(&mut self, action: save::RepairAction) -> bool;
    pub(crate) fn is_ready(&self, position: Vec3) -> bool;
    pub(crate) fn sections(&self) -> &[DemoSection];
}
```

`DemoFrame` carries the sections to upload, the new LOD set, and the counters. `App` translates that
into renderer calls; `demo` never names `Renderer`. This is the "deep module" move: `App` sees one
interface instead of six fields and their ordering rules.

Because `demo::lod`, `demo::build`, and the other child modules are descendants of `demo`, they can
`impl TerrainDemo` and read its private fields without making those fields `pub(crate)`. That is the
Rust privacy rule the split relies on.

## 5. Rules the structure enforces

### 5.1 Allowed dependency edges

| From | May use | Must not use |
|---|---|---|
| `startup` | `app`, `headless`, `demo`, `terrain_core`, `config` | `renderer` directly, `v0` internals |
| `app` | `demo`, `renderer`, `camera`, `player`, `raycast`, `sky`, `overlay`, `v0`, `terrain_core` | `startup` |
| `demo` | `terrain_core`, `player::BlockSource`, `block` | `app`, `renderer`, `winit`, `wgpu`, `v0::world` |
| `renderer` | `camera`, `frustum`, `v0::mesh`, `v0::voxel`, `overlay`, `sky`, `terrain_core::mesher2` | `app`, `demo`, `startup` |
| `v0::light` | `v0::world` | `v0::mesh` |
| `v0::mesh` | `v0::world`, `v0::light` | `v0::mesher` |
| `terrain_core` stage | foundation modules and its own data | `pipeline`, `save`, `world_save` |
| `terrain_core` persistence | foundation modules | `pipeline` and the stages |
| `pipeline` | every stage, foundation, `metrics2` | `world_save`, `save` |

The graph is a DAG. A stage module is a leaf with respect to orchestration: it may call
`description`, `hash`, `error`, `section`, `content`, `curve`, `noise`, `lattice`, and its sibling
stage if the spec says so, but it may not call `Terrain::new` or open a world. That is what makes
`pipeline` the only composition point.

A CI check can build this edge list from `crate::` paths and fail on a back edge. It is not part of
this proposal, but it is the natural enforcement once the first splits land.

### 5.2 Visibility policy

- `pub` only in `lib.rs` for `run`, `RunError`, and (for now) the terrain re-exports. `terrain-core`
  keeps `pub` items for `terrain-gen` and the engine; everything else becomes `pub(crate)`.
- `pub(crate)` for anything crossing a module boundary inside a crate.
- `pub(super)` when a child module must expose a type to its parent only (for example
  `demo::build::DemoRebuild`). It is narrower than `pub(crate)` and should be preferred when the
  parent is the only consumer.
- Private by default. Fields stay private; constructors validate. `ChunkCoord`, `SectionKey`, and
  `ColumnKey` are newtypes that keep the three coordinate spaces apart; `BlockId` keeps block
  identity out of a raw `u16`.
- No `pub use` inside a crate, except a deliberate compatibility re-export at a module boundary (the
  `BlockId` move in Stage 2 is one). Use `pub(crate) use` for internal re-exports; a `pub use` is an
  API promise.
- Do not expose a third-party type in a public signature without deciding to re-export it
  ([Effective Rust, Item 24](https://www.lurklurk.org/effective-rust/re-export.html)). `RunError`
  currently exposes `winit::error::EventLoopError`, `wgpu::RequestAdapterError`, and `std::io::Error`;
  that is a known coupling, recorded in [public-api](../dev/public-api.md), and is a separate
  decision from this plan.

### 5.3 Errors at boundaries

| Boundary | Error | Translation |
|---|---|---|
| `terrain-core` generation and storage | `error::TerrainError` | none; this is the domain error |
| `terrain-core` section files and journal | `error::TerrainError` | carries IO, format, and corruption faults through `ErrorKind` |
| `terrain-core` world open | `error::TerrainError` | folds "missing" into create; seed and generator-version mismatches stay errors |
| `demo` | `DemoError` | wraps `TerrainError` and `std::io::Error` |
| `renderer` | `RendererError` | wraps `wgpu` and shader validation |
| `app`/`startup` | `RunError` | wraps `DemoError`, `RendererError`, the v0 `SaveError`, `ConfigError`, and `winit` |

Rules:

- Translate once, at the boundary that owns the recovery policy. Do not convert a typed error to
  `String` and back.
- `define errors out of existence` where the domain has a total answer: out-of-world block reads,
  missing save directory, empty mesh. Keep the error for a failed disk write or a corrupt header.
- No `unwrap`, `expect`, or indexing on data from disk or user input. `expect` is only for static
  invariants, with the invariant in the message.
- Logging is not error handling. A `warn!` before returning a degraded result is fine; a `warn!`
  instead of a `Result` is not.

### 5.4 Feature gates

- `tracy`: profiling only. `#[cfg(feature = "tracy")]` lives in `metrics.rs`, `renderer.rs`, and one
  startup block. It must not appear in `demo`, `v0`, or `terrain-core`.
- `rayon-pool`: v0 mesh pool only. If `v0/` is removed, the feature and the `rayon` dependency go
  with it.
- No feature may change generation output, save bytes, or the meaning of a revision. A feature that
  does is a separate build, not a flag.
- CI runs `--no-default-features` today because `tracy` is default and builds a C++ client
  ([testing](../dev/testing.md)). Keep that gate, and add `--all-features` once the demo split lands
  so both pool backends compile.
- `terrain-core` stays feature-free. A headless crate with conditional compilation is harder to
  reproduce and harder to golden-test.

### 5.5 Test placement per tier

| Tier | Where | What it may touch |
|---|---|---|
| T1 unit | `#[cfg(test)] mod tests` at the bottom of the file under test | private items in that module and its descendants |
| T2 integration | `crates/terrain-core/tests/` | the public API only; goldens in `tests/golden/` |
| T3 engine integration | `tests/` | `engine`'s public API only; today that is `tests/light_preview.rs` |
| T4 headless | `ENGINE_HEADLESS=1` and `terrain-gen` commands | the real frame path, no window |
| T5 GPU | feature- or env-gated, never in the default suite | window, surface, pipelines |

Consequences of the target tree:

- Demo tests move from `src/lib.rs` into `demo/`, beside the code they test. `App::new` stops being a
  test constructor for demo behaviour.
- `build_terrain_demo_extent` and its fixture stay in `demo/` as `#[cfg(test)]` helpers; they do not
  need to be in the crate root.
- `terrain-core` goldens are byte contracts. A refactor must not change a golden. If a golden
  changes, the change is behaviour and needs its own review.

### 5.6 Size budgets

| Budget | Value | Justification | Exceptions |
|---|---:|---|---|
| Module file | warn at 800, split at 1,000 | The current median is 556 and p90 is 1,898. 1,000 is about 1.8x the median, and one review screen set is roughly 400-600 lines. | Existing files over 1,000 are listed in §3.2 and may not grow; split on the next substantive change. |
| Function | warn at 80, fail at 120 | Median 10, p90 37; Clippy's `too_many_lines` default is 100. Most real functions are far below. | Dispatch, table construction, and test fixtures, with the reason in a comment. |
| `impl` block | 600 | An `impl` is a module-level interface; 600 lines is the point where a reader loses the method list. | `App` and `TerrainDemo` during the migration only. |
| `pub` items per module | 25 | A module with more than 25 public items is usually several modules. | `description`, `section`, `mesher2`, and `light2` until the visibility pass. |
| Test module | no cap | Tests are data, not interface. | none |

The numbers are deliberately not a "defect threshold". Koru et al. found a sublinear size-defect
relationship, so a per-line defect argument would be false. The budget protects review and change
locality, which is what the change-coupling data shows is actually failing here.

### 5.7 Module file layout

- `lib.rs` is a table of contents: crate docs, `mod` declarations, `pub use`. It contains no `fn` body
  except a one-line re-export.
- `mod foo;` with the semicolon is the declaration form. Inline `mod foo { ... }` is reserved for
  `#[cfg(test)]`.
- New submodules use `foo.rs` plus `foo/bar.rs`. Existing `foo/mod.rs` directories stay.
- One module per file, one concept per module, and the file name predicts the contents. If a module
  needs a second noun in its name, it is probably two modules.
- Contractual names stay: `light2`, `mesher2`, `metrics2`, `pipeline`, `section`, `world_save`. The
  numeric suffix is ugly, but renaming it is a spec and doc change, not a refactor.

## 6. Staged migration

Every stage is one commit (or a short series of commits of the same kind), behaviour-preserving, with
a gate and a revert. File moves use `git mv` and no edits in the move commit, so `git log --follow`
and `git diff --color-moved` stay useful.

| Stage | Move | Size | Gate | Rollback | Value / risk |
|---|---|---|---:|---|---|
| 1 | `App` + handler + frame to `app.rs`; `run` + world choice to `startup.rs` | ~1,800 lines | `cargo fmt`, `clippy`, `test --no-default-features`, headless run | revert one commit | Removes the root/headless cycle and the 37% hotspot. Low risk. |
| 2 | Two type moves: `BlockId` to `content`; `ChunkSnapshot` to `world` (later `v0::world`); both parents re-export | ~50 lines | `cargo test -p terrain-core`, goldens, `cargo test` for the engine | revert one commit | Removes both data cycles: `section <-> content` and `light <-> mesh`. Low risk. |
| 3 | `demo/patch.rs`, `demo/build.rs`, `demo/lod.rs`, `demo/content.rs` | ~1,400 lines | unit tests move with the code; headless; manual demo | revert one commit | Makes the patch and LOD testable without `App`. Medium risk. |
| 4 | `demo/stream.rs`, `demo/edit.rs`, `demo/persist.rs`; introduce `DemoRuntime`; shrink `App` | ~1,200 lines | `cargo test`, `ENGINE_HEADLESS=1`, `TERRAIN_DEMO=1` manual | revert one commit | The interface win: one field replaces six. Medium risk. |
| 5 | Split `mesher2`, `light2`, `pipeline`, `volume`, `features/mod`, `description`, `surface`, `section`, `terrain_gen` | 9 files, ~19,000 lines reorganised | `cargo test -p terrain-core`, goldens byte-identical, `cargo doc` | revert per file | Reviewability; no behaviour. Low risk, high review cost. |
| 6 | Visibility and error pass: `pub(crate)` audit, re-export decision, `RunError` wrapping | ~927 `pub` items | `clippy` with `unreachable_pub`, `cargo doc`, integration tests | revert per module | Narrows the surface; API-visible. Medium risk. |
| 7 | Optional: `crates/terrain-demo` | ~5,000 lines | full gates plus a measured build-time check | revert | Only with a trigger (the demo must not link `wgpu`, a second frontend, or a measured build win). High cost. |

**Stage status (2026-11-20):** the `v0/` grouping from Stage 2 has landed. `world`, `light`, `mesh`,
`mesher`, `generation`, `streaming`, and `voxel` now live under `src/v0/`, and the crate root
re-exports them, so every existing module path is unchanged. The two type moves in Stage 2 have not
landed yet.

**Stage 4 status (2026-11-21):** landed. `DemoRuntime` owns the patch, ring, save worker, repair prompt, pending rebuild, and edit queue; `App` holds one `demo: Option<DemoRuntime>` and performs the renderer upload, and `demo` names no renderer, `winit`, or `wgpu`.

### 6.1 Stage detail

**Stage 1: split the root.**

- Move `App`, `impl App`, `impl ApplicationHandler for App`, and the key helpers to `app.rs`;
  `FrameAccumulator` to `app/frame.rs`; the input mapping to `app/input.rs`.
- Move `RunError` to `app.rs`, because `App` stores the error and returns it after the event loop.
- Move `run`, `WorldSetup`, `world_for`, `generated_setup`, `stress_requested`,
  `terrain_demo_requested`, `no_vsync_requested`, and `generated_seed` to `startup.rs`.
- `lib.rs` keeps the crate docs, `mod` declarations, `pub use app::RunError`, `pub use startup::run`,
  and nothing else.
- Update `headless.rs` to import `crate::app::App`. The edge becomes `startup -> headless -> app`,
  which is acyclic.
- Do not move the demo in this stage. The goal is to remove the root/headless cycle with one
  mechanical move.

Gate: compile, clippy, tests, `ENGINE_HEADLESS=1` for 600 frames, and a diff check that no function
body changed. Rollback: one revert.

**Stage 2: break both data cycles.**

- Move `BlockId` from `section.rs` to `content.rs`; add `pub use crate::content::BlockId;` in
  `section.rs` so every existing path still resolves.
- Delete `use crate::section::BlockId` from `content.rs`.
- Keep `SavedSection` and the codec in `section.rs`; they legitimately depend on `ContentPack`.
- Move `ChunkSnapshot` from `mesh.rs` to `world.rs`, beside `Chunk`; add a `pub(crate) use
  crate::world::ChunkSnapshot;` in `mesh.rs` for the transition. `light.rs` and `mesh.rs` then both
  depend on `world`, and neither depends on the other.
- Do not change the save bytes. `BlockId` is the same `u16`, and `ChunkSnapshot` is runtime-only.

Gate: `cargo test -p terrain-core`, the golden check, `cargo test` for the engine, and `cargo tree`
unchanged. Rollback: one revert.

**Stage 3: the pure demo core.**

- `demo/patch.rs`: `TerrainDemo`, `DemoSection`, `block_at`, `is_solid`, `is_empty`,
  `column_ready`, `aabb_ready`, `baseline_for`, `insert_column`, `remove_column`, `apply_light`,
  `apply_patches`, `settle_spawn`, `set_block`, `apply_saved`, `apply_rebuild`, `refresh_stats`,
  `footprint_bytes`.
- `demo/build.rs`: `DemoEdit`, `DemoPatch`, `DemoRebuild`, `demo_rebuild`, `demo_section`,
  `demo_section_light`, `demo_column_light`, `demo_fine_all`, `demo_fine_column`, `demo_coarse`,
  `demo_light_table`, and the `#[cfg(test)]` builders.
- `demo/lod.rs`: `select_lod`, `refresh_coarse_around`, `coarse_bands`, `demo_lod_sets`,
  `demo_coarse_regions`, `demo_far_regions`.
- `demo/content.rs`: `solid_table`, `demo_materials`, `demo_texture_layer`.

Keep the `impl TerrainDemo` blocks in the child modules; they are descendants of `demo`, so private
fields remain accessible. Move the tests with the functions. Do not introduce `DemoRuntime` yet; this
stage is a file split, not an interface change.

Gate: the tests that exercised the demo through `App` now exercise `demo` directly; headless run;
manual `TERRAIN_DEMO=1` smoke. Rollback: one revert.

**Stage 4: the runtime seam.**

- `demo/stream.rs`: `ColumnMap`, `RingAssets`, the job structs, `ColumnPool`, `generate_column`,
  `build_column`, `build_coarse`, the snapshot helpers, `RingStream`.
- `demo/edit.rs`: `queue_demo_edit`, the edit target calculation, the pending-rebuild state machine,
  and the upload throttle.
- `demo/persist.rs`: the save worker setup, `replay_saved_sections`, `ReplayOutcome`,
  `RepairPrompt`, `repair_action`, and `legacy_block_name`.
- `demo/mod.rs`: `DemoRuntime` owns `TerrainDemo`, `RingStream`, `Option<SaveWorker>`,
  `Option<RepairPrompt>`, and the pending rebuild. `App` keeps `demo: Option<DemoRuntime>`.
- `app.rs` keeps `upload_terrain_demo` as the renderer adapter.

This is the highest-value stage: it removes six fields from `App` and turns the demo's ordering rules
into one interface. It is also the stage most likely to conflict with concurrent work, so it should
land when the demo path is otherwise quiet.

Gate: full test suite, headless, manual demo open/edit/save/reopen/repair. Rollback: one revert.

**Stage 5: split the large files.**

Order by independence, not by size:

1. `mesher2.rs` (pure functions over sections; tests are local).
2. `light2.rs` (pure functions over columns; fuzz tests are local).
3. `volume.rs`, `surface.rs`, `climate.rs` (stage maths; goldens catch a mistake).
4. `description.rs` (parser/schema; round-trip tests are local).
5. `pipeline.rs` (orchestration; keep the public `Terrain` surface).
6. `features/mod.rs`, `section.rs`, `editlog.rs`.
7. `terrain_gen.rs` to `src/bin/terrain-gen/`, one module per command; remove the explicit `[[bin]]`
   path and let Cargo discover `src/bin/terrain-gen/main.rs`.

Each split keeps the parent module path. `mesher2.rs` becomes the module root; `mesher2/greedy.rs`
declares `use super::*` only if that stays readable, otherwise explicit imports. No function body
changes.

Gate per file: `cargo test -p terrain-core`, the relevant golden, and `cargo doc` for intra-doc links.
Rollback: revert that file's commit. Do not combine two file splits in one commit; the review is the
cost, not the edit.

**Stage 6: visibility and the public surface.**

- Audit `pub` in `terrain-core`: keep `pub` for items `terrain-gen` and `engine` use; make the rest
  `pub(crate)`.
- Decide the engine re-export policy. Option A: keep the 28 module re-exports and correct
  [public-api](../dev/public-api.md) to say so. Option B: publish one facade (`engine::terrain`) and
  move `tests/light_preview.rs` to `terrain_core` imports. Option B is cleaner; Option A is
  zero-risk. The owner must choose.
- Audit `RunError` and decide whether to wrap `winit`/`wgpu` errors. Wrapping narrows the API but
  costs a `source()` chain and a mapping table; it is not required for modularity.
- Add a CI check that `terrain-core` has no `pub` item outside the curated list, if the surface is
  narrowed.

Gate: `clippy` with `unreachable_pub`, `cargo doc`, integration tests, and a count of public items
before and after. Rollback: per module.

**Stage 7: optional last move.**

- `crates/terrain-demo`: only if one of the triggers in §7 fires. The module split must be stable for
  at least one milestone first, and the new crate must not depend on `renderer` or `winit`.
- `v0/` is not optional any more: it moves in Stage 2, behaviour-preserving, while it is still the
  default run path (Section 8).

### 6.2 Ordering rationale

Stage 1 first because it is the largest, most mechanical move and it removes the root/headless cycle
and the most-changed file's app responsibility in one review. Stage 2 second because it is two type
moves and removes both data cycles (the headless crate and the v0 light/mesh pair); leaving them while
doing larger work would waste the cheapest win. Stages 3-4 are the demo seam, in dependency order:
data, then policy, then the interface. Stage 5 is mechanical and can be interleaved with feature work.
Stage 6 is API-visible and needs the owner. Stage 7 is a different project.

## 7. Risks and trade-offs

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Merge conflicts with concurrent agents | High | Medium | One responsibility per commit; move-only commits; land Stage 1 when the app loop is quiet. |
| Tests trapped in `src/lib.rs` | High | Medium | Move each test with its function; keep `#[cfg(test)]` helpers in `demo/`; do not leave a test-only back door in `App`. |
| `headless` depends on `App` | Certain | Low | Update it in Stage 1; the resulting edge `startup -> headless -> app` is acyclic. |
| Renderer leaks into `demo` | Medium | High | Keep `upload_terrain_demo` in `app.rs`; a grep for `crate::renderer` under `src/demo/` must stay empty. |
| Public re-export breakage | Medium | Medium | Keep the 28 re-exports until Stage 6; `tests/light_preview.rs` is the canary. |
| Spec contract drift | Medium | High | Do not rename `light2`, `mesher2`, `pipeline`, `section`, or the stage names. Keep the flat top-level module paths. |
| Over-engineering | Medium | Medium | Stop at modules; no new crate, no `Renderer` trait, no layer folders. |
| Budget erosion | High | Low | Budgets are warnings first; the exception list is explicit and only shrinks. |
| Build-time regression | Low | Medium | File moves inside a crate do not change codegen. Measure before splitting a crate. |

### 7.1 Cost

- Stage 1: about 1-2 engineer-days including review.
- Stage 2: about half a day.
- Stages 3-4: about 4-5 engineer-days; this is where most of the design value lands.
- Stage 5: about 3-4 engineer-days, mostly review.
- Stage 6: about 2-3 engineer-days, depending on the re-export decision.
- Stage 7: about a week, and only with a trigger.

The first four stages (about one week) deliver most of the change-locality and test-seam win. Stages
5-6 can be spread across milestones; they are not prerequisites for the demo split.

### 7.2 Where modularity would be over-engineering

This workspace is 51,719 lines with one application consumer (`src/main.rs`), one acceptance binary
(`terrain-gen`), and one integration test (`tests/light_preview.rs`). That size does not justify:

- a crate per subsystem, or a `terrain-demo` crate before the module boundary is stable;
- a trait for `Renderer`, `WorldSave`, or the file system;
- a `domain/`, `application/`, `infrastructure/` folder tree;
- one type per file, or a `mod.rs` in every new directory;
- a generic event or command bus to decouple `App` from `demo`;
- an abstraction for the clock, when `Instant` is already passed in.

The right stopping point is: acyclic modules, one `DemoRuntime` seam, private fields, typed errors at
the boundaries, and a `lib.rs` that is a table of contents. That is a real improvement for this
project; more would be ceremony.

## 8. Decisions (2026-11-20)

The owner answered the open questions:

1. **v0 stays, grouped under `src/v0/` now.** It remains the default run path until the demo becomes
   the default, so the move is behaviour-preserving and the modules keep working. This moves ahead of
   the optional Stage 7.
2. **`engine` narrows to an `engine::terrain` facade.** The 28 module re-exports stop being public
   API; the facade re-exports the curated types `engine` actually uses, and `docs/dev/public-api.md`
   is corrected to match.
3. **`terrain-core` is an internal crate.** `publish = false`, no semver promise; `pub` is narrowed to
   what `engine` and `terrain-gen` need, and the missing-docs gate is not added.
4. **`terrain-gen` splits into `src/bin/terrain-gen/`** with one module per command, after the core
   moves, in one commit per subcommand.
5. **Size budgets are CI warnings, not failures.** A check reports offenders; it tightens into a hard
   gate once the current tree is under the limits.

Still open, and cheap to decide later: whether `BlockId` lives in `content` or a leaf `ids` module
(the plan prefers `content`), the trigger for a future `terrain-demo` crate, and whether `tracy`
stays a default feature.

## References

- D. L. Parnas, "On the Criteria To Be Used in Decomposing Systems into Modules", *Communications of
  the ACM* 15(12), 1972. <https://dl.acm.org/doi/10.1145/361598.361623> (digitized copy:
  <https://teaching.csse.uwa.edu.au/units/CITS4401/readings/parnasmodules.htm>).
- W. P. Stevens, G. J. Myers, and L. L. Constantine, "Structured Design", *IBM Systems Journal*
  13(2), 1974. <https://doi.org/10.1147/sj.132.0115>.
- R. C. Martin, "OO Design Quality Metrics: An Analysis of Dependencies", 1994.
  <https://linux.ime.usp.br/~joaomm/mac499/arquivos/referencias/oodmetrics.pdf>.
- R. C. Martin, "The Clean Architecture", 2012.
  <https://blog.cleancoder.com/uncle-bob/2012/08/13/the-clean-architecture.html>.
- R. C. Martin, "Screaming Architecture", 2011.
  <https://blog.cleancoder.com/uncle-bob/2011/09/30/Screaming-Architecture.html>.
- J. Ousterhout, *A Philosophy of Software Design*, 2nd edition, 2021.
  <https://web.stanford.edu/~ouster/cgi-bin/book.php>.
- J. Ousterhout and R. C. Martin, "A Philosophy of Software Design vs Clean Code", 2024-2025.
  <https://github.com/johnousterhout/aposd-vs-clean-code>.
- A. Cockburn, "Hexagonal Architecture (Ports and Adapters)", 2005.
  <https://alistair.cockburn.us/hexagonal-architecture/>.
- M. Fowler, "YAGNI". <https://martinfowler.com/bliki/Yagni.html>.
- M. Fowler, "Rule of Three". <https://martinfowler.com/bliki/RuleOfThree.html>.
- S. Metz, "The Wrong Abstraction", 2016.
  <https://sandimetz.com/blog/2016/1/20/the-wrong-abstraction>.
- The Cargo Book, "Workspaces".
  <https://doc.rust-lang.org/cargo/reference/workspaces.html>.
- The Cargo Book, "Features".
  <https://doc.rust-lang.org/cargo/reference/features.html>.
- The Rust Reference, "Visibility and privacy".
  <https://doc.rust-lang.org/reference/visibility-and-privacy.html>.
- The Rust Reference, "Trait implementation coherence" (orphan rules).
  <https://doc.rust-lang.org/reference/items/implementations.html#trait-implementation-coherence>.
- The Rust API Guidelines, checklist. <https://rust-lang.github.io/api-guidelines/checklist.html>.
- D. Drysdale, *Effective Rust*, Item 22 "Minimize visibility".
  <https://www.lurklurk.org/effective-rust/visibility.html>.
- D. Drysdale, *Effective Rust*, Item 25 "Manage your dependency graph".
  <https://www.lurklurk.org/effective-rust/dep-graph.html>.
- A. Kladov, "Fast Rust Builds", 2021.
  <https://matklad.github.io/2021/09/04/fast-rust-builds.html>.
- The Rust Book, "Defining Modules to Control Scope and Privacy".
  <https://doc.rust-lang.org/book/ch07-02-defining-modules-to-control-scope-and-privacy.html>.
- Clippy, `mod_module_files` and `self_named_module_files`.
  <https://rust-lang.github.io/rust-clippy/master/index.html#mod_module_files>.
- A. G. Koru, D. Zhang, K. El Emam, and H. Liu, "An Investigation into the Functional Form of the
  Size-Defect Relationship for Software Modules", *IEEE TSE* 35(2), 2009.
  <https://ieeexplore.ieee.org/document/4693715>.
- H. Gall, K. Hajek, and M. Jazayeri, "Detection of Logical Coupling Based on Product Release
  History", ICSM 1998. <https://ieeexplore.ieee.org/document/738508>.
- N. Nagappan and T. Ball, "Use of Relative Code Churn Measures to Predict System Defect Density",
  ICSE 2005.
  <https://www.microsoft.com/en-us/research/publication/use-of-relative-code-churn-measures-to-predict-system-defect-density/>.
- "An Empirical Study of the Impact of Bad Designs on Defect Proneness", 2017.
  <https://ieeexplore.ieee.org/document/8118515>.
