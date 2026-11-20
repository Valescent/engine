# Terrain spec

**Status:** Draft for implementation.

**Kind:** Normative spec. Where this page disagrees with a design page, this page wins. Where it
disagrees with the executable source, the source wins and this page is a bug.

**Derived from:** [Terrain interview](../design/terrain-interview.md). Every section cites the
question ids it implements. The interview is the decision record; this page is the contract.

## 0. Authority and supersessions

This spec supersedes the following parts of earlier pages. They remain true only where this page
does not speak.

| Superseded | Where | Now |
|---|---|---|
| World height 128, `SEA_LEVEL = 32` | v0.0.7 §2 | 384 tall, `SEA = 128` (§7.4) |
| `round()` in the height formula | v0.0.7 §4.2 | `f32` height, one `floor(h)` (§6.4) |
| Five fixed noise fields | v0.0.7 §4.1 | Declared field graph (§5) |
| Trees must fit one chunk | v0.0.7 §4.3 | Region discovery with `MAX_EXTENT` (§10) |
| Baseline + overlay + cache | terrain-pipeline §9 | Saved effective sections, edit log as audit (§16) |
| Two LOD levels, LOD1 derived per chunk | terrain-pipeline §7 | Three levels, section-derived (§13) |
| Conservative any-solid downsample | terrain-pipeline LOD table | Most-frequent-solid (§13.1) |
| `xxh3-128` / `[u8; 16]` identity hashes | §2.2 as first drafted | **SHA-256 in-tree / `[u8; 32]`**, pinned by `src/hash.rs` (§0.1) |

The words MUST, MUST NOT, SHOULD, and MAY are used as in the other specs. A MUST that cannot be met
is a spec bug, not a tuning opportunity.

### 0.1 Implementation pins that override the draft

These were decided during Wave 0 of the build, before any world shipped, to avoid adding
dependencies the repository forbids without an ADR. They are part of the contract now.

1. **SHA-256, implemented in-tree** (`src/hash.rs`), replaces `xxh3-128`. Identity is `[u8; 32]`.
   Published FIPS 180-4 test vectors pin the output. Replacing it with a crate is allowed only
   behind an ADR, and the *bytes* are the contract either way.
2. **SplitMix64** (a `u64` finaliser chain) replaces the "low 64 bits of xxh3" for stream seeds and
   per-block randomness. Integer-only, exact on every target.
3. **`Section::Paletted` carries a `solid: u32` field** in addition to the three fields named in
   §4.1, because the O(1) solid count is required by §4.1 and the three-field shape cannot provide
   one.
4. **No `toml` dependency.** The description reader is a strict `key = value` subset parser that is
   TOML-compatible in the text it accepts and rejects everything else. It is not a general TOML
   parser.
5. **Flat `src/` modules** (`src/section.rs`, `src/graph/`, …) rather than a `terrain/` subtree.
   Existing house style; `terrain-gen` needs `pub` items, which the new modules provide.
6. **Block ids come from the name sort.** The interview's "reserved `0/1/2`" line is not part of the
   contract: ids are runtime-only and never persisted, so only the *names* are stable. `core:air` is
   0 because it sorts first; other numbers are an implementation detail.

## 1. Scope

v1 delivers a finite, deterministic, single-player world with:

- a versioned world description and generator version (§2);
- declared field graphs compiled per world (§5);
- heightmap landform with beaches, terraces, and a coast that is a level set (§6);
- rivers, lakes, and a sea from tile-independent hydrology (§7);
- latitude/altitude climate, rain shadow, four climate biomes, and data surface rules (§8);
- a depth-limited 3D detail band, two cave families, and cave mouths (§9);
- strata, deep rock, and ore veins (§10);
- trees and small scatter placed through two-phase features (§10);
- 15-level skylight and block light with bit-identical incremental repair (§11);
- greedy blocky meshing with per-vertex AO and an 8-byte quad record (§12);
- three LOD levels with skirts (§13);
- camera-relative rendering, one fog function, and a defined pass order (§14);
- a square streaming ring with priorities, revisions, and budgets (§15);
- a fully specified persistence format (§16) — header, journal, section record, backup rotation, and
  migration — whose save writer is the next milestone. No world in this build reads or writes
  sections;
- typed errors, metrics, debug views, and a headless `terrain-gen` (§17–§19).

Out of scope is listed in §21, with the reserved extension points named.

## 2. World identity

### 2.1 Description file (A1.1, A1.3)

A world directory contains `description.toml`, `header.bin`, section files, the edit log, and
journal. `description.toml` MUST be the sole source of generation parameters.

- Parsing MUST be typed and strict: unknown keys, duplicate keys, and missing keys are errors.
  There are no runtime defaults; every key is present.
- Integers are plain decimal. `f32` values are stored as bit patterns in hexadecimal
  (`0x3F000000`) with a decimal comment. NaN and infinity MUST be rejected. `-0.0` MUST be
  normalised to `+0.0` before hashing.
- The description MUST contain every generation-relevant parameter, including
  `WORLD_HALF_EXTENT`, `WORLD_HEIGHT`, `SEA`, `MAX_TERRAIN_Y`, `DEEP_Y`, `MACRO_CELL`,
  `TILE_BLOCKS`, `HYDRO_BORDER`, `RIVER_MIN_CELLS`, `LAKE_MIN_CELLS`, `LAKE_MIN_DEPTH`,
  `CAVE_T`, `CAVE_MIN_ROOF`, `ENTRANCE_*`, `DETAIL_*`, `WATER_MARGIN`, `WATER_SKY_ATTEN`,
  `LAPSE`, `T_SNOW`, `BLEND_BLOCKS`, `STRATA_*`, `FAULT_*`, `ORE_*`, `TREE_CELL`, `Z_EQUATOR`,
  `Z_HALF_SPAN`, `EDGE_BAND`, `BEACH_*`, `SHADOW_*`, `MARITIME`.
- Renderer, streaming, and budget settings (`VIEW_RADIUS`, LOD radii, `MEM_CAP`, worker counts,
  fog, flush intervals, presets) MUST NOT appear in the description; they are runtime config and
  never part of identity.
- A constant that changes block output MUST either be a description key or be pinned by the
  generator version. The v1 taxonomy thresholds `T_COLD_SHORE`, `T_COLD_SHELF`, and the clay-patch
  share are the latter: they live in code and changing them is a version bump, not a description
  edit.
- A `[meta]` table MAY exist and MUST NOT be hashed. It carries preset name, tool version, and
  provenance.
- Hand edits are allowed but change identity. The tool MUST NOT rewrite an existing description in
  place; it writes a new file and a new header entry (T1.7).

### 2.2 Canonical encoding and hashes (A3.1, A3.5, B3.8, T1.2)

Three hash roles are pinned and MUST NOT change without a generator and save-format bump. §0.1
records why the first two differ from the interview's original proposal.

1. **Identity hashes** — **SHA-256** (`src/hash.rs`, in-tree, FIPS 180-4), over canonical bytes.
   Stored as `[u8; 32]`. Text form is 64 lowercase hex characters; other lengths or uppercase are
   rejected. Used for `description_hash`, graph hash, manifest and record hashes, `baseline_hash`,
   per-stage hashes, golden hashes, and the stream vocabulary hash. Published test vectors
   (empty string, `"abc"`, a million `a`s) pin the implementation.
2. **Stream seeds** — `hash64`, a SplitMix64 finaliser chain over
   `[seed u64][generator_version u16][stream_id u16][coords i32…]` in a fixed word sequence, in a
   stack buffer.
3. **Per-sample integer hash** — the same SplitMix64 finaliser over integer inputs.
   `hash01 = (mix >> 40) as f32 * 2^-24`, exact in `f32`.

Canonical encoding: fixed field order from the struct definition, little-endian fixed-width
integers, length-prefixed strings and arrays, maps sorted by key, a leading `schema_version: u16`.
Comments, whitespace, key order, file names, and formatting MUST NOT affect any identity hash.
`std::hash`, `DefaultHasher`, and `derive(Hash)` MUST NOT feed output. A 64-bit truncation is
allowed only for in-memory map keys.

The header stores `hash_algorithm = "sha256/v1"` beside every hash so a future algorithm can
coexist.

### 2.3 Generator version (T1.2, T1.3, T1.12)

Identity is the pair `(generator_version: u16, description_hash: [u8; 32])`.

- `generator_version` is monotonic, never reused, and bumps on any generation-code change that can
  alter output bytes: a new node type, predicate, feature shape, noise change, or a bug fix that
  moves terrain. Performance-only changes do not bump.
- The seed is **not** in `description_hash`; one description serves many worlds.
- The engine ships the current version and the two previous frozen evaluators:
  `Generator { version, tile(description, tx, tz), column(description, cx, cz) }`. A frozen
  evaluator changes only if output bytes are provably identical.
- A version becomes Deprecated when its successor's successor ships, and Removed the release after
  that, never before `MIN_SUPPORT_DAYS = 180`. Save-format readers live at least 4 releases and 12
  months.
- A world whose version is newer than the engine MUST refuse to open. A pruned version MUST offer
  Migrate / Fork / Cancel, never silent regeneration.

The versions table (`docs/versions.toml`) pairs every shipped `(version, description_hash)` with
notes and dates. CI MUST run the goldens for every shipped version and fail if a pair's output
changes.

### 2.4 Determinism boundary (A2.1, A2.3, A2.5, A2.4)

The promise is **same machine, same binary**: a world is bit-identical for a given description,
seed, platform tag, and build configuration. Cross-platform equality is not promised.

Field maths MUST use `+ - * /`, `sqrt`, `abs`, `min`, `max`, `floor`, `ceil`, `clamp`,
comparisons, and explicit casts. It MUST NOT use `sin`, `cos`, `tan`, `atan2`, `pow`, `exp`, `ln`,
`log`, `mul_add`/FMA, fast-math, float reductions whose order is not fixed, or `HashMap` iteration
order. NaN and infinity are rejected at load.

Generation MUST be a pure function of `(description, seed, generator_version, coord)`. It MUST NOT
read a clock, tick, frame counter, thread id, process id, address, locale, environment, filesystem,
network, or the edit revision. Imports of `std::time`, `chrono`, `thread_rng`, `std::env`,
`std::fs`, and `std::process` are rejected in generation crates by clippy `disallowed_*` plus a CI
grep. A determinism test runs the same region with a mocked clock, 1 and N workers, and shuffled
order, and asserts identical block hashes.

### 2.5 Streams (A3.2, A3.3, A3.4)

- `stream_vocab.toml` is checked in with `{id: u16, name, since_version, deprecated}`. Ids are
  assigned once and never reused or renumbered. Renaming or reassigning changes terrain and is a
  version bump.
- Rust call sites use generated constants (`stream::BIOME_DITHER`); an unknown name is
  `DescriptionInvalid`.
- Parameterised families (`ore:<name>`, `tree_pos:<group>`) are `(family_id, member_index)`,
  resolved at content-pack compile time; the map is part of the description hash.
- Draws are stateless per coordinate. Multiple values at one coordinate use a stack-local
  `LocalRng` seeded once from `hash64(...)`, consumed in a fixed golden-tested order, capped at
  `MAX_DRAWS_PER_KEY = 64`. Rejection loops have a fixed bound (~8) and a deterministic fallback.
- `LocalRng` MUST NOT be shared between features, cached, or seeded from anything but the hash.

### 2.6 Content pack (A5.1–A5.4, T1.5)

- Block, biome, feature, species, ore, and rule ids are stable names `namespace:name`, lowercase
  `[a-z0-9_]`, at most 64 bytes, immutable once shipped. Removal is deprecation
  (`deprecated = true`, still loads). Renaming is forbidden; aliases are load-time only and never
  rewrite saved data.
- Runtime `BlockId` is `u16`, assigned at load **in sorted-name order**, so ids are dense and
  reproducible and `core:air` is 0 by construction (`core:bedrock` is 1 only because it sorts
  second). Ids are never saved, hashed, or reported, so a numeric slot is **not** a contract:
  `core:water` is not required to be 2. The placeholder range starts at `UNKNOWN_BASE`, above the
  pack's ids. A pack missing `core:air`, `core:bedrock`, or `core:water` fails validation — by
  name, not by number.
- Properties are frozen per id and live only in the pack: opacity, `sky_atten`, emission,
  `render_shape`, `render_layer`, collision, `replaceable`, liquid, hardness, drops, tint, texture
  refs, tags. They compile to flat arrays indexed by `BlockId`. Code attaches behaviour through
  tags or startup-resolved handles, never numeric literals.
- Each record is hashed over its canonical parsed bytes; `pack_hash` is over the sorted
  `(kind, name, record_version, record_hash)` list. Render-only fields use a separate
  `render_hash`, so retexturing never forks identity. A generated `pack.lock` is checked in.
  Implemented as `textures.top|side|bottom` per record plus `content::render_hash` (`ADR-0015`);
  the mesher resolves each face's reference to a texture layer.
- To **generate**, every record pinned by the description MUST match exactly
  (`ManifestIncompatible` otherwise); extra unpinned records are ignored. To **open saved
  terrain**, a valid header/description/journal is enough, and saved names missing from the pack
  degrade to placeholders. **A world with any unknown name loaded is read-only**: no edits, no
  journal, no section saves until the content is restored or aliased (`T1.5`), so the original names
  are never re-encoded. A world MUST NOT generate against a subset.

## 3. Coordinates, units, and keys

### 3.1 Axes and units (A4.3, A4.4)

Right-handed, Y-up. `+x` east, `+y` up, `-z` north, `+z` south; looking down `-z` with `+y` up,
`+x` is to the right. `WIND_DIR = +x` blows east.

1 block = 1 metre in every subsystem. World block coordinates are `i32`; camera, entities,
physics, and audio are `f64` metres with the same origin and axes. Block `(x,y,z)` occupies
`[x,x+1) × [y,y+1) × [z,z+1)` and has centre `(x+0.5, y+0.5, z+0.5)`. Noise and graph frequencies
are cycles per block. Amplitudes and heights are blocks. Velocity is `m/s`, time seconds, angles
radians internally. The only `f64 → f32` conversion is the render boundary (§14.3).

Face normal order is `+x -x +y -y +z -z`; winding is counter-clockwise from outside; texture `v`
increases downward on side faces.

### 3.2 Keys (A4.2)

Keys are explicit types, never packed integers:

```rust
struct ColumnKey { cx: i32, cz: i32 }
struct SectionKey { col: ColumnKey, sy: u8 }   // 0..SECTIONS_PER_COLUMN
struct LodKey { level: u8, col: ColumnKey }
struct RegionKey { rx: i32, ry: i32, rz: i32 } // 32^3 regions
```

They derive `Copy, Eq, Ord` (field order, so iteration is deterministic) and `Hash` for in-memory
maps only. Any key used as hash input is serialised as explicit `i32` LE fields in declared order.
Runtime maps use a non-randomised hasher; output paths use `BTreeMap` or sorted `Vec`.

`from_block`, `origin_block`, and `neighbours8` live in one place; nothing computes keys inline.
Conversions use `floor_div` and `rem_euclid`, never `/` or `%` on signed integers.

Saves are per column with sections as slots (§16.1); region files group a 16×16 Column grid and
use signed decimal text filenames. On-disk keys are `i32` LE plus `u8` and are versioned with the
save format.

## 4. Data model

### 4.1 Sections (I1.1, I1.2)

A section is 32×32×32 = 32,768 blocks.

```rust
enum Section {
    Uniform(BlockId),
    Paletted { palette: Vec<BlockId>, bits: u8, solid: u32, data: Box<[u64]> },
}
```

- `bits` is the smallest of `{1,2,4,8,16}` that fits `palette.len()`; indices are packed LSB-first
  into `u64` words and MUST NOT straddle a word boundary.
- The palette is unordered and append-only within a section. Nothing MAY depend on index values.
  Growth repacks in one pass. Demotion to `Uniform` is lazy (on save or timer).
- On disk, palettes store stable names or a manifest-local remap table, never runtime ids.
- A section carries an opaque/solid count so empty/full checks and skylight skipping are O(1).
- Hashes are over sorted palette ids plus decoded indices, never raw words.
- At most 65,536 block types.

Block index:

```text
index(x,y,z) = x + 32*(z + 32*y)     // x fastest, one y-layer contiguous
             = x | z << 5 | y << 10
```

`section_index` and its inverse are defined once. Local coordinates are masked `& 31`. One bulk
decode into a `[u16; 32768]` scratch is the only decode path. Index order is part of the save
format.

### 4.2 Columns (K1.5)

A column is a dense array of 12 section slots, each `Uniform(BlockId)` or `Allocated(Box<Section>)`.

- `Uniform` costs one slot value and no allocation. Only mixed sections allocate.
- A column is resolved only when all 12 slots resolve. "Missing" is not a downstream state.
- Generation writes `Uniform` for homogeneous sections; edits may demote lazily.
- The column caches `top_solid_y` min/max so meshing and lighting skip uniform air bands.
- Memory targets: ≤40 KB resident per typical column, ≤2.5 MB per worst-case LOD0 section mesh.

A full 384-tall column holds 393,216 blocks before sparse representation.

### 4.3 Revisions (I3.2, K3.1)

- `section_rev: u64` per section, bumped once per committed edit batch that touches it and on
  `Uniform ↔ Allocated` promotion. It is the batch revision, not a separate counter.
- `border_rev` per face, bumped when a face-adjacent cell of that section changes, so a neighbour
  goes stale only when a cell it actually read changed.
- `column_rev` is derived as `max(section_rev)` and is never stored on disk.
- Revisions are runtime state starting at the saved world `revision`; they are never persisted per
  section.
- Every job result records the `section_rev` of every section it copied (or the pure
  `(description hash, coord)` key for tile results) and is discarded on mismatch.
- Derived data (light, mesh, LOD, `HeightTile` edit state) is keyed by `section_rev`.

A `TerrainView` API provides `get_block(x,y,z) -> BlockId`. Out of world: `y < 0` returns bedrock,
`y >= WORLD_HEIGHT` returns air, and outside `WORLD_HALF_EXTENT` returns the boundary wall. An
unresolved column returns `Unloaded`, which physics MUST treat as solid. The main thread is the only
writer, so main-thread reads take no lock; workers use padded scratch (§5.7).

## 5. Field graph

### 5.1 Node vocabulary (B3.1, B3.5)

The graph is data in the content pack, compiled once per world into a flat, topologically ordered
evaluator with preallocated slots. There is no per-sample interpretation.

Closed node set: `noise(OpenSimplex2)`, `fbm`, `ridged`, `billow`, `warp`, `terrace`, `curve`,
`remap`, `add`, `mul`, `lerp`, `clamp`, `unit`, `const`, `input(pos)`, `curve_shadow`,
`curve_edge`, `peak_mask`. Adding a node type is a code change and a version bump with a golden
test.

Evaluation order is topological, ties broken by node id. The canonical serialisation fixes key
order and normalises `-0.0`; its hash is the graph hash and is stored in the description.

Noise MUST be OpenSimplex2 only (ADR-0012), normalised at the source:
`unit(raw) = clamp(raw * 0.5 + 0.5, 0, 1)`. `0.5` is neutral. Warp and ridged transforms run on the
raw signed value inside the noise layer. 3D density and caves are also `[0,1]`; a carve is
`density > CAVE_T`.

Every octave is bounded by measured constants `NOISE_BOUND_2D` and `NOISE_BOUND_3D`, set to the
measured maximum × 1.02. An offline sweep of hundreds of millions of samples, 2D and 3D, plus an
adversarial extreme search, produces them; the method and constants live with the spec. Changing
the noise implementation, hash, or gradient table requires re-measurement and a version bump.

fBm divides by the exact sum of amplitudes, computed at compile time. Ridged and billow transform
per octave and then normalise through the same divisor. Contrast is shaped only by explicit curve
or remap nodes after the fractal.

### 5.2 Domains and classes (B3.3, B4.4)

Every node declares a domain and a class:

| Class | Domain | Sampling |
|---|---|---|
| `lattice2D` | `D2(x,z)` | global 4-block lattice, bilinear per block |
| `block2D` | `D2(x,z)` | per column, after interpolation |
| `lattice3D` | `D3(x,y,z)` | 4×4×4 lattice, trilinear per block |
| `detail` | `D3` | restricted to the detail band (§9.2) |

A node may read only the same or a coarser class. A `D2` node MUST NOT read a `D3` node.
Interpolation happens exactly once, at the class boundary, on `f32` before any quantisation.
Nonlinear nodes (curves, terrace, thresholds) MUST run after interpolation unless declared
lattice-resolved. `D3` evaluation hoists `D2` values per column.

### 5.3 Lattice (B4.5)

Samples sit at `4k + 0.5` on every axis: the centre of block `4k`. Block `x` is in cell
`k = floor_div(x, 4)` with weight `w = rem_euclid(x, 4) / 4.0`, interpolated between samples `k`
and `k+1`. Weights are exact quarters.

`MACRO_CELL = 4` is defined once; `macro_cell(x) -> (k, w)` is the only place the maths lives. The
lattice is global in world coordinates and never chunk-anchored.

### 5.4 Curves (B2.2)

A curve is a list of control points with strictly increasing `x`, compiled to per-segment
Fritsch–Carlson monotone cubic coefficients. The compiler rejects non-increasing `x`, out-of-range
`y`, and fewer than 2 points. Output stays within `[min y, max y]` by construction; input is
clamped to the end points. Lookup is a linear scan to ~8 points, binary search beyond, evaluated in
Horner form with no `mul_add`. A per-curve `linear` flag covers terraces and hard cliffs; flat
segments get zero tangents.

### 5.5 Warp (B2.1)

One 2D warp node type, two noise taps:

```text
warp(field, seed_a, seed_b, freq, amp_blocks)
```

It offsets `x` and `z` by one OpenSimplex2 octave each and samples the input at the warped position.
Offsets are bounded by `±amp`, which grows the tile halo by `ceil(amp / MACRO_CELL) + 1`. At most
one warp per dependency path; chained warps are rejected. Warp applies to 2D macro fields at lattice
points only. 3D cave density gets no warp in v1.

### 5.6 Limits and cost (B3.4, B5.5)

Enforced at compile with `GraphInvalid { node, rule }`:

| Limit | Value |
|---|---|
| Total nodes | ≤128 (lattice-2D ≤64, block-2D ≤32, 3D ≤24) |
| Named outputs | ≤12 |
| Dependency depth | ≤16 |
| Fan-in | ≤4 |
| Curve points | ≤24 per curve, ≤64 total |
| Cave taps | `CAVE_TAP_BUDGET = 4` per 3D sample |
| Detail taps | `DETAIL_TAP_BUDGET = 2` per band sample |
| Warps | ≤1 per dependency path |
| Per-node parameters | required: `freq`, `octaves 1..=8`, `gain (0,1]`, `lacunarity [1.5,3.0]`, `stream_id` |

The finest octave wavelength MUST be ≥2 blocks, and ≥8 blocks for lattice nodes. The compiler
propagates `[0,1]` through every node and rejects a graph that can leave range before `unit()`. It
emits a weighted cost report per class; a benchmark asserts measured cost is within 25% of it.

Ceilings: `NS_PER_TAP_2D ≤ 100 ns`, `NS_PER_TAP_3D ≤ 150 ns`,
`stage_budget = samples × taps × NS_PER_TAP × 1.25`.

### 5.7 Neighbour access (I2.3, I2.4)

Each worker owns one reusable `PaddedBlocks` `[u16; 34³]`. A job takes a brief read borrow of the
target section and its 26 neighbours, copies into the buffer (Uniform by constant, Allocated by bulk
decode), releases all borrows, then runs lock-free.

```text
padded_index(x,y,z) = (x+1) + 34*((z+1) + 34*(y+1))    // x,y,z in -1..=32
```

Vertical neighbours come from the column's 12 slots; out-of-world vertically is bedrock below and
air above; the horizontal world edge mirrors the edge row. Light uses a parallel nibble scratch and
exchanges column borders. A job records the revision of every section it copied. A unit test
asserts the padded copy equals direct world reads for every cell, including `Uniform` neighbours,
edges, and corners.

## 6. Macro landform

### 6.1 Height composition (C1.1, C2.1, C2.2)

```text
base   = curve_base(continentalness)            // blocks relative to SEA
relief = curve_relief(roughness) * coast_mask   // blocks
height = SEA + base + relief * curve_shape(peaks) * mask(continentalness)
```

The v0.0.7 field named `erosion` is renamed **`roughness`**; `erosion` is reserved for a future
solver and MUST NOT appear in graph data (CI lint). Continentalness alone decides where land meets
sea: the coast is the level set `base = 0`. `relief` is masked to ~0 at the coast, so retuning
mountains cannot move the shoreline.

`peaks` is `fbm_ridged` (OpenSimplex2, 4–5 octaves, frequency ~1/256), `unit()`, then
`curve_shape`, which is convex near 0 (broad foothills) and steep near 1 (rare peaks). Plateaus come
from a `terrace` node after shaping, gated by a curve of `roughness`.

`height` stays `f32` everywhere. Blending, slopes, caves, and features read the float.

### 6.2 Coast and beach (C1.6, C1.7)

`base` is authored with knots on continentalness `c` (relative blocks):

| `c` | `base` |
|---|---|
| 0.00 | −32 |
| 0.25 | −30 |
| 0.38 | −12 (shelf break over ~0.07 of `c`) |
| 0.45 | −2 |
| 0.48 | 0 (waterline) |
| 0.52 | +3 (beach plateau) |
| 0.60 | +10 |
| 1.00 | inland plateau |

Invariants: floor depth 32; `base` monotone; `base = 0` at exactly one `c`; the beach plateau spans
only `0..+3`.

The beach is guaranteed at least 3 blocks wide:

1. **Vertical flattening.** For `|base| < 0.75`, blend toward `BEACH_PLATEAU = 0.5` with `smoothstep`
   over another 0.75. This runs per block (`block2D`), not on the lattice, so bilinear interpolation
   cannot smear it. The plateau floors to `SEA` and never sits at or below the waterline.
2. **Horizontal width.** The slope of `curve_base` between `base = -2` and `+3` is capped, and the
   compiler MUST prove
   `max|grad(c)| * max dbase/dc <= 5 / BEACH_MIN_WIDTH` using the noise gradient bound. A graph that
   cannot meet it refuses to compile.

Terrace and river carve are gated off inside the coast mask, so mouths keep the plateau.

### 6.3 Cliffs and terraces (C2.4)

`terrace(h, step_blocks, sharpness, mask)` quantises `h` toward multiples of `step_blocks` (4–8),
anchored to `SEA` (`SEA + k*step`), with a smooth-to-vertical riser controlled by `sharpness`, then
lerps by the mask. It runs per block after interpolation and **before** `floor(h)`, so risers can be
vertical. Max displacement `step/2` is included in the height bound. Slope-side material changes do
not create geometry. No cliff primitives in v1.

### 6.4 Quantisation and vertical layout (C3.1, C4)

`top_solid_y(h: f32) = floor(h)`. This is the **only** place a height becomes an integer. Surface,
water, caves, features, slope, and spans all read it.

- `WORLD_HEIGHT = 384`, `y ∈ 0..=383`.
- `SEA = 128`.
- `MAX_TERRAIN_Y = 352` (sea + 224), with 32 blocks of headroom. The compiler computes
  `SEA + max(base) + max(relief*shaped) + max(terrace displacement) + max(detail)` from the curve
  knots and node bounds and rejects any graph above it.
- `y = 0` is indestructible bedrock. Players build `1..=383`; generated features clamp to
  `WORLD_HEIGHT-2 = 382`.
- Out-of-range placement is rejected by one `can_edit(x,y,z)` gate before any record is written.
  Bulk edits clip and report the clipped count.

### 6.5 Height tile (E4.6)

`HeightTile` is `i32 top_solid_y` per column, **130×130** (128 + a 1-block margin), a pure function
of the field tile and the hydrology tile.

Build order per column: bilinear macro height → per-block detail → terrace → river carve → one
`floor(h)`.

It is keyed `(description hash, tx, tz)`, disposable, and the **only** height array that
`slope_drop`, tree placement, and surface rules read. Slope uses this 2D top before 3D detail and
caves; span tops use §9.5.

## 7. Hydrology

### 7.1 Tiles and mode (D8.1–D8.4)

- A hydrology tile owns 512×512 blocks (128×128 cells), aligned to the global grid and to 4×4
  field tiles. Stored area adds `HYDRO_BORDER = 16 cells` (64 blocks) per side: **160×160 cells**.
- Border cells are recomputed from the global lattice by each tile and are bit-identical to the
  neighbour's interior. They are never copied at runtime and never read by consumers.
- Tiles are pure functions of `(description, hx, hz)`, cached outside the world directory, and
  evictable. Invalidation is hash-only; edits, carve, LOD, and time never touch them.
- **v1 ships tile mode** (the interview's option a): a hydrology tile owns 512×512 blocks and adds
  `HYDRO_BORDER_CELLS = 16` cells of halo, recomputed from the global lattice. A basin larger than
  the halo is treated as open and drains at the tile edge, so a seam-straddling basin can be
  under-counted; the seam suite bounds that, and no lake level may depend on tile order for basins
  inside the halo. **World mode** (one whole-world pass sliced per tile, no halo and no basin cap) is
  implemented in `hydro::WorldHydro` and is the intended v1.1 upgrade; wiring it is a
  type/ownership change, not a format change.
- At most 2 hydrology jobs in flight; they are leaf jobs with no queued dependency. Cost ≤20 ms per
  tile excluding macro height (~60 ms cold including it); world mode ~150 ms once at open, off the
  main thread behind the loading cover.

### 7.2 Depression filling (D1, D2.5, D2.6)

Priority-flood on the macro lattice, queue key `(filled_height, cell index)` compared exactly.
Seeds are sea cells and boundary inflow cells. For neighbour `n` of popped cell `c`:

```text
filled[n] = max(h[n], filled[c] + EPS)        EPS = 1/64 = 0x3C800000
```

`EPS` is exact in `f32` for heights up to ~2^17 blocks. Filled heights are used **only** for routing
and accumulation, never for the surface. Distortion is bounded by the longest flat run; a debug
assertion checks the lake-surface slope bound.

A lake is a connected set of cells where `filled > h + LAKE_MIN_DEPTH`. Its level is the **outlet's**
filled height, floored once; interior filled heights never define a surface.

### 7.3 Flow and accumulation (D2.1)

D8 on the macro grid. Each cell's receiver is the lowest of its 8 neighbours by
`drop = (h_cell - h_nbr) / dist`, with `dist = 1` or a pinned `sqrt(2)` constant for diagonals.
Ties break by fixed order N, NE, E, SE, S, SW, W, NW. Accumulation is `u32` upstream cell counts,
processed by height descending then cell index. Every non-sea cell MUST have a receiver, and no
cycles may remain after fill.

Routing uses the pre-detail macro height only, never per-block noise. No per-block refine in v1.

### 7.4 Rivers (D3)

- `RIVER_MIN_CELLS = 300` (world parameter). A cell is river when `acc >= RIVER_MIN_CELLS`.
- Width: `clamp(W_SCALE * sqrt(acc - RIVER_MIN_CELLS + 1), 1, 10)`, exact `sqrt`, never `pow`.
  Width ramps from 0 over the first ~50 accumulation above the threshold.
- Cross-section: bed half-width `w/2`, bank half-width `w/2`, depth `D = clamp(w/2, 2, 6)`, bank
  falloff by `smoothstep`. Distance comes from the interpolated river-influence field, not a
  per-block path search.
- Carve is applied to the `f32` height before `floor(h)`, gated by the coast mask, and never lowers
  the bed below `SEA-30` or `y = 1`.
- Meander: `p' = p + n_perp(p*MEANDER_FREQ) * amp`, one OpenSimplex2 octave,
  `amp = clamp(1.5*w, 0, 8)` blocks, along the receiver direction (unit vector per cell, bilinearly
  interpolated and renormalised), wavelength ~64–256 blocks, fading to 0 at headwaters and mouths.
- Bed material: sand where flat, gravel otherwise, 1–2 blocks thick. Banks keep the surface
  material.
- A river chain ends at the sea or a lake (open or closed) and MUST NOT end on dry land or in a
  basin that failed the lake gates. Endorheic basins are allowed; the carve ends where the bed meets
  the lake level.

### 7.5 Lakes and water levels (D7.2, D7.4)

- A basin becomes a lake only if `max depth >= 2` and `area >= LAKE_MIN_CELLS = 12`; otherwise it
  stays dry and flow passes through its filled cells.
- Level is the outlet (spill) height floored once. `MAX_LAKE_Y = MAX_TERRAIN_Y - 32`.
- Open lakes feed the outlet as a river source with their accumulation. Closed lakes are recorded,
  never drained or evaporated in v1.
- River level per cell is `floor(h_center - 1)`, then made monotone downstream:
  `level[c] = min(level[c], level[receiver] + 1)`. Per-block level is the bilinearly interpolated
  value floored once, clamped to `bed_y+1 ..= bank_top-1`. At a lake outlet it is ≥ the lake level;
  at the sea it meets and never exceeds `SEA`.
- Water is static in v1: no flow simulation, no currents, no level changes after generation.

### 7.6 Water blocks (D7.1, D7.3)

Water is a normal block. Generation fills `top_solid_y+1 ..= SEA` per column, plus lake and river
levels. Sections fully covered by water or solid become `Uniform`. Sections mixing water with one
other block use a 1-bit palette.

Air below `SEA` stays air: caves and player spaces are never flooded. There is **no water table**,
no springs, wells, aquifers, or seepage in v1, and caves never contain water because of the
`WATER_MARGIN` gate (§9.6).

## 8. Climate, biomes, and surface

### 8.1 Temperature (E1.1, E1.2)

```text
temp = clamp(curve_lat(|z - Z_EQUATOR| / Z_HALF_SPAN)
             + (fbm(freq 1/512) - 0.5) * NOISE_TEMP_AMP
             - LAPSE * max(h_macro - SEA, 0), 0, 1)
```

`LAPSE = 1/320` per block, `NOISE_TEMP_AMP ≈ 0.15`, and `Z_HALF_SPAN = WORLD_HALF_EXTENT`, so both
`z` edges are coldest. Climate is lattice-only: a 1-block bump or a river carve never changes it.

### 8.2 Humidity and rain shadow (E2, E2.4)

Fixed direction `+x` wind (march toward `-x`), `SHADOW_RANGE_CELLS = 32` (128 blocks):

```text
shadow = clamp(max_k(h[x-k] - h[x] - SLOPE_K*k*MACRO_CELL), 0, inf)
rain   = curve_shadow(shadow / SHADOW_NORM)                  // [0,1]
coast  = curve of upwind distance to the first ocean cell
humidity = clamp(unit(noise_humid) * (1 - SHADOW_STRENGTH*rain) + MARITIME*coast, 0, 1)
```

Evaluation order is exactly: `n = unit(noise)`, `d = 1 - SHADOW_STRENGTH*rain`, `t = n*d`,
`h = clamp(t + MARITIME*coast, 0, 1)`. `SHADOW_STRENGTH` and `MARITIME` are `f32` bits in `[0,1]`.
`shadow = 0` and `coast = 0` MUST return `n` bit-exactly. A saturation counter warns above ~5% of
land cells; the tile halo grows upwind by `SHADOW_RANGE_CELLS`.

There is no seasonality field. `T_SNOW ≈ 0.25` sets the snow line; a colder world is a different
description.

### 8.3 Biome selection (E3.1, E3.2, E3.5)

Biomes are records with climate centres and a weight. Distance is
`d2 = (dt*T_SCALE)^2 + (dh*H_SCALE)^2 / weight`, no `sqrt`; argmin ties break by lowest id. Chosen
per lattice cell; non-climate overrides (ocean, beach, river, lake, snow) never enter climate space.

v1 climate biomes (starting centres, tuned against the measured climate histogram):

| id | temp | humidity |
|---|---|---|
| `plains` | 0.55 | 0.45 |
| `forest` | 0.50 | 0.75 |
| `desert` | 0.85 | 0.10 |
| `tundra` | 0.10 | 0.35 |

`ocean` and `beach` are **override surface sets**, not climate biomes. "Mountains" is not a biome:
alpine look comes from snow cap, slope stone, strata, and deep rock.

The compiler requires ≥3 climate biomes, distinct centres, resolvable ids, and a catch-all rule per
biome. Per seed, ≥3 climate biomes MUST be present and each MUST cover ≥5% of land.

A tile stores three parallel `u8` arrays per cell: `biome_best`, `biome_second`, and
`biome_margin`, where `q = clamp(floor(d / BLEND_W * 255 + 0.5), 0, 255)`,
`d = m / |grad m|` is the straight-line block distance to the border, and
`m = d2_second - d2_best`. The analytic gradient is
`grad m = 2*[(T_b - T_s)*grad T + ((H - H_s)/w_s - (H - H_b)/w_b)*grad H]`, with `grad T` and
`grad H` forward differences over one macro cell. `BLEND_W = BLEND_BLOCKS / (2*s95)` blocks,
where `s95 ≈ 0.8042` solves `smoothstep(s95) = 0.9`; this makes a straight border measure
exactly `BLEND_BLOCKS`. The dither reconstructs `w = 0.5 + 0.5*smoothstep01(q/255)`; `q = 0` is
the border (`w = 0.5`), `q = 255` is deep inside (`w = 1`). One helper `biome_pick(tile, cell,
x, z)` owns the decision; climate values are stored separately as `f32` and never feed terrain
through the biome arrays.

### 8.4 Transitions (E3.3, E3.6)

Per block, pick the best biome if `hash01(seed, "biome_dither", x, z) < w`, else the second. The hash
is keyed on `(x, z)` only, never `y`, so a column has one biome and chunk order cannot matter.
Terrain-affecting values (height, snow line) use undithered lattice climate, so terrain stays
smooth.

`BLEND_BLOCKS = 24` is a calibration target, not a label: `W_blocks` is measured from `w = 0.5` to
`w = 0.95`, doubled, along transects perpendicular to each biome-pair border. Acceptance: median
20–28 blocks, p10 ≥ 8, p90 ≤ 64, per pair, over the sweep seeds. Failures are fixed in biome data
first; the `q` formula itself is pinned.

v1 status (16 seeds, 2026-11-20): all five pairs are assessed and pass. Medians are 23.33-27.17,
p10 14.69-18.76, and p90 32.55-63.32. `terrain-gen calibrate-blend --seeds 16` exits `0`; the
per-pair table is in [Benchmark results](../dev/benchmarks.md#terrain-blend-calibration-2026-11-20).
The column pipeline calls `biome_pick` once per column for the surface rules and the `3 x 3`
feature footprint, so the stored margin is the shipped transition. Both are bit-identical for a
column and do not depend on chunk order.

### 8.5 Surface rules (E4.1–E4.5)

Each biome owns an ordered predicate list; the first match wins, resolved **per column**. A rule is
`(predicate, block_id, depth_blocks)`. Predicates are a closed enum: `depth_below_top`,
`depth_below_sea_in(lo,hi)`, `slope_drop_ge`, `slope_gt`, `height_vs_SEA`, `temp_lt`,
`humidity_lt`, `water_adjacent_within`, `is_river_bed`, `is_lake_bed`, `near_coast`, `cave_floor`,
`near_mouth`, plus `and/or/not`. Block ids are stable names resolved at compile. Rule constants are
`f32` bits.

Fixed global precedence, first match wins:

| # | Rule |
|---|---|
| 1 | river bed / lake bed |
| 2 | underwater sea floor (`top_solid_y < SEA`) |
| 3 | beach band (`0 <= base <= +3`, `top >= SEA`) |
| 4 | snow cap (`temp < T_SNOW`, never below `SEA`, never on bed or beach, ±2 block jitter) |
| 5 | slope stone (`slope_drop >= 2`, 35% dither at 1) |
| 6 | biome topsoil |
| 7 | biome filler |
| 8 | stone (`stone_at`) |
| 9 | bedrock at `y = 0` |

Rules 1–5 are locked entries in every biome list; the compiler rejects a biome rule before slot 5 or
a missing catch-all. `slope_drop` is the max absolute difference of `top_solid_y` against the four
cardinal neighbours, from the `HeightTile`.

Sea floor (slot 2), with `d = SEA - top` and bands as `(offset, material)` rules:

| `d` | material |
|---|---|
| 1–4 | sand, gravel if `temp < T_COLD_SHORE` |
| 5–14 | sand, gravel if `temp < T_COLD_SHELF`, hashed clay patches |
| 15–24 | gravel |
| ≥25 | clay or silt |

Band borders are ragged via `hash01(seed, "seafloor_dither", x, z) < 0.35`, keyed `(x,z)` only.
Three blocks of band material, then `STONE`. Bands MUST tile `d >= 1` with a catch-all.

## 9. Volume synthesis

### 9.1 Column pass order (F1.5, D9)

Per column, fixed:

1. `HeightTile` → `base_top`.
2. Fill solid `<= base_top`; water `base_top+1 ..= SEA`, plus lake and river levels.
3. 3D detail band edits solidity (excluded where `base_top <= SEA+2`).
4. Cave carve; the roof is tested against the **highest solid after step 3**.
5. `cave_water_repair` — **no-op in v1**.
6. Span scan, `stone_at`, surface stack.
7. Ores → boulders → trees → plants.

Carve never floods; air below `SEA` stays air. There is no erosion stage and no repair pass.

### 9.2 Detail band (F3.1–F3.4)

Detail applies only in `y ∈ [base_top - 24, base_top + 8]` (`DETAIL_DOWN`, `DETAIL_UP`).

```text
density3d = clamp((h_f32 - (y + 0.5)) / DETAIL_SOFT, -1, 1)
          + DETAIL_AMP * band_fade(y) * mask * n3
mask      = curve_detail(roughness) * coast_mask * curve_slope(macro_slope)
```

`DETAIL_SOFT = 8`; `n3` is one OpenSimplex2 fBm (2 octaves, ~1/24) in `[-1,1]`. `band_fade` reaches
0 at both band edges, so the band joins the heightfield with no seam. Solid iff `density3d > 0`.

Exclusions: `base_top <= SEA + 2`, river and lake beds, within 2 blocks of a bank, and `y < 1`.
Floating cells must exceed `FLOAT_MIN = 0.15`; slivers are dropped. No connectivity pass runs in
v1. `DETAIL_AMP`, `DETAIL_SOFT`, and the band sizes are world parameters and are included in the
`MAX_TERRAIN_Y` proof.

### 9.3 Caves (F2.1, F2.3)

Two families on the global 4×4×4 lattice, trilinearly interpolated, combined as
`density = max(cheese*W_cheese, spaghetti*W_spag)` and compared to `CAVE_T`.

- **Cheese:** `fbm` 2 octaves at ~1/48, scaled by depth and floor curves.
- **Spaghetti:** two independent single-octave fields `a`, `b` at ~1/64;
  `tunnel = 1 - smoothstep(0, SPAG_R, max(|a|,|b|))`, minimum radius ≥3 blocks.

Depth weights (monotone curves, `[0,1]`), with `d = top_solid_y - y`:

- Cheese: 0 for `d < 12`, 1 by `d = 40`, flat to `d = 140`, 0 by `d = 200`. Floor: 0 below `y = 8`,
  1 by `y = 24`. Must be 0 where `top_solid_y <= SEA+1 && y > top-24`.
- Spaghetti: 0 below `y = 12`, 1 from `y = 24`, flat to `y = 296`, 0 by `y = 308`. Depth 0 at
  `d < 4`, 1 by `d = 16`. On lowlands near sea the ceiling is capped at `SEA - 8`.

Weights are lattice-resolved from `HeightTile` tops, so the window costs no 3D tap. The roof rule
still applies. Taps are bounded by `CAVE_TAP_BUDGET = 4`.

### 9.4 Roofs and mouths (F2.5)

Default roof: carve only where `top_solid_y - y >= CAVE_MIN_ROOF = 4`.

An entrance field ([0,1], fBm ~1/192, `curve_entrance`) lowers the roof smoothly:

```text
roof = lerp(4, 0, smoothstep(ENTRANCE_T, ENTRANCE_T + 0.05, entrance))   // ENTRANCE_T ~ 0.82
```

and eases the density threshold by `ENTRANCE_EASE`. Hard exclusions the entrance cannot override:
`y < 1`, `top_solid_y <= SEA+1`, river and lake beds, within 2 blocks of a bank. Mouths require
`macro_slope_ge(MOUTH_SLOPE_MIN)` and are flagged per column so §9.5 treats the top span as open to
the sky.

### 9.5 Spans and materials (F4.1–F4.3)

`resolve_spans(column) -> [(y0, y1, kind)]` scans bottom-up after carving.

- One solid span (the common case) uses the column stack of §8.5 unchanged.
- Multiple spans: the **top** span keeps the full stack. Every lower span top (cave floor) gets a
  reduced rule set; cave ceilings are always stone. Depth is measured from the nearest air above,
  capped at the filler depth.
- "Open to sky" is decided by skylight reach, not span count.

Cave walls, ceilings, and span undersides are always `stone_at`. Cave floors get gravel patches:

```text
gravel iff hash01(seed, "cave_floor", x, z) < GRAVEL_P(zone)
        && patch_field(x,z) > GRAVEL_PATCH_T
```

`GRAVEL_P` rises near mouths and in the first ~12 blocks below `top_solid_y`, falling to 0 below
`DEEP_Y + 4`. Deep rock is never gravel. Gravel is one block thick. Mouth columns may keep biome
topsoil on the floor only where the span is open to the sky, for `CAVE_MOUTH_BLEND = 4` blocks
inward.

### 9.6 Water margin (D9)

Cave carve and 3D detail are skipped within `WATER_MARGIN = 2` blocks of any water cell or any
column with a non-empty water range, applied at column granularity with a 1-column dilation of the
water mask, read only from the `HeightTile` and hydrology tile. `WATER_MARGIN` is a world parameter
≥1. After steps 1–4, no carved air cell may be face-adjacent (6-neighbour, across column and section
borders) to water; a debug check and a sweep check raise
`InvariantViolated{check:"cave_water_adjacent", coord}`.

### 9.7 Strata and deep rock (G1.1, G1.4, G1.2)

```text
band_index = floor_div(y + 0.5 + strata_warp(x,z) + fault(x,z), THICKNESS)
```

`strata_warp` is one fBm (2 octaves, ~1/96) scaled by `STRATA_WARP_AMP ≈ 8`, evaluated on the 4-block
2D lattice. `fault` is a piecewise-constant offset from a ~256-block cell grid (hash-keyed,
`0` or `±3`), smoothstepped over 4 blocks. The band table is indexed by `band_index mod N`.

Folds and faults deform **materials only**; they never move height. Visible fault scarps are
deferred behind a version bump.

Below `DEEP_Y = 64` everything is one deep rock, with a 4-block ragged transition driven by
`hash01(seed, "deep_dither", x,y,z)` at thresholds 1.0/0.75/0.5/0.25. Strata lookup is skipped
below `DEEP_Y`.

`stone_at(x,y,z)` is the only resolver for "generic stone": deep rock below `DEEP_Y`, the dither
band, else the strata band. Rule 8, cave walls, cliffs, span undersides, slope stone, and ore host
checks all call it. A golden test asserts that a cave wall, a cliff face, and subsoil at the same
coordinate return the same id.

## 10. Features

### 10.1 Regions and discovery (H1.1–H1.4)

- Discovery regions are 32³, aligned to `SECTION_SIZE`, indexed by `floor_div`.
  `REGION_SIZE = SECTION_SIZE`; it is not an independent constant.
- Every feature type declares `MAX_EXTENT`, measured from its centre to the farthest block it can
  write. The compiler rejects `MAX_EXTENT > 32`. Ores ≤8; trees ≤12.
- Centres always lie inside their own region; apply gathers exactly 3×3×3 regions.
- Candidate RNG is `hash64(seed, "feat:<name>", region)` consumed in a fixed order; ids are stable;
  overlaps resolve lowest-id-wins.
- Discovery filters are 2D only (biome, slope, water, river/lake) from the height and hydrology
  tiles, never neighbouring chunk contents.
- Candidate lists are cached by `(description hash, region)`, pure, listable in debug mode.
- Structures larger than 32 blocks are not v1.

### 10.2 Apply order and masks (H2.1–H2.4)

Stages, per section: **1 terrain → 2 ores → 3 boulders → 4 trees (logs then leaves) → 5 plants**.
Each type declares a write mask from a closed enum: `replace_host`, `air_only`, `air_or_leaf`,
`replace_surface`. No type may write water, bedrock, or air-to-solid outside its mask; the compiler
rejects contradictory masks. A bounding-box cull runs before any per-block test, and `Uniform`
sections are skipped.

Application is a pure function of `(description, section)` plus the 3×3×3 candidate lists, so
generation order never matters.

### 10.3 Ores (G3.1–G3.3)

Discovery per 32³ region and ore type: `hash64(seed, "ore:<name>", region)` drives a fixed-order RNG
that emits `N` candidates from a depth curve. Each candidate has a stable id, centre, ellipsoid or
short random-walk shape, and `ORE_MAX_EXTENT ≤ 8`. Apply tests membership in a fixed order and
replaces only host rock (stone family plus deep rock, by name). Never surface, bed, beach, snow,
water, air, or bedrock. Overlaps resolve to lowest id. Ores skip carved air, so caves expose veins.

### 10.4 Trees (H3.1–H3.6)

- Shape: data templates over a closed primitive set (`trunk`, `branch`, `canopy` with
  ellipsoid/cone/layered-disc, `root_flare`), with a per-tree variant from
  `hash64(seed, "tree:<species>", candidate_id)`.
- Leaves are placed only inside the canopy volume and within `LEAF_MAX_DIST = 4` of a trunk or
  branch, checked on the template's own parts. No world scan, no floating leaves.
- Spacing: jittered grid with `TREE_CELL = 6`, priority rejection against the 3×3 cells, ties broken
  by lower cell index, `MIN_DIST ≤ 1.5 * TREE_CELL`. Density changes acceptance only, never the
  grid. Spacing resolves before 2D height filters.
- Filters: not underwater, not a bed, not within 2 blocks of a bank, `slope_drop <= TREE_MAX_DROP`,
  biome allows the species, top span open to the sky.
- Trees never touch water: rejection also covers `TREE_WATER_MARGIN = 2` from any column with a
  non-empty water range. At apply, a target cell that is water is **skipped, not replaced**.
- Trunk base is sunk to the lowest of the four neighbouring tops and rejected below `SEA+1`.
- No growth simulation: no saplings, no ticks, no decay.

### 10.5 Small scatter (H4)

Grass, flowers, and snow caps are real blocks with a `render_shape` (`cube`, `cross_quad`,
`snow_layer`) and no collision. **v1 ships `core:snow` as a full opaque cube**; the `snow_layer`
step variant is deferred (§22). Placement is a keyed hash per column against a density curve plus a
low-frequency patch field, so flowers clump. Cross-quads never occlude, never block skylight, are not
solid for the span scan or `top_solid_y`, emit into the cutout pass with no greedy merging, and are
dropped at LOD1+. `replaceable = true` allows placing over them.

## 11. Lighting

### 11.1 Storage (L1.1, L2.2, L2.3)

Two 4-bit channels per block (sky, block), packed per `Allocated` section in the §4.1 index order.
`Uniform` sections store a constant with the slot: air = 15, solid = 0, and water allocates unless
the column's top-of-section value is known. Light is derived, recomputed on load, and never saved or
hashed.

Opacity and attenuation come from the manifest: air and cross-quads 0, leaves 1, opaque 15, water
`sky_atten = 1` and `block_opacity = 0`.

### 11.2 Skylight (L1.1, L1.2)

15-level BFS per column, all 12 sections in one job, seeded from `WORLD_HEIGHT-1`:

1. **Vertical seed:** 15 straight down to the first opaque block, using the cached top span so open
   air costs nothing per cell.
2. **Lateral BFS:** seeded only at sky/shadow boundary cells, `-1` per step, no loss for straight-down
   propagation of 15.

A column is lit only when it and its 8 neighbours are resolved; the border is read once into a
padded nibble scratch. Edits relight the touched section plus a 15-block halo and the 8-neighbour
border.

### 11.3 Block light (L2.1)

Same BFS, emitters seed at their emission. v1 emitters: **torch (14)** and **lava (15)**. Emission
is a manifest field `0..=15`, frozen per id. Water does not attenuate block light. Lava is not
generated by terrain in v1; it is placeable by tools and used by fixtures. Torches need a support
face and break with it in one edit batch.

### 11.4 Incremental repair (L3.3)

Repair MUST be bit-identical to a full recompute. The algorithm is the standard two-phase dark
removal then re-light, scoped to the edited section plus a 15-block halo for both channels, with the
vertical sky pass redone for the edited column. Each cell's value is a max over sources, so queue
order cannot change the result. CI fuzzes ≥1M random edits (including `Uniform ↔ Allocated`
promotion and border edits) and asserts byte equality; release samples 1 in 1000 repairs and
self-heals by recomputing the column on mismatch.

### 11.5 LOD light (N3.5)

Coarse sections store no light and run no BFS. Per face at mesh time: sky = 15 if the air-side cell
is open to the ceiling, else `15 - SHADE_PER_CELL * cells_below_first_opaque`, minus
`WATER_SKY_ATTEN` per water cell above. Block light 0, AO 0. A shader curve eases the LOD0/LOD1
step.

## 12. Meshing

### 12.1 Greedy rules (M1)

Merge key for opaque faces: `(block id, face direction, sky nibble, block nibble, packed AO)`.
Water merges on `(normal, sky, block)` with no AO; LOD merges on block id only.

AO is 4 corner values per face, 0..3, packed 2 bits each, computed from the padded scratch:
`if side1 && side2 { 0 } else { 3 - (side1 + side2 + corner) }`, counting only opaque full cubes.
Triangulation flips on the anisotropy test `ao00 + ao11 > ao01 + ao10`. Water and cross-quads skip
AO; LOD1+ uses identity AO.

Output order is deterministic: fixed face order and fixed per-section ranges (opaque, cutout,
water). The mesh is derived and never hashed, but ordering is stable for tests.

### 12.2 Water and transparency (M2)

Only water/air faces exist. Water/water never emits; water/solid never emits — the solid face is
drawn by the opaque mesher, which treats water as non-occluding. A 32-deep ocean therefore costs one
top face per surface cell, merged into one quad per all-ocean section. Side faces exist only where
water borders air sideways.

Cutout blocks (leaves, scatter) render as alpha-tested geometry in the cutout range, with no greedy
merging and two crossed quads for scatter. Cutout leaves never blend, so they write depth and sort
correctly.

### 12.3 Quad record (M3.1)

One `u64` per quad, vertex pulling, no index buffer:

```text
x:5  y:5  z:5      quad origin, section-local (0..31)
w1:5 h1:5          width-1, height-1 along the face tangents
normal:3           0..5 = +x -x +y -y +z -z (6 reserved for cross-quad)
ao:8               4 corners x 2 bits
sky:4 blk:4        light of the air-side cell
tile:16            texture-array layer
tint:4             biome tint index
```

`tile` is resolved from the face's own texture reference: top for `+y`, bottom for `-y`, side for the
four sides and cross quads (`ADR-0015`).

The vertex shader emits 6 vertices per quad from a corner table, offsets `+face` planes by 1 along
the normal, computes UV from local position with a repeat sampler, and derives the triangulation flip
from the AO values. The section origin is per draw or indirect data, never per quad. Separate quad
ranges per section cover opaque, cutout, and water. Any bit-layout change is a renderer format bump
only.

**T-junction overlap.** Greedy merging can put a long quad's edge across the vertex of two shorter
neighbours. The edges are collinear, but their rasterised edge equations differ, so a one-pixel gap
can open at some viewpoints and shows as a bright speck (sky through a wall). The vertex shader
therefore expands every quad by **0.001 blocks** in its tangent plane: `u = duv.x * (width +
0.002) - 0.001`, and the same for `v`. Coplanar overlap is benign because the depth test keeps the
first surface, and the texture UV is unchanged, so the 1/1000 stretch is invisible.

### 12.4 Meshes and arenas (M3.2, M3.4)

One mesh per section, **only if it emits at least one face**; a typical column has 2–5. Each mesh has
its own AABB and is sub-allocated inside a **per-column vertex arena**, so buffer objects stay at 1–2
per column while culling stays per section. Edits rebuild the touched section plus border-affected
neighbours, never the column. Uploads split per section under the frame byte cap.

Budget: ≤25,000 quads per emitting section, ≤2.5 MB per worst-case mesh, ≤3,000 LOD nodes globally.

## 13. LOD

### 13.1 Ladder and downsample (N1.1, N1.4, N3.1)

| Level | Blocks per cell | Section covers | Radius |
|---|---|---|---|
| LOD0 | 1 | 32³ | `VIEW_RADIUS = 16` columns |
| LOD1 | 2 | 64³ | 32 columns |
| LOD2 | 4 | 128³ | 48 columns |

Coarse sections use the same `Section` layout and index order. Downsample per 2×2×2 group, applied
level to level: if any cell is solid, take the **most frequent solid id** (ties: lowest id); else if
any is water, water; else air. Cross-quads are dropped; ores are dropped; logs and leaves use the
same rule.

LOD is derived, never saved, never hashed, never authority. Radii are clipped to the world:
`min(config radius, distance to boundary)`.

### 13.2 Selection and seams (N2, N3.2, N3.3)

Selection is distance-based with **2-column hysteresis**. A LOD(n+1) column draws only where its
covered LOD(n) columns are not resident-and-meshed, so there is no overlap and no gap. A skirt of
`SKIRT_DEPTH = 2` cells covers the lower-resolution edge. Skirt quads carry identity AO (all corners
3, i.e. fully open) and the coarse face's `sky`/`blk`/`tile`/`tint`; a flush edge emits no skirt, a
one-cell drop emits one cell, and a larger drop is capped at `SKIRT_DEPTH`.

Budget: `LOD_NODE_BUDGET = 3,000` emitting LOD sections (soft target ~2,000) at world centre.
Exceeding fails CI, never a runtime clamp; runtime may lower radii but never raise them past budget.

Popping: at each switch distance, the same pose one column either side MUST differ by ≤1.0% of
pixels (per-channel threshold >12/255), with fixed fog and time of day, on mountain, coast, and
forest seeds. Fixtures include a ridgeline, a coastline, a canopy edge, and a river crossing.

### 13.3 Invalidation (N4)

An edit bumps `section_rev` and marks covering LOD1/LOD2 sections dirty, coalesced with
`DEBOUNCE = 500 ms` (LOD1) and 2 s (LOD2, which reads the rebuilt LOD1 first). Targets: LOD1 ≤2 s
p95, LOD2 ≤10 s p95 on an idle pool, as class 4 work that never delays class 0/1. Until replacement
the old mesh keeps drawing; a stale-revision result is discarded and re-queued. Entering the LOD0
ring prioritises a dirty column's LOD as class 3.

## 14. Rendering

### 14.1 Pass order (O1.1–O1.4)

1. Opaque: depth write on, front-face culling.
2. Cutout: alpha test with discard, depth write on, culling off.
3. Depth copy after cutout (skipped when no water section is near).
4. Water: blended, depth test on, depth write off, culling off, far to near by column distance.
5. Particles and block-break effects: blended, depth test on, depth write off.
6. Overlay: block highlight and debug lines, depth tested, drawn after water so the highlight reads
   through the surface.
7. Fog/atmosphere: applied per pass in the fragment shader from one shared function, plus the
   underwater tint.
8. UI.

Draws are multi-draw-indirect from the per-column arena. LOD meshes use the same records and passes.

Culling is frustum-only: per column AABB first (tightened to the cached `top_solid_y` range), then
per section AABB against 6 planes using the conservative positive-vertex test, in camera-relative
coordinates. Budget <0.5 ms at `R = 16`, asserted by a benchmark. The visible-section list is the
single hook for future occlusion. No occlusion queries in v1.

### 14.2 Fog and edges (O3)

One pinned polynomial, no `exp`/`pow`. `FOG_START = 128`, `FOG_END = LOD2_RADIUS_BLOCKS - 64`
(1,472 default), both derived from the active LOD radii so a radius cut moves fog with it. Distance
is Euclidean from the camera-relative position, never depth, so every pass agrees. A mild height term
thickens fog in valleys and over the sea, capped so `f` stays in `[0,1]`. Colour is sky at the
view's horizon band. Far clip is `FOG_END + 32`. Water thickness tint is applied before fog.

Beyond the last column, a renderer-only flat water quad at `SEA` extends to the far clip and uses the
same fog, so the horizon never shows a cut.

### 14.3 Precision (O2)

Camera-relative from day one, no floating origin. World positions `i32`; camera and entities `f64`.
Per frame `cam_block = floor(cam_pos)` (`i64`) and `cam_frac = cam_pos - cam_block` (`f32` in
`[0,1)`). Per draw `delta = (section_origin - cam_block)` converted exactly to `f32` (< 2^24).
Shader: `pos = delta + local_pos - cam_frac`. The view matrix is rotation-only. Picking and
collision stay in world space. A unit test asserts exact round-trip up to max view distance, and a
fixture at ±2000 matches the origin fixture.

## 15. Streaming and scheduling

### 15.1 Ring (K1)

Square Chebyshev ring around the player's column: `max(|dx|,|dz|) <= VIEW_RADIUS` (16 → 1,089
columns). Load nearest-first by ring, then Euclidean distance, then column index. The **meshing ring
is `R-1`**, so the outer generated ring is never meshed without neighbours. Readiness requires the
column and its 8 neighbours resolved.

Unload at `R + 2` (≤1,369 resident), checked on column change, queued into an LRU with a ~5 s grace
period. `R+1..R+2` stay resident but unmeshed and unprioritised. Dirty columns are saved before
eviction. The memory cap evicts farthest-first regardless of margin.

### 15.2 Priorities and caps (K2)

Strict class order, distance within a class:

| Class | Contents | Cap |
|---|---|---|
| 0 urgent | player 3×3, pending edits, collision, player lighting | 64 |
| 1 visible | in frustum, inside `R-1` | 1,200 |
| 2 ring | in ring, outside frustum | 1,200 |
| 3 prefetch | ahead of movement, speed > threshold | 128 |
| 4 background | compaction, cache, LOD | 64 |

Requests are keyed `(coord, stage, revision)`, so a re-queue replaces. Classes 3/4 drop the farthest
on overflow; 1/2 drop farthest and count `dropped_before_start`, re-requested automatically; class 0
is never dropped or blocked and its overflow is `InvariantViolated`. One worker is reserved for
class 0. In-flight is one job per worker plus `GENERATE_AHEAD = 12` ready columns.

### 15.3 Cancellation and teleport (K3, K4.4)

Jobs run to completion (≤~10 ms); a result is applied only if `(coord, stage, revision)` still
matches, else discarded and its buffers freed. Counters: `dropped_before_start`,
`discarded_stale`, `applied`. Pure-function results (tiles, hydrology, candidates) are keyed
`(description hash, coord)` and go to the derived cache instead of being discarded.

On a column jump (>1 column), one idempotent pass: recompute membership; drop unstarted requests
beyond `R+2`; re-derive classes; rebuild class 0 first; **bump no revisions**; free resident columns
beyond `R+2` except dirty ones; evict LOD outside its radii; let in-flight jobs finish and discard
their results if stale; boost caps until the new 3×3 is resident; reset the lag timer; clear
prefetch. Input and gravity pause behind the loading cover until the 3×3 and its 8-neighbour
footprint are ready.

### 15.4 Budgets (K4, P1.2, P1.6)

Per frame at 16.6 ms: render CPU ≤10 ms; apply ≤3 ms; world/input/UI/physics/edits ≤1 ms; ~2.5 ms
slack never spent by design. GPU target ≤12 ms. Apply caps: 2 columns and 4 MiB, with
`APPLY_TIME_CAP = 3 ms` as a backstop. While the 3×3 is unresolved, apply may use 8 ms and 8
columns/16 MiB, with a 33 ms frame cap hidden by the cover.

Nothing blocks the main thread: no disk IO, no worker lock waits, no job waits, no GPU readbacks, no
fence waits except swapchain present. Results arrive on a bounded ready queue polled with `try_recv`.
A debug watchdog flags any main-thread call over 2 ms outside render submission and apply.

Edit latency: **p95 ≤50 ms, p99 ≤100 ms** from input to visible frame. The game thread writes the
block immediately; reserved edit workers relight (≤10 ms) then remesh (≤15 ms); upload ≤4 ms. The old
mesh keeps drawing until replacement; duplicate rebuilds coalesce. Bulk edits are batched with the
first visible change within 100 ms.

### 15.5 Degradation ladders (K4.3, S1.7)

**Lag ladder** (class 1 queue wait p95 > 500 ms, or ready queue empty with class 1 pending, sustained
2 s). One step per window: 1 background → 2 stop new LOD2 → 3 stop new LOD1 → 4 stop prefetch →
5 pull fog in and drop `VIEW_RADIUS` by 2 (min 8, log once). Never shed class 0 or 1. Restore in
reverse, one step per 10 s lag-free.

**Memory ladder** with `MEM_CAP_BYTES = min(60% RAM, 4 GiB)`, stopping at ≤90%: LOD2 → LOD1 →
derived caches → `R+1..R+2` and LRU columns (dirty saved first) → drop `VIEW_RADIUS` by 2 (min 8,
notice). Restore +2 only after ≤70% for 60 s. Never evict the player 3×3, pending edits, or in-flight
jobs. Both ladders share one effective radius.

### 15.6 Startup (P1.11)

Movement unlocks when the player's 3×3 is resolved, lit, meshed, and uploaded, and the 8 neighbours of
that 3×3 are generated (25 columns). Target ≤3 s on the reference machine; a cover shows progress and
fades over ~0.5 s. Spawn is chosen on the CPU from the `HeightTile`: land with `top_solid_y > SEA+1`,
not a bed, `slope_drop <= 1`, in deterministic spiral order within `SPAWN_SEARCH_RADIUS = 256`; no
valid land is `DescriptionInvalid`. Saved worlds use the saved position, clamped inside the edge.
`STARTUP_TIMEOUT = 20 s` shows the stuck stage and coord with Retry / Save and Quit. Input, gravity,
and entity simulation stay paused until unlock.

## 16. Persistence

> **Status.** The format is fully specified in [`docs/dev/save-format.md`](../dev/save-format.md)
> (header, journal, section record, backup rotation, migration journal). Code exists for the header,
> the journal framing/policy, and the section record. **No save writer ships in this build**: nothing
> writes or reads a section file, so "saved sections are the source of truth" is a design contract
> here, not a shipped behaviour. The first persistence milestone implements it without changing the
> layout; any byte change is a format version bump.

### 16.1 Saved sections (J4.1, J4.2, J4.5)

**Saved effective sections are the single source of truth. A saved column is never regenerated.**

- Every generated column is written as effective (post-edit) sections, using the §4.1 layout. On
  disk, palette entries are stable names (or a manifest-local remap table).
- Each section record carries a checksum (`crc32`), `baseline_hash` (decoded ids as
  first generated), and an `edited` bit.
- Columns never visited generate on demand from the description. There is no baseline cache. Only
  derived data (tiles, hydrology, candidates, light, meshes, LOD) is cacheable and disposable.
- Normal load never regenerates. An optional `verify` mode regenerates unedited sections and reports
  drift as "N sections differ".
- Header checksum is verified first.

### 16.2 Header (A1.2)

`header.bin`, fixed little-endian, written temp → fsync → rename → dir-fsync:

`magic "TWLD"`, `format_version: u16`, `generator_version: u16`, `description_hash: [u8; 32]`,
`hash_algorithm`, `seed: u64`, `platform_tag`, `last_saved_platform_tag`, `world_id` (128-bit random,
never reused; Fork assigns a new one), `name`, `created`, `last_saved`, `revision: u64`,
`schema_version`, `crc32`.

Load order: header crc → format support → description present and parses → recomputed hash matches →
generator version supported → manifest validation → journal recovery. Any failure aborts and leaves
the directory byte-identical.

### 16.3 Edit log and journal (J1, J2, J3)

Edit record:
`{x, y, z, old: name, new: name, revision: u64, author: u32, batch: u32}`. Stored with stable names.
One revision per committed batch; undo reverses a batch atomically. Append-only; coalescing happens on
read only. Highest `(revision, batch, seq)` wins; `author` is a defensive tie-break and an equal key is
`InvariantViolated`. **Saved sections are authority; the log is never replayed onto them** except
journal recovery, once, for records newer than the section's last save.

Journal framing: `[len u32][revision u64][payload][crc32]`, group-committed on a dedicated IO thread.
Flush every 250 ms or 64 batches; fsync on pause, quit, before any section save, and every 10 s. A
torn tail is truncated on recovery. Section save is temp → fsync → rename → dir-fsync, and the covered
journal prefix is dropped only after that.

### 16.4 Corruption (S1.5)

A corrupt section (crc, framing, palette name, or bit width) is a **hard load error**: abort, name
world/section/path/kind, exit to the world list, leave files untouched, and report all failures in one
pass. Recovery is an explicit opt-in `world-repair` (restore backup, quarantine to `.corrupt` and
regenerate as never-saved with an edit-count report, or salvage). One rolling backup generation per
section. Journal tail corruption is distinct and is truncated silently.

### 16.5 Migration (T1.3, T1.7, T1.8, J5)

Dialog only when the world cannot open as-is, no default button: **Pin / Migrate / Fork / Cancel**.

- Preflight: free space ≥ world + backup + 10%, verify every crc, exclusive world lock.
- Backup: copy (hardlinks do not count), verify by re-reading, never deleted by the tool.
- Migrate: sorted section order, per-section temp → fsync → rename → dir-fsync, then append to
  `migration.journal`. Never regenerates. A crash resumes idempotently (a section in either format is
  accepted by crc). Header carries `migration_state = InProgress`, which opens only the resume dialog.
- Completion: new description version and header with format/version/history, `migration_state`
  cleared. The edit log carries over unchanged; unmappable records are kept and flagged.
- Fork copies to a new `world_id` and migrates the copy; Pin writes nothing. Both are tested by
  comparing the original directory hash.
- Report `migration-report-<date>.json` is always written (even on failure/cancel): sections
  read/rewritten/unchanged; unknown/deprecated names with counts and aliases; edit-log entries and
  unmappable/conflicting edits with examples; `baseline_hash` drift; warnings; bytes and elapsed;
  backup path; tool and engine versions. CLI parity with `--dry-run`.

## 17. Failure and errors (S1)

One error type crosses every boundary:

```rust
struct TerrainError { stage: Stage, kind: ErrorKind, coord: Option<Coord>, context: Vec<Frame> }
```

`Stage` and `ErrorKind` are closed enums (`ErrorKind` `#[non_exhaustive]`) with typed payloads, never
strings as discriminants. Kinds include `CorruptSection{path,kind}`,
`DescriptionHashMismatch{expected,found}`, `DescriptionMissing{path}`, `DescriptionInvalid{key,rule}`,
`UnknownGeneratorVersion`, `NewerThanEngine`, `JournalTornTail`, `IoError{op,path,os_error}`,
`UnknownBlock{name,section}` (degrades), `ManifestIncompatible{id,reason}`,
`GraphInvalid{node,rule}`, `JobPanicked`, `JobOverdue`, `JobFailed`, `ResourceCap{class,bytes}`,
`DeviceLost{backend,adapter,driver,reason}`, `InvariantViolated{check,coord}`.

Every kind declares a fixed `Severity` (Fatal / SessionFailed / Recoverable / Degraded) and a
`Disposition` (RefuseLoad / FailJob / DegradeAndContinue / RetryOnce / Report) in one wildcard-free
table. Callers branch on those, never on message text. `thiserror` for impls; `anyhow` only in binary
`main()`.

Worker panics are caught at the job boundary (`panic = "unwind"`), logged with a repro record, and
retried once for classes 1/2; a second panic marks `(coord, stage)` **Failed for the session** — never
meshed, applied, or saved, so neighbours never treat it as resolved. Class 0 escalates immediately to a
blocking dialog. `panics_caught > 20 in 60 s` pauses new generation with a banner.

Stage deadlines are soft: `max(250 ms, 10 × p95)`, tracked by a watchdog. On expiry, log, mark
`Overdue`, bump the revision so the eventual result is stale, and re-queue once. A second expiry is
`Failed`. A worker still running after `HARD_STUCK = 60 s` is counted lost, a replacement is spawned,
and the lost thread is leaked with its memory accounted; `> 2` lost pauses generation and offers Save
and Quit. No stuck worker is ever killed.

GPU device loss is fatal for the session: stop rendering, freeze the sim, flush and fsync the journal,
let an in-flight section save finish, write the report bundle, show a blocking error, exit to the
world list (or stderr + exit 3). No device recreation, no re-upload, no software fallback in v1.

## 18. Observability (Q1)

Pinned counters, exported to Tracy when enabled and to an in-game HUD/overlay: per-stage latency
p50/p95, queue depth per class, in-flight jobs, resident/ready/queued columns, meshes and quads,
upload bytes, light repairs, edit latency, LOD counts, per-class memory, `dropped_before_start`,
`discarded_stale`, `applied`, `panics_caught`, `nonfinite_clamped`, `clamp_fired`.

Debug overlay, one indexed mode per number key: field probe, height (macro/detail/HeightTile/diff),
biome best/second/margin, fired surface rule, strata and fault, spans and mouths, light and AO toggle,
LOD level and coarse light, water quads/levels/outlets, revisions and queue state; F-keys for bounds,
memory, timelines, and culling counters. It reads CPU-side data only, labels anything approximate, and
costs nothing when off. A "copy report" key writes the §19 bundle.

Capture/replay: binary file with a versioned header + CRC containing the description hash, seed,
platform tag, camera path, edit log, settings, and stage timings. Replay regenerates from the
description, re-applies the log, and compares per-section hashes.

## 19. Tooling (Q1.9, Q1.8, Q1.10)

`terrain-gen` is a workspace binary depending only on generation/hashing/section/save-format crates
(no window, GPU, audio, or renderer; enforced by a crate-graph check). Subcommands: `gen`,
`golden check|update`, `seams`, `sweep`, `verify`, `replay --report`, `describe-diff`,
`world-migrate --dry-run`, `map`, `calibrate-blend`, and tile/height/biome/hydrology dumps. Options
`--threads N` (default 1) and `--order forward|reverse|shuffled`; output MUST be identical for any
value. Exit codes 0 ok / 1 test or invariant failure / 2 usage / 3 internal or panic.

Current report bundle: `terrain-gen verify --report <dir>` writes `manifest.json`, `verify.json`, and `verify.txt`. The full zip bundle (`description.toml`, `pack.lock`, `header.json`, `failure.json`, `stages.json`, `counters.json`, scrubbed `logs.txt`, `system.json`) is deferred.

Current `terrain-gen map` writes a deterministic ASCII surface grid over a sampled rectangle, with `--json` rows. The PNG modes (`height|biome|surface|rivers|climate|strata|caves|slope|lod`), `tEXt` metadata, legends, `--ndjson`, and the full zip report bundle are deferred; see the accepted limitations below.

## 20. Performance targets (P1, B5.5)

Reference machine: 2020 mid-range desktop, 1080p, 60 FPS, 4–6 cores, GTX 1660 / RX 5600 class.

| Metric | Target |
|---|---|
| Field tile, lattice stages | ≤4.6 ms cold |
| `HeightTile` | ≤8.5 ms cold |
| Tile bundle total | ≤13 ms cold, ≤0.8 ms/column amortised |
| Hydrology tile | ≤20 ms (excl. macro height); world mode ~150 ms once |
| Column build p95 | ocean ≤0.5 ms, plains ≤2 ms, mountain ≤6 ms |
| Light, full column | ≤1 ms p95 |
| Mesh | ≤0.5 ms per emitting section p95 |
| Edit latency | p95 ≤50 ms, p99 ≤100 ms |
| Frame | no frame >20 ms while walking; p99 ≤12 ms |
| Startup | 3×3 + footprint ready ≤3 s |
| Memory | ≤40 KB per typical column; cap `min(60% RAM, 4 GiB)` |
| LOD | LOD1 ≤2 s, LOD2 ≤10 s after edit; ≤3,000 nodes |

CI gates: PR tier uses deterministic counters and fails at >5%; nightly uses wall-clock on one pinned
machine, 5-run median, fails at >15%, reports >10% spread as unstable, and fails at >25% cumulative
drift over 30 days. Absolute budgets fail regardless of relative thresholds.

## 21. Testing and acceptance (R1)

Three tiers.

**Tier 1 — unit, every commit, <~60 s, no GPU:** graph compiler rejections; description
parse/emit/hash; monotone curves; `macro_cell`/`floor_div` at negatives and extremes; hydrology on
hand-made bowls, plains, and pits; D8 ties; lake gates and spill; U-profile and bed clamps; surface
precedence slots; `stone_at` agreement; feature spacing order independence; downsample ties; section
pack/unpack at every bit width; palette growth; edit-log batch undo. A pure stage without a tier-1
test fails a coverage lint.

**Tier 2 — property, fixed CI seeds plus nightly random, failing inputs committed:** fields in
`[0,1]` with a zero clamp counter; curves never overshoot; index/inverse round-trip; padded copy
equals `get_block`; shuffled order and 1 vs N workers give identical hashes; every non-sea cell has a
receiver and no cycles after fill; incremental light equals full recompute (1M edits nightly); greedy
meshing covers exactly the visible faces; noise within `NOISE_BOUND`.

**Tier 3 — golden, seam, and sweep.** ~40 branch-forcing fixtures (ocean, shelf, beach warm/cold,
plains, hills, peak near `MAX_TERRAIN_Y`, terrace wall, river narrow/wide/mouth, open/closed lake,
cave mouth, cheese, spaghetti, overhang, tree on slope, ore across a region border, biome dither, snow
line, deep-rock transition, tile-edge basin, LOD1/LOD2 coast and mountain), hashed per section, per
column, and per stage, stored per platform tag, regenerated only by the tool with a diff report.

Seams: exhaustive 5×5 windows of field tiles and columns, both axes, both directions, each side cold
in its own worker, asserting border samples, hydrology/basin outputs, biome arrays, candidate lists,
shared-section blocks, and the 1-block margins for slope/AO/light; random far pairs to the world edge
with negatives; and forward/reverse/shuffled plus 1-vs-N-worker runs.

Sweeps: PR 16 fixed seeds × 2 regions <3 min; nightly 64 rotating seeds × 3 regions plus negative
coordinates, world edges, and polar `z`. Invariants: nothing above `MAX_TERRAIN_Y`; `y=0` bedrock;
water only at `y<=SEA`, in a lake at its level, or in a river channel at its stepped level; every
river ends in sea or lake; no cave breach under `SEA+1`, rivers, lakes, or banks; no carved air
face-adjacent to water; one surface rule per column with no topsoil under water; all fields in
`[0,1]`; no leaf beyond `LEAF_MAX_DIST`; no tree writing water or bedrock; seam/order/worker equality;
two runs identical.

Migration fixtures per save version (unedited, edited, unmappable) with golden decoded-block hashes,
resume-after-kill, dry-run equality, and unchanged originals for Fork/Pin. Corruption fuzzing is
table-driven on header, sections, journal, description, `migration.journal`, and `pack.lock`,
asserting the exact `ErrorKind` and a byte-identical directory.

### 21.1 Definition of done (R1.13)

Every clause passes in CI or a scripted run at `VIEW_RADIUS = 16` on a fixed seed set.

*Content, asserted by queries:* every river reaches sea or lake; lakes at outlet level; ≥3 biomes per
seed each ≥5% of land; measurable lee/windward humidity contrast; strata on cliff and terrace faces;
cave mouths, overhangs, trees, scatter, and ores present and respecting write masks; finite edge with
deep-sea band, wall, and fogged horizon; edge-to-edge walk on both axes and a diagonal with no void
and no unloaded-ground fall.

*Correctness:* goldens, seams, and sweeps green per platform tag; identical output across worker
counts and orders; edits survive restart and a kill loses at most the journal window; corrupt section
refuses with its path; unknown id round-trips byte-identically; panic and hang injection pass; replay
reproduces hashes.

*Performance:* the table in §20.

### 21.2 Rejection criteria (R1.14)

Any one stops the approach rather than tuning it: (1) determinism cannot be held on one platform tag;
(2) edit p95 stays above 100 ms after the reserved-worker and section-scoped design; (3) 20 ms cannot
be met at radius 8; (4) hydrology seams keep failing after halo and basin fixes; (5) worst-biome
memory exceeds 2× the estimate; (6) the ring cannot fill at walking speed with the pool busy.
Fallbacks are named in the interview and must be adopted deliberately.

## 22. Non-goals and deferred work

Not in v1, with the reserved extension point where one exists:

| Deferred | Extension point |
|---|---|
| Hydraulic or thermal erosion, resistance | `heightfield_post` no-op (D5.1) |
| Water/cave repair | `cave_water_repair` no-op (D9) |
| Fault scarps, height deformation | Materials-only strata (G2) |
| Noodle caves, ravines, lava pools, aquifers | Cave graph subtree |
| Water table, springs, wells | None; would add a field |
| Seasonality, weather, growth, fluids | Runtime simulation through the edit path |
| Cross-platform bit equality | Generator version bump |
| GPU generation | CPU-only for block output (B5.3) |
| Occlusion culling | Visible-section list (O1.3) |
| In-engine terrain editor | Files + `describe-diff` (Q1.6) |
| Per-region parameters | World-level only (T1.9) |
| Structures larger than 32³ | Multi-region plan (H1.1) |
| `snow_layer` step geometry | v1 ships a full snow cube; the layer is a content + render-shape change |
| World-mode hydrology wiring | `hydro::WorldHydro` exists; switching the pipeline is a type change |

Also out of scope: multiplayer and server-authoritative terrain, scripting/modding API, SVDAG or
raymarched world models, isosurface meshing, and infinite ungenerated worlds.

## 23. Accepted limitations

No open design questions remain. Each item below is deliberate, stated where it belongs, and does not
contradict a MUST elsewhere in this document.

- **Hydrology is world mode** (§7.1): one solve over the 512×512-cell finite world, shared by every
  worker, so a basin or river that crosses a 512-block tile seam has one level. This is generator
  version 2. Tile mode remains for tiles outside the solved grid, where the 16-cell halo still
  applies.
- **Only generator version 3 ships.** No frozen evaluator for versions 1 or 2 is implemented yet,
  so a world pinned to an older version is refused rather than regenerated with different
  hydrology or biome blending.
- **Snow is a full cube** (§10.5); `snow_layer` step geometry is deferred (§22).
- **Cold beach uses sand** for both temperature variants (§8.5).
- **Persistence** (16): section files, one-generation backup rotation, the journal file, replay,
  prefix trimming, repair, and header migration are implemented in `src/save.rs` and
  `src/world_save.rs`. See [Save format](../dev/save-format.md). Section-record migration for an
  older record format has no normative format yet.
- **`TERRAIN_DEMO` is a demo, not the product**: a column-level streaming ring with real
  per-column lighting (sky, block, water attenuation), per-face texture references resolved to
  procedural layers, collision, edits, per-edit saves, distance LOD1, and residency-driven
  generate/build/evict decisions. The ring runs at the spec's 16-column view radius (1,089 wanted
  columns, ~126 MB of section bytes, ~590 MB peak process working set, ~45 s cold fill), so the
  memory margin against the 600 MB target is thin. LOD2 remains; the world model behind it is the
  shipped one.
- **`terrain-gen map` is ASCII-only** for now: `--seed --from --size --cell` prints a height/water grid and `--json` rows. PNG modes, `tEXt` metadata, legends, and `--ndjson` are deferred.
- **The full tooling bundle is deferred.** `verify --report <dir>` writes a manifest plus JSON/text; the zip bundle is not implemented. `calibrate-blend` and the PR counter gate are implemented.
- **Biome blend calibration passes and the dither is wired.** `terrain-gen calibrate-blend
  --seeds 16` assesses all five pairs and passes (medians 23.33-27.17, p10 14.69-18.76, p90
  32.55-63.32) after normalising the margin by its block-space gradient. Generator version 3
  calls `biome_pick` once per column for the surface rules and the feature footprint, so the
  measured transition is the shipped one. The command is diagnostic and is not a CI gate.
- **Section-record migration has no older format.** Version 2 records are canonicalised and headers upgrade v1 to v2; there is no normative v1 section record to decode or migrate.
- **Camera-relative rendering is applied; the CPU camera is still `f32`** (§14.3). Both draw paths
  store absolute world-block origins as integers and subtract the frame's render origin in the
  shader, casting only the small relative delta to `f32`; the view matrix uses the camera's offset
  from the same origin, and fog and culling use that relative frame while picking, collision, and
  streaming stay world-space. Crossing a block rewrites no origin uniform. A precision test at
  2^20 blocks covers both paths. The spec's `f64` camera and entity positions are not implemented,
  so very large worlds still depend on the `f32` camera's block-aligned precision; see
  [Terrain v1 runtime](../dev/terrain-v1.md#camera-relative-rendering).
- **The headless tool is split out of the engine crate.** `terrain-gen` is a binary of the
  `terrain-core` library, which has no window, GPU, audio, or renderer dependency. CI enforces
  this by checking that `cargo tree -p terrain-core -e normal --no-default-features` resolves no
  `wgpu`, `winit`, `wgpu-profiler`, or `tracy-client` (§19).
- **`pipeline::AIR`/`WATER`** remain as tool-facing constants, pinned by a test; generation reads ids
  from `TerrainMaterials` (§2.6).

Deferred features and their extension points are listed in §22.
