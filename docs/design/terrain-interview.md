# Terrain interview

**Status:** In progress. Working document, not a spec.

**Kind:** Question checklist. It records what must be decided before the terrain track becomes
a spec. Answers land in the log at the bottom and in
[Terrain pipeline](terrain-pipeline.md#interview-decisions). This page never overrides a spec,
an ADR, or the executable source.

## How this page works

- Every domain is a numbered task. Every task has subtasks. Every subtask has questions.
- A question is answered only when the answer is unambiguous enough to implement and test.
- Answers are written in the log, dated by round. A checked box means "answered", not
  "implemented".
- A decision that changes world output is a generator version bump. A decision that changes
  only performance does not.
- Three outcomes are always allowed: accept the cost, defer with an explicit trigger, or
  reject with a reason. "We will figure it out in code" is not an answer.

## Coverage map

| Domain | Task | Questions | Answered |
|---|---|---|---|
| A | Foundations and identity | 25 | 25 |
| B | Field graph and sampling | 31 | 31 |
| C | Macro landform | 21 | 21 |
| D | Erosion and hydrology | 29 | 29 |
| E | Climate and biomes | 22 | 22 |
| F | Volume synthesis | 17 | 17 |
| G | Geology and materials | 12 | 12 |
| H | Features and scatter | 17 | 17 |
| I | Chunk data model | 12 | 12 |
| J | Overlay and persistence | 20 | 19 |
| K | Streaming and residency | 17 | 17 |
| L | Lighting | 14 | 14 |
| M | Meshing | 12 | 12 |
| N | LOD hierarchy | 16 | 16 |
| O | Rendering and precision | 10 | 10 |
| P | Performance and budgets | 12 | 12 |
| Q | Observability and tooling | 11 | 11 |
| R | Testing and acceptance | 14 | 14 |
| S | Failure and robustness | 10 | 10 |
| T | Versioning and migration | 12 | 12 |
| **Total** | | **334** | **334** |

---

## A. Foundations and identity

### A0 Order
- [x] A0.1 Does the versioned description ship before the first generated chunk?
      **Yes.** No world exists without a generator version; no generate-first retrofit.

### A1 World description
- [x] A1.1 Is the description one TOML/JSON file, a binary header, or both?
      **One `description.toml` per world**, human-readable and diffable. Identity is the
      **structural hash** (parse → typed `Description` → canonical bytes → pinned hash); file
      text never affects it. Typed parse: unknown keys, duplicate keys, and missing keys are hard
      errors (`deny_unknown_fields`, no silent defaults). Numbers: integers plain decimal, `f32`
      as bit-pattern hex strings with a decimal comment. NaN/inf rejected, `-0.0` normalised.
      Canonical encoding: struct field order, little-endian fixed-width integers, length-prefixed
      strings/arrays, maps sorted, leading `schema_version: u16`. Graph and content pack are
      embedded or referenced by `(name, record_version, hash)` so the hash covers content, not
      paths. **Seed and platform tag are not in the hash.** Loader recomputes and refuses a
      mismatch. Hand edits are allowed but produce a new description version; the tool never
      rewrites in place. Tests: parse→emit→parse→hash equality; whitespace/key-order changes leave
      the hash unchanged; any value change alters it; unknown/duplicate/missing keys rejected.
- [x] A1.3 Is the parameter set inline, a named preset plus overrides, or a hash only?
      **Inline and self-contained.** Every parameter is an explicit required key (`WORLD_HEIGHT`,
      `SEA`, `MAX_TERRAIN_Y`, `DEEP_Y`, `MACRO_CELL`, `TILE_BLOCKS`, `RIVER_MIN_CELLS`,
      `LAKE_*`, `CAVE_*`, `ENTRANCE_*`, `DETAIL_*`, `WATER_SKY_ATTEN`, `LAPSE`, `T_SNOW`,
      `BLEND_BLOCKS`, `STRATA_*`, `FAULT_*`, `ORE_REGION`, `TREE_CELL`, `Z_EQUATOR`,
      `Z_HALF_SPAN`, `EDGE_BAND`, …). Presets are authoring templates that expand to a full
      inline description; the preset name and tool version live in an **unhashed `[meta]`**
      table. Renderer/streaming/budget settings (`VIEW_RADIUS`, LOD radii, `MEM_CAP`, worker
      counts, fog, flush intervals) are **runtime config, never identity**. Schema marks each key
      hashed or provenance-only; CI fails if a generation constant exists in Rust without a
      schema key. A `describe-diff` tool lists changed keys and whether they change identity.
- [x] A1.2 What are the mandatory header fields?
      **`header.bin`** (fixed LE, versioned, crc32 last, written temp→fsync→rename→dir-fsync):
      `magic "TWLD"`, `format_version: u16`, `generator_version: u16`,
      `description_hash: [u8;16]`, `hash_algorithm "xxh3-128/v1"`, `seed: u64`,
      `platform_tag` + `last_saved_platform_tag`, `world_id` (128-bit random, never reused; Fork
      assigns a new one), `name` (display only), `created`/`last_saved` (UTC, never ordering),
      `revision: u64`, `schema_version`, `crc32`. The TOML description stays separate. Never stores
      derived state, runtime config, or per-section data.
- [x] A1.5 What does a missing or malformed description do?
      **Refuse; never auto-create.** Load order: header crc → format support → description present
      and parses → recomputed hash matches → generator version supported → manifest validation →
      journal recovery. Missing → `DescriptionMissing{path}` naming path and expected hash;
      malformed → `DescriptionInvalid{key,rule}`; hand-edited hashed value →
      `DescriptionHashMismatch`. Directory stays byte-identical. **"Open as viewer"** is an explicit
      refusal-dialog choice: saved sections read-only, no generation (unvisited = sea fog), no edits,
      no journal, no saves; it cannot upgrade itself. `world-repair` may restore the file from a
      backup or accept a supplied file whose hash matches; it cannot synthesise one.
- [x] A1.4 Is `u64` the final seed width, or is a 128-bit seed needed to make worlds
      unguessable and collision-free?
      **u64 stays.** Stream seeds come from hashing `(seed, stream name, coordinate)`.

### A2 Determinism boundary
- [x] A2.1 Is the promise same-binary-only, or cross-platform bit equality?
      **Same machine, same binary.** Two platforms may differ; no fixed-point requirement.
- [x] A2.2 If cross-platform: fixed-point fields, quantised `f32`, or a recorded baseline?
      **Not needed.** Revisit only if world sharing across platforms becomes a feature.
- [x] A2.5 **Conflict to resolve.** Round 3 asked for "bit-identical across platforms and
      compiler versions" in the graph answer, which is stricter than A2.1. Either A2.1
      relaxes to a per-platform reference hash, or the graph must ban the ops that are not
      bit-stable. Cannot hold both.
      **Resolved: per-machine wins.** Bit-identical for a given graph, seed, platform, and
      build config. Linux == Windows is not promised. The graph compiler pins evaluation and
      float op order so refactors do not drift. Field maths may use IEEE-754 basic ops and
      `sqrt` (correctly rounded); `sin`, `cos`, `pow`, `exp`, `ln` stay out of the noise layer
      when a polynomial or table works. No fast-math; no unpinned FMA contraction. Integer
      hashing of `(seed, coord)` is exact and platform-independent. Goldens are tagged by
      target triple plus build profile; a world file records the platform tag it was generated
      on. Saved chunks are the source of truth, so a cross-platform load is always valid;
      only regeneration of unsaved chunks may differ. Cross-platform equality is a later
      project behind a version bump.
- [x] A2.6 Is a platform tag written into every generated chunk, or only into the world?
      **World-level tag.** It names the platform of first generation, not a per-chunk origin.
- [x] A2.3 Which operations are banned because they are not bit-stable?
      **Banned in field maths:** `sin`, `cos`, `tan`, `atan2`, `pow`, `exp`, `ln`, `log`,
      `mul_add`/FMA, fast-math, float reductions whose order is not fixed (SIMD horizontal sums,
      `sum()` over unordered data), `HashMap` iteration order, and NaN/inf (rejected at load).
      **Allowed:** `+ - * /`, `sqrt`, `abs`, `min`, `max`, `floor`, `ceil`, `clamp`, comparisons,
      explicit casts. Enforced by clippy `disallowed_methods` plus a CI grep over the noise and
      graph modules.
- [x] A2.4 Is the simulation tick tied to generation? Does generation ever read time?
      **No.** Generation is a pure function of `(description, seed, generator_version, coord)` — no
      clock, tick, thread/process id, address, env, locale, fs, network, **and not the edit revision**
      (so edits can never change unvisited terrain). Time is allowed only in logs/metrics,
      scheduling/deadlines, LOD/fog selection, render animation, header timestamps, and benchmarks,
      and never flows back into a block, field, hash, or cache key. Seasonal/day-night looks are
      renderer-side; anything that changes blocks goes through the edit path. Enforced by import
      lints (`std::time`, `chrono`, `thread_rng`, `env`, `fs`), by entry points that take only pure
      inputs, and by a mocked-clock determinism test.

### A3 Keyed randomness

> **Implementation note (branch `terrain/build-v1`).** Wave 0 pinned two deviations because the
> repository forbids new dependencies without an ADR, and no world has shipped: identity hashes are
> **SHA-256 implemented in-tree** (`src/hash.rs`) with a **`[u8; 32]`** digest, and stream/per-sample
> hashing is a **SplitMix64** finaliser chain. `xxh3-128` and `[u8; 16]` below are the interview's
> original proposal; the [spec §2.2](../specs/terrain.md) and the build plan carry the pinned
> contract. Replacing either is an ADR plus a generator and save-format bump.

- [x] A3.1 What hash function addresses a named stream? Is it fixed forever once shipped?
      **Three pinned roles.**
      1. *Content/identity* (`description`, graph, manifest, `baseline_hash`, goldens):
         **xxh3-128** (`xxhash-rust`, default secret, seed 0) over canonical bytes, stored as
         16 bytes (see A3.5 for the 32-byte field question).
      2. *Stream seeds*: `hash64 = low 64 bits of xxh3-128` over
         `[seed u64 LE][generator_version u16 LE][stream_id u16 LE][coords i32 LE …]`, built in a
         stack buffer, no allocation. `std::hash`/`DefaultHasher` never feed output.
      3. *Per-sample integer hash* (noise lattice, dither, jitter, `hash01`): hand-written
         splitmix64-style mixer, integer-only, spec'd with test vectors;
         `hash01 = (mix >> 40) as f32 * 2^-24`.
      `xxhash-rust` version is locked; golden vectors over fixed byte strings (empty, short,
      1 KB, 1 MB) fail CI if a crate upgrade changes output. Algorithm changes are a generator
      **and** save-format bump; the header records `"xxh3-128/v1"`.
- [x] A3.2 How is a stream name built? Are coordinates folded as bytes, integers, or both?
      **Names resolve to `u16` ids at compile time** from a checked-in `stream_vocab.toml`
      (`{id, name, since_version, deprecated}`); ids are never reused or renumbered. Rust call sites
      use generated constants (`stream::BIOME_DITHER`), so a typo is a compile error; an unknown
      name is `DescriptionInvalid`. Parameterised families (`ore:<name>`, `tree_pos:<group>`) are
      `(family_id, member_index)` resolved at content-pack compile time, with the
      `(name → member_index)` map inside the description hash. The vocabulary's used pairs are
      hashed into the description hash; CI fails on any removed/renumbered id. Debug views map ids
      back to names.
- [x] A3.5 **Closed: all hashes are `[u8; 16]`.** `xxh3-128` output, high 64 bits first, each as
      `u64` big-endian, so the hex reads like the canonical digest. Applies to `description_hash`,
      graph, manifest, `baseline_hash`, per-stage and golden hashes, and the stream vocabulary.
      **T1.2's `[u8; 32]` is corrected to `[u8; 16]`.** The header stores
      `hash_algorithm = "xxh3-128/v1"` beside every hash. Text form is 32 lowercase hex chars
      (filenames, logs, TOML, JSON); any other length or uppercase is rejected. 64-bit truncation is
      allowed only for in-memory map keys. Golden vectors per role plus bytes→hex→bytes round-trip.
      A prototype world with 32-byte fields is refused as `BadVersion`; no shipped artefact exists.
- [x] A3.3 Does every consumer get an independent stream, or do siblings share and offset?
      **Independent and stateless.** A draw is a pure function of
      `(seed, generator_version, stream_id, coords [, sub_index])`. Multiple values at one
      coordinate use a stack-local `LocalRng` seeded once from `hash64(...)` and consumed in a
      **fixed, golden-tested order**; adding a draw to the end is a version bump, and new quantities
      get their own stream or `sub_index` rather than extending a shared sequence. `LocalRng` is
      never shared, cached, or seeded from a clock/thread/address.
- [x] A3.4 Can a stream be exhausted? What happens at 2^64 draws?
      **No exhaustion for stateless draws.** `LocalRng` is capped at `MAX_DRAWS_PER_KEY = 64`
      (debug assert + counter); rejection loops have a fixed iteration bound (~8) and a
      deterministic fallback, so work per key is bounded. `rand`/OS entropy, `HashMap` order, and
      cross-draw float accumulation are lint-forbidden in generation crates.

### A4 Coordinates and units
- [x] A4.1 Are negative coordinates floored or truncated when converting world to chunk?
      **Floored, always** (`floor_div`/`rem_euclid`, never `/` or `%` on signed ints) — stated in
      B4.5, I1.2, A4.2. Confirm and remove from the open list.
- [x] A4.2 Is the chunk key `(i32, i32, i32)` or a packed integer?
      **Explicit types, never packed.** `ColumnKey{cx,cz}`, `SectionKey{col, sy: u8}` (checked
      `0..12`), `LodKey{level: u8, col}`, `RegionKey{rx,ry,rz}`. Derive `Copy, Eq, Ord` (field
      order, so iteration is deterministic) and `Hash` **for in-memory maps only**; derived hashing
      never feeds output. Hash inputs are serialised explicitly as `i32` LE fields in declared
      order. Runtime maps use a non-randomised hasher; output paths use `BTreeMap`/sorted `Vec`.
      `from_block`/`origin_block`/`neighbours8` live in one place; nothing computes keys inline.
      Saves are per column, sections are slots; region files on a 16×16 Column grid with decimal
      text names; on-disk keys are `i32` LE + `u8`, versioned; the edit log stores world block
      coords so key layout changes never rewrite it. Tests: negatives, multiples of 32, round-trip,
      total order, `sy` rejection, golden serialised bytes.
- [x] A4.3 Is vertical order Y-up confirmed for every stage, including save and meshing?
      **Right-handed, Y-up.** `+x` east, `+y` up, `-z` north, `+z` south; looking down `-z` with
      `+y` up, `+x` is right. Compass labels are display-only. `WIND_DIR = +x` blows east.
      `Z_EQUATOR`/`Z_HALF_SPAN` are in `z` blocks, symmetric.
- [x] A4.4 Is one block one metre everywhere, including camera, physics, and noise?
      **Yes, with one conversion point.** World coords `i32`; camera/entities/physics/audio `f64`
      metres, same origin and axes (block `(x,y,z)` spans `[x,x+1)³`, centre `+0.5`). Noise/graph
      frequencies are **cycles per block** (`1/256` = one feature per 256 blocks), amplitudes in
      blocks, velocity `m/s`, time seconds, radians internally. Face normal order
      `+x -x +y -y +z -z`, CCW from outside, texture `v` down on sides. The only `f64 → f32`
      conversion is the render boundary (O2.1). Tests: 32-block mesh spans 32.0 m, block centre
      reconstructs, `x cross y = z`, spectrum check on `freq 1/256`, compass labels lint-forbidden
      in generation.

### A5 Content model

> **Implementation note (branch `terrain/build-v1`).** Runtime ids come from the name sort, so
> `core:water` is not id 2. Only names are persisted or hashed, so the numeric slots below are not
> part of the contract; the spec records this in §2.6 and §0.1. `core:snow` currently ships as a
> full opaque cube rather than a `snow_layer` block; the spec lists that gap as open.

- [x] A5.1 Are block IDs stable integers with a registry, or packed names?
      **Stable names persisted; runtime `BlockId: u16` assigned at load**, sorted by name so one
      pack yields the same ids on every run. Ids are never saved, hashed, or reported. Names are
      `namespace:name`, lowercase `[a-z0-9_]`, ≤64 bytes, immutable once shipped; removal is
      deprecation. Reserved: `0 core:air`, `1 core:bedrock`, `2 core:water`, plus the placeholder
      range from `UNKNOWN_BASE` (T1.6). A pack missing the three core names fails validation.
- [x] A5.2 Where do block properties live: code, content file, or both? Which is authority?
      **The content pack is the sole authority**, per name, frozen per id: opacity, `sky_atten`,
      emission, `render_shape`, `render_layer`, collision, `replaceable`, liquid, hardness, drops,
      tint, texture refs, tags. Compiled at load into flat arrays indexed by `BlockId`, so hot paths
      are one array read. Code attaches behaviour by tag or by a startup-resolved handle
      (`blocks::TORCH`), never by number or per-block string compare. Generation-relevant
      properties are in the description hash; render-only ones are marked non-generation so
      retexturing never forks identity.
- [x] A5.3 Is the content hash over the file bytes, the parsed definitions, or both?
      **Per-record hash over canonical parsed bytes**, then `pack_hash` over the sorted
      `(kind, name, record_version, record_hash)` list. Comments, whitespace, key order, and file
      names never enter it. The description pins the `(name, record_version, record_hash)` of every
      generation-relevant record, so the description hash covers content, not paths. Render-only
      fields go into a separate `render_hash`. A generated `pack.lock` is checked in; the loader
      recomputes and fails on drift.
- [x] A5.4 Can a world load with a subset of its content pack, and how is that reported?
      **Two levels.** To *generate*, every pinned record must match exactly — missing, changed, or
      different `record_version` is `ManifestIncompatible{id,reason}` and generation refuses; extra
      unpinned records are ignored (superset is fine). To *open and play saved terrain*, a valid
      header/description/journal is enough, and saved names missing from the pack degrade to
      placeholders (T1.6). A world never generates against a subset: it offers the explicit viewer
      or Pin/Migrate/Fork. Aliases resolve on load for blocks only, never for generation records.

---

## B. Field graph and sampling

### B1 Sources
- [x] B1.1 Which noise algorithms are allowed: OpenSimplex2, OpenSimplex2S, Perlin, value,
      cellular, voronoi? Which are banned and why?
      **OpenSimplex2 only, per ADR-0012.** Variety comes from composition: fBm, ridged,
      billow, warp, terrace, curve. Noise lives behind a registry keyed by algorithm ID with
      exactly one entry. A new algorithm needs a version bump, a bit-stability golden test,
      and a benchmark entry.
- [x] B1.2 Is noise normalised to `[0, 1]` at the source, or `[-1, 1]` with an explicit
      mapping?
      **`[0,1]` everywhere.** One helper at the source: `unit(raw) = clamp(raw*0.5+0.5, 0, 1)`.
      Neutral is `0.5`. Nothing downstream renormalises. Warp and ridged run on raw signed
      values *inside* the noise layer, before `unit()`. 3D density and caves are also `[0,1]`;
      carve is `density > CAVE_T` with `CAVE_T` a `[0,1]` constant. Tests assert the range.
- [x] B1.5 What is the exact `raw` bound per fractal type? `fBm` of OpenSimplex2 is not
      strictly `[-1,1]`, so where is the divisor applied: sum of amplitudes, per-octave
      clamp, or a measured scale constant?
      **Answered as design, one assertion still open.** `raw = (Σ a_i n_i) / (Σ a_i)`, divisor
      computed at compile time and stored as a constant; ridged/billow transform per octave
      then normalise. The claim `|raw| <= 1` relies on each OpenSimplex2 octave being inside
      `[-1,1]`. That is *not* guaranteed by the algorithm — it must be measured or clamped.
      See B1.7.
- [x] B1.7 How is the per-octave bound enforced: measured max over a large sample stored as a
      scale constant, a per-octave clamp, or rely on `unit()`'s clamp plus a range fuzz test?
      **Measured constant with a 2% margin.** Offline sweep (hundreds of millions of samples,
      seeds, 2D and 3D, plus adversarial extreme search) gives `NOISE_BOUND_2D/3D =
      max*1.02`. Base output scales by `1/NOISE_BOUND`, so every octave is in `[-1,1]` by
      construction. Constants and sweep method live in the spec; a golden test pins noise bits.
      Debug asserts per octave; CI fuzz fails above the bound; `unit()` clamp is a backstop
      that should never fire. Changing the noise impl, hash, or gradient table means re-measure,
      new constant, version bump.
- [x] B1.6 Are frequency, gain, lacunarity per field or per world, and are they stored as
      decimal text or `f32` bits in the pinned graph?
      **Per node, required, no defaults, `f32` bits.** `noise`: `freq`, `stream_id`, optional
      `offset`; `fbm|ridged|billow`: `freq`, `octaves: u8`, `gain`, `lacunarity`, `stream_id`;
      `warp`: `freq`, `amp_blocks`, `stream_id_a/b`. Octave `i` uses `sub_index = i` and frequency
      `freq * lacunarity^i`, amplitude `gain^i`, computed by the compiler by repeated
      multiplication (no `pow`) and stored as per-octave constants; the divisor is the exact
      amplitude sum (B1.5). Compiler rejects `octaves ∉ 1..=8`, `gain ∉ (0,1]`,
      `lacunarity ∉ [1.5,3.0]`, non-finite or non-positive `freq`, finest wavelength < 2 blocks
      (< 8 for macro/lattice nodes). All values are in the graph hash.
- [x] B1.4 Is fractal type (fBm, ridged, billow, ping-pong) a source property or a transform?
      **Separate nodes** (`fbm`, `ridged`, `billow`), so the type is part of the graph, not a flag.
- [x] B1.3 Are octave count, lacunarity, gain, and frequency per field or per world?
      **Per node.** Each graph node carries its own and is hashed into the world identity.
- [x] B1.4 Is fractal type (fBm, ridged, billow, ping-pong) a source property or a transform?
      Implied: separate nodes, so a transform.

### B2 Transforms
- [x] B2.1 Which domain warps are supported, and how many octaves does a warp cost?
      **One 2D warp node, 2 noise taps**: `warp(field, seed_a, seed_b, freq, amp_blocks)`
      offsets `x`/`z` by one OpenSimplex2 octave each, then samples the input at the warped
      position. Offsets are bounded by `±amp`, which the compiler uses to grow the tile halo:
      `halo = ceil(amp / MACRO_CELL) + 1`, max over the graph. At most one warp per dependency
      path; chained warps are rejected. 2D macro fields only, at lattice points (1/16 the
      per-block cost). 3D cave density gets no warp in v1.
- [x] B2.2 Are curves splines, piecewise linear, or an expression language?
      **Fritsch–Carlson monotone cubic**, compiled to per-segment coefficients. Control points
      `(x,y)` with strictly increasing `x`; the compiler rejects non-increasing `x`, out-of-range
      `y`, or fewer than 2 points. Output stays in `[min y, max y]` by construction. Input is
      clamped to the end points. Linear scan to ~8 points, binary search beyond. Horner form,
      no `mul_add`. A per-curve `linear` flag covers terraces and hard cliffs; flat segments get
      zero tangents.
- [x] B2.3 Is `min`/`max`/`clamp` enough, or are smooth-min and terrace needed?
      **`clamp` plus `terrace`** are in the vocabulary. `smooth_min` is not in v1.
- [x] B2.4 Does a transform operate on a scalar, a 2D field, or a sampled tile?
      **Per scalar sample** on the node's declared lattice; no transform owns a tile.

### B3 Composition
- [x] B3.1 Is the graph declarative data, fixed code, or a compiled closure chain?
      **Data graph, compiled once per world** into a flat topologically ordered evaluator with
      preallocated slots. No per-sample interpretation.
- [x] B3.5 What is the node vocabulary?
      Closed Rust enum: `noise(OpenSimplex2)`, `fbm`, `ridged`, `billow`, `warp`, `terrace`,
      `curve`, `remap`, `add`, `mul`, `lerp`, `clamp`, `unit`, `const`, `input(pos)`.
      Adding a node type is a code change and a version bump with a golden test. Editing
      graph data is not.
- [x] B3.6 How is the graph pinned to the world?
      Canonical serialisation is hashed and stored in the world description. Compile rejects
      cycles, unknown nodes, missing outputs, and any field that can leave `[0,1]` after
      `unit()`. Evaluation order and float op order are fixed by the compiler.
- [x] B3.7 Does every node declare its lattice?
      **Yes.** Macro nodes evaluate on the 4-block lattice, per-block nodes at block
      resolution. Debug views can show any node output.
- [x] B3.2 How is evaluation order pinned? Is it topological, declaration order, or both?
      **Topological, ties broken by node ID.** Canonical form also fixes key order and
      normalises `-0.0`.
- [x] B3.8 How are graph constants serialised?
      **Exact `f32` bits as `u32` hex** plus a decimal comment for humans. The hash covers
      bits only — never comments, whitespace, or key order. Integers are plain decimal.
      NaN/inf are rejected at load. `-0.0` normalises to `+0.0` before hashing. Hash algorithm
      is pinned (xxh3-128 or SHA-256) over canonical bytes; the loader recomputes and rejects
      a mismatch with the world description. A comment that disagrees with its bits is a lint
      warning, never a silent fix.
- [x] B3.2 How is evaluation order pinned? Is it topological, declaration order, or both?
      **Topological, ties broken by node ID.** Canonical form also fixes key order and
      normalises `-0.0`.
- [x] B3.3 Are intermediate values cached per tile?
      **No.** One flat topologically ordered program, each node computed once per sample, fan-out
      read from its slot, slots allocated by a compile-time liveness pass (peak live set, not node
      count). Tiles store only named outputs; intermediates never leave the evaluator and never
      enter a cache key.
- [x] B3.4 What is the maximum graph depth and node count for the first world?
      **nodes ≤128** (lattice-2D ≤64, block-2D ≤32, 3D ≤24), **outputs ≤12**, **depth ≤16**,
      **fan-in ≤4**, curve points ≤24 each / ≤64 total, plus the tap budgets (`CAVE_TAP_BUDGET = 4`,
      `DETAIL_TAP_BUDGET = 2`, one warp per path). All are world-description constants, enforced at
      compile with `GraphInvalid{node,rule}`; the compiler prints a weighted cost report per class,
      and a benchmark asserts measured cost is within 25% of it. Exceeding a limit refuses to
      compile, never degrades.

### B4 Tiling and sampling
- [x] B4.1 What is the canonical sample grid: per block, per 4-block macro cell, or per LOD
      sample?
      **4-block lattice for macro nodes, bilinear to blocks, then per-block detail.**
      Distance-dependent re-sampling is not used; one grid keeps LOD and collision consistent.
- [x] B4.2 Does a tile include a border, and how wide is it per consumer?
      **128×128-block tile** (32×32 cells, 33×33 samples) plus a 1-sample border on all sides
      (35×35 stored). Tile `(tx,tz)` owns blocks `128tx..128tx+127` via `floor_div`. Border
      samples are bit-identical to the neighbour's interior samples because they are the same
      global lattice. Tiles store lattice samples only, never interpolated values. `TILE_BLOCKS`
      must be a multiple of both `MACRO_CELL` and `SECTION_SIZE`, so a tile is 4×4 chunk
      columns. Tiles are pure functions of `(description, tx, tz)`; LRU keyed by
      `(description hash, tx, tz)`.
- [x] B4.3 Is a sample at integer coordinates taken at the block centre, corner, or corner
      plus half?
      **Block centre:** block `(x,z)` samples `(x+0.5, z+0.5)`. 3D density uses the same
      centre convention so caves align with blocks.
- [x] B4.5 Where does the 4-block lattice sit relative to block centres?
      **Samples at `4k+0.5`**, the centre of block `4k`. Block `x` is in cell
      `k = floor_div(x,4)` with weight `rem_euclid(x,4)/4`, interpolated between `k` and
      `k+1`. `floor_div`/`rem_euclid` only — never `/` or `%` on signed ints. Same rule for
      `z` and for `y` if a macro field is 3D. `MACRO_CELL = 4` is defined once;
      `macro_cell(x) -> (k, w)` is the only place the maths lives. The lattice is global in
      world coords, never chunk-anchored, so chunks generate independently. Weights are
      exact quarters, so interpolation is exact in `f32`.
- [x] B4.4 Is sampling separable (evaluate x, z, y independently) or fully 3D?
      **No separability analysis.** Every node declares a **domain** (`D2(x,z)`, `D3(x,y,z)`) and a
      **class** (`lattice2D`, `block2D`, `lattice3D`, `detail`). A node may read only the same or a
      coarser class; a `D2` node cannot read `D3`. Interpolation happens exactly once, at the class
      boundary, on `f32` before any quantisation; nonlinear nodes (curves, terrace, thresholds) run
      **after** interpolation unless declared lattice-resolved. `D3` evaluation hoists `D2` values
      per column. Taps are counted per domain/class; debug mode 1 labels each node.
- [x] B5.5 Cost per sample?
      **Ceilings:** `NS_PER_TAP_2D ≤ 100 ns`, `NS_PER_TAP_3D ≤ 150 ns`, cold, single worker, scalar
      `f32`, no FMA. `stage_budget = samples × taps × NS_PER_TAP × 1.25`. Targets: field tile
      lattice stages ≤4.6 ms; `HeightTile` ≤8.5 ms; tile bundle ≤13 ms cold (≤0.8 ms/column
      amortised); hydrology tile ≤20 ms excluding macro height; column build p95 ocean ≤0.5 ms /
      plains ≤2 ms / mountain ≤6 ms; light ≤1 ms; mesh ≤0.5 ms per emitting section. Ring fill
      ~1.5–2 s on ~6 workers. First nightly run may revise each target once with a rationale; after
      that they are baselines. A >2× tap miss or an unfillable ring triggers R1.13 trigger 6.

### B5 Cost and parity
- [x] B5.3 Is a GPU sampling path allowed to differ in the last bits, and how is that
      reconciled with golden hashes?
      **CPU-only for anything that decides a block.** Field sampling, `floor(h)`, carve
      thresholds, feature placement, and material/biome selection run on the CPU evaluator.
      GPU is for rendering, culling, display lighting, upload, post, and debug visualisation.
      A GPU preview may read CPU tiles or approximate, but never writes back to tiles, chunks,
      or saves. Revisit only with the cross-platform project and proven bit equality.
- [x] B5.2 Which fields may be evaluated lazily vs eagerly?
      **Eager, dependency-ordered bundle per field tile.** One job builds, in fixed order:
      1 macro lattice fields (with compiler-grown halo) → 2 macro height → 3 hydrology (a *separate*
      leaf cell keyed `(description hash, hx, hz)`; a field tile depends on it, never builds it)
      → 4 climate → 5 biome arrays → 6 `HeightTile`. Tile state is `Absent | Building | Ready |
      Failed(TerrainError)` — no partial states visible to consumers. A 128-block tile covers 16
      columns; a hydrology tile covers 16 field tiles, so cost is amortised 1/16 per block.
      Everything per-column/per-section (detail, caves, strata, features, light, mesh) stays lazy
      by construction. Results are pure, cached, evictable, rebuildable; a failed stage records
      `Failed` so dependents fail fast. Overlay mode 0 shows tile state and per-stage build time.
- [x] B5.1 What is the measured cost per field sample on the reference machine?
      **Answered in B4.5** (`NS_PER_TAP_2D ≤ 100 ns`, `3D ≤ 150 ns`, validated by benchmark).
- [x] B5.4 How is field cost budgeted per chunk at the target radius?
      **Answered in B4.5** (per-stage ms ceilings derived from taps × samples).

---

## C. Macro landform

### C1 Elevation
- [x] C1.1 Which fields drive base elevation, and in what order?
      **Two curves, shelf + relief.** `base = curve_base(continentalness)` in blocks relative
      to sea; `relief = curve_relief(erosion)` in blocks; `height = SEA + base + relief *
      curve_shape(peaks) * mask(continentalness)`. Continentalness alone decides where land
      meets sea; the coast is the level set `base = 0`. Relief is masked to ~0 at the coast so
      retuning mountains cannot move the shoreline. Output stays `f32`, clamped, and is
      quantised once via `top_solid_y`.
- [x] C1.6 What is the exact coast profile? `base = 0` is a single level set, so beach width
      and shelf depth come from the slope of `curve_base` near 0. State the shelf floor depth,
      the beach band width, and whether a flat band exists.
      **Ocean floor 32 below sea (y=96); 6-block beach.** All values are offsets from `SEA`,
      never absolute `y`. Knots on `c` (relative blocks): `0.00 → -32`, `0.25 → -30`,
      `0.38 → -12` (shelf break over ~0.07 of `c`), `0.45 → -2`, `0.48 → 0` (waterline),
      `0.52 → +3` (beach plateau), `0.60 → +10`, `1.00 → inland plateau`. Invariants: floor
      depth 32, `base` monotone, `base = 0` at exactly one `c`, beach plateau only spans
      `0..+3`. Beach width is measured from the continentalness gradient, not assumed.
- [x] C1.7 Does the beach plateau need a minimum width floor, so a steep coastline cannot
      produce a zero-block beach?
      **Yes: guaranteed ≥3 blocks, by two means.** *Vertical:* for `|base| < 0.75`, blend `base`
      toward `BEACH_PLATEAU = 0.5` with `smoothstep` over another 0.75, **per block** (not on the
      lattice, so bilinear cannot smear it); the plateau floors to `SEA` and never sits at or below
      the waterline. *Horizontal:* cap the slope of `curve_base` between `base = -2` and `+3`, and
      have the compiler prove `max|grad(c)| * max dbase/dc <= 5 / BEACH_MIN_WIDTH`; a graph that
      cannot meet it refuses to compile. Terrace and river carve are gated off inside the coast
      mask, so mouths keep the plateau. Tests: synthetic steep coast yields a ≥3-block strip at a
      constant height, golden columns, and a sweep check for no cliff neighbour on the land side of
      a shoreline column.
- [x] C1.2 What is the elevation range in blocks: min, sea level, max?
      **Sea level 128** in a 384-tall world. Height clamp still to pin (see C1.5).
- [x] C1.5 What is the maximum terrain height, and is it a hard clamp or a soft spline
      asymptote near the top?
      **`MAX_TERRAIN_Y = 352` (sea + 224), 32 blocks of headroom.** The compiler computes the
      worst case `SEA + max(base) + max(relief*shaped) + max(per-block detail)` from the curve
      knots and node bounds and rejects any graph above it. Peaks thin out via the curves, not
      a ceiling. The `[1, WORLD_HEIGHT-2]` clamp is a backstop that debug asserts and CI fuzz
      check. The 32-block headroom is reserved for features, clouds, and build height.
      `MAX_TERRAIN_Y` lives in the world description; changing it is a version bump.
- [x] C1.3 Is elevation the sum of contributions, or a spline lookup per continentalness?
- [x] C1.4 Is there a flat "spawn plateau" near origin, or is spawn found, not made?

### C2 Shaping
- [x] C2.3 Are valleys carved by the height function or left to hydrology?
      **Hydrology only.** No valley/drainage/erosion term exists in the macro graph, and no
      hydro output may feed a height node (compiler cycle check, same as E3.4/E2). The only
      hydro-driven height change is the river carve in the `HeightTile`, after macro height, detail,
      and terrace, before `floor(h)`. **No dry valleys in v1**: every channel is a river cell that
      drains to sea or lake. Valley density is tuned by `RIVER_MIN_CELLS` plus the width/depth
      curves; a tributary threshold and real erosion are later version bumps.
- [x] C2.1 How does erosion/roughness modulate slope rather than absolute height?
      **`relief = curve_relief(roughness) * coast_mask`.** The field is renamed `roughness`
      everywhere; `erosion` is reserved for the solver. The rename alone changes no output but
      does change the graph hash, which is the correct version bump. CI lints against the
      identifier `erosion` in graph data.
- [x] C2.2 Are mountains ridged, plateaued, or both, and where is the switch?
      **Ridged → `unit` → `curve_shape`**, convex near 0 (broad foothills), steep near 1 (rare
      peaks). `fbm_ridged`, 4–5 octaves, frequency ~1/256 per block, lattice-only. Plateaus are
      a `terrace` node after shaping, gated by a mask of `roughness`.
- [x] C2.4 Is there a coastal shelf, a beach band, and a cliff band, with what widths?
      **Yes: 32-block shelf depth, 6-block beach, terrace-driven cliffs.** `terrace(h, step,
      sharpness, mask)` quantises toward `SEA + k*step` with smooth-to-vertical risers, lerped
      by a `roughness` curve mask. Runs per block after interpolation, before `floor(h)`, so
      risers can be vertical. Max displacement `step/2` is in the height bound. Cliff
      primitives are not in v1.
- [x] C2.3 Are valleys carved by the height function or left to hydrology?

### C3 Quantisation
- [x] C3.1 For `h = 72.4`, is the top solid block at 72 or 73? State the exact rule.
      **One helper: `top_solid_y(h: f32) -> i32 = floor(h)`.** Surface, water, caves, and
      features all read it. Nothing else quantises a height.
- [x] C3.2 Does the rule hold for negative and clamped heights?
      **Yes.** `floor` is defined for the whole range; clamping happens after.
- [x] C3.3 Is the macro grid bilinearly interpolated to blocks, or nearest-sampled with
      features? What happens at a cell edge?
      **Bilinear from the 4-point cell corners**, weights exact quarters. Per-block detail and
      per-block `terrace` run after interpolation; no nearest sampling.
- [x] C3.5 Is the height field kept as `f32` everywhere?
      **Yes.** Blending, slopes, caves, and features read the float. The v0.0.7 `round()` in
      the height formula is dropped; quantisation exists only at `top_solid_y`.
- [x] C3.4 Does interpolation use the four corners or a bicubic patch, and is that stable?

### C4 Vertical layout
- [x] C4.1 Bedrock: flat at y=0, noisy depth, or none?
      **Flat bedrock at y=0.** No cave, erosion, or water may breach it.
- [x] C4.2 How many air blocks above the highest terrain before the build limit?
      **Players build `y = 1..=383`; `y = 0` is indestructible bedrock and `y = 383` is
      player-only.** Terrain tops at 352, leaving 31 blocks of headroom; generated features clamp to
      `WORLD_HEIGHT-2 = 382`. One function `can_edit(x,y,z) -> Result<(), EditDenied>` gates place,
      break, bulk edits, explosions, and tools; a rejected edit writes no record and bumps no
      revision. Bulk edits clip and report the count. Skylight still seeds at 383, so a block there
      shades the column. Tests: 383 accepted, 384 rejected, breaking 0 rejected, bulk clip, no
      record on rejection, features never write 383.
- [x] C4.3 Is the world 128 tall for every column, including ocean floors?
      **No: the world is 384 tall.** This supersedes the 128-block constant and forces
      vertical residency, sky-light columns, and a section layout change.
- [x] C4.4 What happens at `y<0` and `y>=WORLD_HEIGHT`: clamp, void, or error?
      **Nothing exists outside `0..=383`.** `y=0` is bedrock, features clamp to
      `WORLD_HEIGHT-2`, and the `[1, WORLD_HEIGHT-2]` clamp is a backstop that debug asserts and
      CI fuzz require never to fire (C1.5).

---

## D. Erosion and hydrology

### D1 Depression filling
- [x] D1.1 Priority-flood with epsilon, or plain fill to the lowest outlet?
      **Priority-flood with `EPS = 1/64`** (D2.5).
- [x] D1.2 Is the filled surface stored, or only the implied water depth?
      **Filled heights are routing-only and never stored as terrain.** Lake extent and level are
      separate outputs, taken from the outlet (D2.6, D7.2).
- [x] D1.3 How is the tile border made deterministic when a basin crosses a seam?
      **Halo covering the longest crossing drainage path plus integer accumulation inflow from
      neighbour tiles** (D2.1); boundary basins are resolved with the same halo (D7.2).
- [x] D1.4 Is filling run before or after erosion, and does it iterate?
      **Before routing, single pass.** There is no erosion stage in v1 to order against (D5.1).

### D2 Flow
- [x] D2.1 D8 or D-infinity? What are the tie-breaking rules on equal neighbours?
      **D8 on the 4-block macro grid, tile-independent.** Receiver = lowest of 8 neighbours by
      `drop = (h_cell - h_nbr)/dist`, `dist = 1` or pinned `sqrt(2)` for diagonals. Ties break
      by fixed order N, NE, E, SE, S, SW, W, NW. Flats and pits are resolved by a deterministic
      priority-flood on `f32` heights (priority = height, then cell index) before routing, so
      every cell drains to the sea or a declared lake. Accumulation is `u32` upstream cell
      counts, processed by height descending then cell index. Routing runs on the 512-block
      hydrology tile with a halo covering the longest cross-edge drainage path; boundary inflow
      is passed as integer accumulation from neighbour tiles, so results are independent of
      tile and chunk generation order. Flow uses the pre-detail macro height only, never
      per-block noise. No per-block refine in v1.
- [x] D2.5 Does priority-flood add an epsilon per visited cell, or use pure height + index
      ordering?
      **Epsilon flood on the macro lattice.** Queue key `(filled_height, cell index)`, compared
      exactly. Seeds: sea cells and tile-boundary inflows. `filled = max(h[n], filled[c] + EPS)`
      with `EPS = 1/64` (`0x3C800000`, exact in `f32` to ~2^17 blocks). Filled heights are used
      for routing and accumulation only — never for the surface. Receivers come from D8 on
      `filled_height`. Max distortion is bounded by the longest flat run (<~2 blocks per
      512-block tile crossing); a debug assertion checks the lake-surface slope bound.
- [x] D2.6 How is a lake separated from a filled flat?
      **Separate step:** a lake is a connected set of cells where
      `filled_height > h + LAKE_MIN_DEPTH` (~1 block). Water level is the *outlet's* filled
      height, floored once. Interior filled heights never define a surface.

### D3 Rivers
- [x] D3.1 What accumulation threshold starts a river, and does it vary by climate?
      **`RIVER_MIN_CELLS = 300`**, an integer in the world description. Not climate-dependent
      in v1.
- [x] D3.2 Is width derived from accumulation, from a channel field, or both?
      **Accumulation.** `width = clamp(W_SCALE * sqrt(acc - RIVER_MIN_CELLS + 1), 1, 10)`, with
      `W_SCALE` an `f32`-bit graph constant tuned so `acc ≈ 300` gives ~2 blocks and
      `acc ≈ 5000` gives ~10. Exact `sqrt`, never `pow`. Width ramps from 0 over the first ~50
      accumulation above the threshold so headwaters taper in.
- [x] D3.3 Is the channel carved as a V, a U, or a depth profile with banks?
      **U-profile, mask-faded.** Bed half-width `w/2`, bank half-width `w/2` beyond the bed,
      depth `D = clamp(w/2, 2, 6)`, bank falloff by `smoothstep`, applied to `f32` height before
      `floor(h)`. Distance comes from the interpolated river-influence field, not a per-block
      path search. Water level per cell `floor(h_center - 1)`; bed material sand on flat slope,
      gravel otherwise, 1–2 blocks thick; banks keep surface material. Gated by the coast mask;
      bed never below `SEA-30` or `y=1`.
- [x] D3.4 Do rivers meander, and is meander applied before or after carving?
      **Carve-only warp.** `p' = p + n_perp(p*MEANDER_FREQ) * amp`, one OpenSimplex2 octave,
      `amp = clamp(1.5*w, 0, 8 blocks)`, offset along the receiver direction (unit vector per
      cell, bilinearly interpolated and renormalised). Wavelength ~64–256 blocks. Fades to 0 at
      headwaters and mouths. Halo grows by `ceil(MEANDER_MAX/MACRO_CELL)+1`. Routing and
      accumulation never see the meander.
- [x] D3.5 Do rivers reach the sea, or can they end in a lake? Is a closed basin allowed?
      **Endorheic basins are allowed.** A river chain ends at the sea or at a lake (open or closed);
      never on dry land, never in a basin that failed the depth/area gates. Closed lakes keep their
      outlet (spill) level, are never drained in v1, and the river's carve ends at the lake edge with
      the bed meeting the lake level. A below-gate basin stays dry and flow passes through its filled
      cells, so the channel remains continuous. A basin larger than the hydrology halo is treated as
      open and drained at the tile edge, so levels never depend on tile origin. Acceptance wording is
      "every river ends in the sea or a lake"; the sweep asserts the terminal cell is sea or a lake
      id.

### D4 Thermal erosion
- [x] D4.1–D4.3 **Deferred with hydraulic erosion.** v1 ships no erosion solver of any kind.
      River carving is separate and unaffected.
- [x] D4.4 **Deferred v2** (`heightfield_post` no-op, see D5.1).

### D5 Hydraulic erosion
- [x] D5.1 Droplet-based, grid-based, or neither for the first world?
      **Neither. Deferred.** v1 terrain comes from the graph plus accumulation carving.
      Extension point reserved: a named `heightfield_post` stage between macro height and flow
      routing, a no-op in v1, pure function of `(description, tile)`. Adding a solver later is
      a version bump with its own goldens. Rivers, lakes, and carving must not read any erosion
      output, so adding it later does not change their logic.
- [x] D5.2–D5.4 **Deferred v2.** Live only once a solver exists; they describe that solver's
      parameters, per-tile keying, and neighbour clamp, none of which v1 has.

### D6 Resistance
- [x] D6.1–D6.3 **Deferred v2.** Resistance is an input to a solver that v1 does not have. The
      hook is reserved in `heightfield_post` (D5.1); no uniform map, no version bump, no effect
      on thermal or hydraulic until the solver lands.

### D7 Water bodies
- [x] D7.1 Is the sea a filled volume or a surface plane with a depth field?
      **Water is a normal block everywhere**, including 32-deep oceans. Generation fills
      `top_solid_y+1 ..= SEA` per column, and sections fully covered by water or solid become
      `Uniform` (K1.5), so no allocation. Sections mixing water with one other block use a
      1-bit palette. The mesher emits faces only at water/air and water/solid interfaces, never
      water/water. Lighting treats water as a skylight attenuator. Air below `SEA` stays air:
      caves and player spaces are not flooded by definition. Flow behaviour is a separate
      decision.
- [x] D7.2 Are lakes global-level, per-basin, or per-tile with shared outlet heights?
      **Per basin, spill at outlet, closed basins allowed.** Level = outlet's filled height,
      floored once. A lake needs `max depth >= 2` and `area >= 12 cells` (`LAKE_MIN_CELLS`),
      else it stays dry. Open lakes feed the outlet as a river source with their accumulation;
      endorheic lakes are allowed and not drained in v1. Never filled above
      `MAX_LAKE_Y = MAX_TERRAIN_Y - 32`. A lake at or below `SEA` is just sea and is not
      recorded. Level/extent/outlet are per-tile pure outputs; boundary basins use the halo.
- [x] D7.3 Is the water table a field, and does it feed wells or underground lakes?
      **None in v1.** Water exists only as sea, lakes at their level, and rivers at their stepped
      level. Everything below the surface is rock or carved air; **caves never contain water**
      (`WATER_MARGIN`, D9), and no spring/well/aquifer/seepage exists. No water-table key exists in
      the schema. Extension points (`cave_water_repair`, `heightfield_post`) are reserved; an
      aquifer is a later version bump needing a stored per-column level and a revised D9 invariant.
      Player-dug water adjacency is runtime behaviour (D7.1/D7.4), not generation.
- [x] D7.4 River and lake water share one level rule, or different?
      **Stepped, blocks only.** River level per cell `floor(h_center - 1)`, then made monotone
      downstream: `level[c] = min(level[c], level[receiver]+1)`. Per-block level is the
      bilinearly interpolated value floored once. Clamped to `bed_y+1 ..= bank_top-1`; at a lake
      outlet `>=` lake level; at the sea it meets `SEA` and never exceeds it. Static water only:
      no simulation, no currents.

### D8 Tiling
- [x] D8.1 Tile size 512 with a 64 border: is that confirmed, and is the border counted in
      the tile or added?
      **Owns 512×512 blocks (128×128 cells), aligned to the global grid and to 4×4 field tiles
      (B4.2).** Stored area adds `HYDRO_BORDER = 16 cells` (64 blocks) minimum on every side, so
      **160×160 cells stored**. Border cells are recomputed from the global lattice (bit-identical
      to the neighbour's interior), never copied at runtime. `HYDRO_BORDER` is the compiler's max
      over: meander halo, warp halo, rain-shadow upwind range, and the priority-flood margin;
      never below 16, always a multiple of `MACRO_CELL`. Only owned cells are authoritative
      output; border cells are never read by consumers. Tiles are pure and cached by
      `(description hash, hx, hz)`; the world edge mirrors the edge row.
- [x] D8.2 Are tiles generated in a fixed order, or order-independent by construction?
      **Order-independent.** A tile is a pure function of `(description, hx, hz)`; borders are
      recomputed from the global lattice, never copied; no tile reads another. Identical for any
      order, thread count, or tile origin. Two modes, fixed per generator version: **(a) tile mode**
      caps basin size at what the border resolves and drains bigger basins at the edge, or
      **(b) world mode** runs one whole-world pass once per description, stores it as a derived
      artefact, and slices per-tile views — no halo, no basin cap. **v1 uses world mode** for the
      finite world; it is ~150 ms once at open, off the main thread behind the cover.
- [x] D8.3 What is the cost of one tile, and how many can be in flight?
      **≤20 ms per tile** excluding macro height (~60 ms cold including it, counted in P1.11);
      **≤2 hydrology jobs in flight** across the pool, leaf jobs with no queued dependency, waiters
      take other work (5 s timeout). World mode is one ~150 ms job at open.
- [x] D8.4 How is a tile invalidated when climate or geology changes?
      **Hash-only.** The key is `(description hash, hx, hz)`, so a different description misses
      naturally. Nothing mutates a tile — not edits, not carved rivers, not LOD, not time. Tiles are
      disposable, evictable under S1.7, rebuilt identically, and a tainted tile (S1.8) is never
      cached. A failed job records `Failed(TerrainError)` so dependents fail fast.

### D9 Volume repair
- [x] D9.1 Which interactions trigger a repair, in priority order?
      **None in v1 — the stage is dropped.** Prevention instead: cave carve and 3D detail are
      skipped within `WATER_MARGIN = 2` blocks of any water cell or any column with a non-empty
      water range (sea, lake, river), applied at column granularity with a 1-column dilation of the
      water mask, read from the `HeightTile` and hydrology tile only (no new 3D dependency).
      `WATER_MARGIN` is a world parameter, `>= 1`.
- [x] D9.2 The `32³` window plus 8 margin: is the margin in the input, the output, or both?
      **Moot** — no repair pass runs. A named no-op `cave_water_repair` sits between step 4 and
      step 5 of F1.5, output a pure function of `(description, section)`, reserved for v2.
- [x] D9.3 Is the repair idempotent, and how is that tested?
      Moot; the invariant is tested instead: after steps 1–4, **no carved air cell is
      face-adjacent (6-neighbour, across column and section borders) to water.** A sweep check and a
      debug check in the column job raise `InvariantViolated{check:"cave_water_adjacent", coord}`.
      Player-dug adjacency is out of scope (that is D7.1 water behaviour).
- [x] D9.4 What is the fallback order when two repairs conflict in one window?
      Moot in v1.

---

## E. Climate and biomes

### E1 Fields
- [x] E1.1 Is temperature latitude-based, altitude-based, noise-based, or a sum?
      **`temp = clamp(lat(|z-EQ|/HALF) + noise - alt, 0, 1)`.** `lat` is a monotone cubic,
      warm at 0, cold at 1; `Z_EQUATOR` and `Z_HALF_SPAN` are world parameters (blocks), so
      world size is data. `noise = (fbm(1/512) - 0.5) * ~0.15`. Symmetric about the equator in
      v1; a hemisphere offset is a later bump. Sea ice and snow are material thresholds, not
      biome entries.
- [x] E1.2 What is the lapse rate per block of altitude?
      **`LAPSE = 1/320` per block above `SEA`, from pre-detail macro height only.**
      `alt = max(h_macro - SEA, 0) * LAPSE`; a 224-block peak is ~0.70 colder. No lapse below
      sea. Climate is lattice-only, so a 1-block bump or river carve never changes it. Snow is
      a material rule at `temp < T_SNOW` (~0.25) with ±2 blocks of per-block height jitter.
- [x] E1.3 Is humidity independent noise, distance to water, or rain-shadow output?
      **Pinned at E2.4:** `clamp(unit(noise) * (1 - SHADOW_STRENGTH*shadow) + MARITIME*coast, 0, 1)`,
      in that exact order, with one clamp. Debug counter tracks saturation; >5% land saturation
      warns that `MARITIME` is too strong.
- [x] E1.4 Is seasonality a field at all in the first world?
      **No.** Climate is static and reads no time (A2.4). Snow line depends only on `T_SNOW`,
      `LAPSE`, and latitude; a colder world is a different description. No season key exists in the
      schema, tiles, vocabulary, or biome records. Seasonal *looks* are renderer-only (tint curve,
      snow-cover shader overlay, sky/fog) and never write blocks; block-changing seasons would be a
      runtime simulation through the edit path.
- [x] E2.4 Is the shadow a multiplier on humidity or an additive drying term?
      **Multiply, then add maritime, then clamp once.** `n = unit(noise)`, `d = 1 -
      SHADOW_STRENGTH*shadow`, `t = n*d`, `humidity = clamp(t + MARITIME*coast, 0, 1)`. Constants
      are `f32` bits in `[0,1]`; the compiler propagates `[0,2]` into the single clamp. Golden
      lattice sample pins the order; `shadow=0, coast=0` must return `n` bit-exactly.

### E2 Rain shadow
- [x] E2.1 Which direction does the prevailing wind march, and is it constant per world?
      **Fixed direction, `+x` wind (march toward `-x`).** `WIND_DIR` is a world parameter
      (an axis in v1), so changing it is a version bump. Z-dependent bands are a later bump
      behind the same node.
- [x] E2.2 How far does the march go, and how does it decay?
      **`SHADOW_RANGE_CELLS = 32` (128 blocks).** `shadow = max_k(h[x-k] - h[x] -
      SLOPE_K*k*MACRO_CELL)`, clamped at 0, then `curve_shadow(shadow/SHADOW_NORM)`.
      Fixed-order loop, `f32` mul/add only, bit-stable. Coast term from the same march (first
      ocean cell upwind) adds `MARITIME` humidity on windward coasts.
- [x] E2.3 Does it reuse the hydrology grid exactly, including the border?
      **Yes.** Macro height and 4-block lattice, never carved or per-block height. The tile
      halo grows upwind by `SHADOW_RANGE_CELLS`.
- [x] E2.4 Is the shadow a multiplier on humidity or an additive drying term?
      Multiplier plus an additive coast term, clamped once — order is still unpinned.

### E3 Biome selection
- [x] E3.1 Table lookup with thresholds, nearest-point in climate space, or weighted blend?
      **Nearest point in `(temp, humidity)`.** Biome record `(id, temp_c, humid_c, weight,
      priority)` in the content pack, centres as `f32` bits. `d2 = (dt*T_SCALE)^2 +
      (dh*H_SCALE)^2/weight`, no `sqrt`; argmin ties break by lowest id. Chosen per lattice
      cell; non-climate overrides (ocean, beach, river, lake, snow) apply later and never enter
      climate space. Compiler rejects duplicate/out-of-range centres and >255 biomes. Adding a
      biome is a content edit plus a description version.
- [x] E3.3 Are transitions hard, dithered, or blended? Over how many blocks?
      **Blend + keyed dither, ~24 blocks.** Per block take best and second `d2`,
      `m = d2_second - d2_best`, `d = m / |grad m|` (straight-line blocks to the border),
      `w = smoothstep(0, BLEND_W, d)` (1 deep inside, 0.5 at the border). Top two biomes only.
      Material/tree/cover pick best if `hash01(seed, "biome_dither", x, z) < w`, else second.
      Hash keyed on `(x,z)` only, never `y`, so a column has one biome and chunk order does not
      matter. Height and snow line use undithered lattice climate, so terrain stays smooth.
      The analytic gradient is
      `grad m = 2*[(T_b - T_s)*grad T + ((H - H_s)/w_s - (H - H_b)/w_b)*grad H]`, with
      forward differences over one macro cell so every global cell has one stencil.
- [x] E3.5 What does a field tile store per cell for biomes?
      **Three parallel `u8` arrays:** `biome_best`, `biome_second`, `biome_margin`, with
      `q = clamp(floor(d/BLEND_W*255 + 0.5), 0, 255)` and `BLEND_W = 24/(2*0.8042) ≈ 14.92`
      blocks. Dither reconstructs
      `w = 0.5 + 0.5*smoothstep01(q/255)`: `q=0` is the border (`w=0.5`), `q=255` is deep
      inside (`w=1`). One helper `biome_pick(tile, cell, x, z) -> u8` owns the logic. Nearest
      lattice cell decides the cell; no interpolation. Single-biome worlds set
      `second = best`, `q = 255`. Climate (`temp`, `humidity`) is stored separately as `f32`
      lattice arrays and never feeds terrain through the biome arrays. ~3 bytes/cell, ~3.7 KB
      per 128-block tile.
- [x] E3.6 How is `BLEND_W` calibrated so the transition really spans 24 blocks?
      **Measured, and now met.** `W_blocks` = transect distance from the border (`w = 0.5`) to
      `w = 0.95`, doubled. `terrain-gen calibrate-blend` casts 512-block transects perpendicular to
      each biome-pair border over the sweep seeds and reports median/p10/p90 per pair. Acceptance:
      **median 20–28 blocks, p10 ≥ 8, p90 ≤ 64**, re-run on any change to climate fields,
      `BLEND_W`, or biome centres; pairs with <20 transects are reported only. The audit found the
      transect scale correct (synthetic linear margin: 23.90 measured vs 23.98 closed form) and
      two measurement defects: the border walk skipped the nearest minimum, and the direction
      plane straddled the border's V. Both are fixed, and the gradient normaliser is now part of
      `q` rather than a later bump. At 16 seeds all five pairs pass (medians 23.33–27.17, p10
      14.69–18.76, p90 32.55–63.32). The column pipeline still reads `biome_best` directly instead
      of calling `biome_pick`, so the generated block stream is unchanged and the generator
      version stays 2 until that call is wired. `q` quantisation is checked to be <0.1 block per
      step at a ~24-block band.
- [x] E3.2 How many biomes exist and how are they named and versioned?
      **Six records in two classes.** Climate biomes (the only argmin candidates): **plains**
      (~0.55, 0.45), **forest** (~0.50, 0.75), **desert** (~0.85, 0.10), **tundra** (~0.10, 0.35),
      all weight 1.0. Centres are tuning starting points placed against the measured climate
      histogram, stored as `f32` bits. Override sets, never in climate space: **ocean** (slot 2) and
      **beach** (slot 3, with a cold-coast variant). **"Mountains" is not a biome** — alpine look
      comes from snow cap, slope stone, strata and deep rock. Compiler: ≥3 climate biomes, distinct
      centres, ids resolve by stable name. Per seed: ≥3 climate biomes present, each ≥5% of land.
      Desert has no trees; tundra has sparse conifers.
- [x] E4.7 Underwater surfaces: does the sea-floor stack vary by depth and climate?
      **Yes, `d = SEA - top_solid_y` bands** (offsets from `SEA`, never absolute `y`): `1..4`
      shore sand (gravel if `temp < T_COLD_SHORE`), `5..14` shelf sand (gravel if
      `temp < T_COLD_SHELF`, hashed clay patches), `15..24` gravel slope, `>=25` abyss clay/silt.
      New closed predicate `depth_below_sea_in(lo, hi)`; bands are ordered rules in the E4.1 format.
      Ragged edges via `hash01(seed,"seafloor_dither",x,z) < 0.35`, keyed `(x,z)` only. Patch
      fields are 2D. Stack is 3 blocks of band material then symbolic `STONE`. River/lake beds
      outrank it; biome topsoil never appears. Compiler requires bands to tile `d >= 1` with a
      catch-all. Golden columns: shore, shelf, slope, abyss, cold shelf, band boundary.

### E4 Surface rules
- [x] E4.1 Is the surface rule a per-biome palette or a predicate list?
      **Ordered predicate list, one shared evaluator.** Rule = `(predicate, block_id,
      depth_blocks)`, first match wins, per *column*; the section fill reads the resolved stack
      by depth. Predicates are a closed Rust enum of integer/`f32` compares (`depth_below_top`,
      `slope`, `height_vs_SEA`, `temp`, `water_adjacent_within(N)`, `is_river_bed`,
      `is_lake_bed`, `near_coast`, plus and/or/not). A catch-all rule per biome is mandatory.
      Block ids reference the material manifest by stable name. Adding a predicate is a code
      change plus version bump; adding or reordering rules is content data.
- [x] E4.2 How deep is topsoil, and does depth vary by slope and biome?
      **Per rule `depth_blocks`**, so depth is data, not a constant. Filler rules follow topsoil
      and may stack.
- [x] E4.3 Is there a slope rule that exposes stone above a threshold angle?
      **Integer slope drop from final quantised tops:**
      `slope_drop = max(|top-top(x±1)|, |top-top(x,z±1)|)`. Stone at
      `slope_drop >= STONE_DROP` (2), with a 35% dither band at `STONE_DROP-1` keyed on `(x,z)`.
      River bank rules sit **before** the slope rule, so banks keep their material. A macro
      gradient predicate exists for broad tinting. Neighbour tops come from the height tile,
      never from neighbouring chunks.
- [x] E4.4 Where does snow start: altitude, temperature, or both?
      **Temperature only:** `temp < T_SNOW` (~0.25), never below `SEA`, never on bed or beach
      columns, with ±2 blocks of height jitter in the material pass.
- [x] E4.5 What is the precedence when several rules apply?
      **Fixed, first match wins:** 1 river/lake bed, 2 underwater sea floor
      (`top_solid_y < SEA`), 3 beach band (`0 <= base <= +3`, `top >= SEA`), 4 snow cap,
      5 slope stone, 6 biome topsoil, 7 biome filler, 8 stone, 9 bedrock at `y=0`. Rules 1–5
      are locked entries in every biome list; the compiler rejects a biome rule before slot 5
      or a missing catch-all. Each rule has a golden test column; a debug view shows the fired
      rule index.
- [x] E4.6 **Resolved: derived `HeightTile`.** `i32 top_solid_y` per column, **130×130**
      (128 + 1-block margin), a pure function of the field tile plus the hydrology tile, keyed
      `(description hash, tx, tz)` and disposable (J4.5). Built per block:
      bilinear macro height → detail → terrace → river carve → **one** `floor(h)`.
      It is the only height array that `slope_drop`, tree placement, and surface rules read.
      Slope uses this 2D top **before** 3D detail and caves; F4.1 handles overhang spans
      separately.
- [x] E4.7 Underwater surfaces: does the sea-floor stack vary by depth (sand near shore,
      gravel mid, clay deep) and by climate?

---

## F. Volume synthesis

### F1 Model
- [x] F1.1 Heightmap plus carve is the first mode: is density mode in the same release?
      **Heightmap-first plus a depth-limited 3D detail band (F3).** Hydrology, climate, rivers,
      and biomes read only the pre-detail 2D macro height.
- [x] F1.3 What resolution is evaluated: per block, or per cell then expanded?
      **Global 4×4×4 lattice, trilinear fill**, same `MACRO_CELL` grid extended in `y` (96
      cells per 384-tall column). Samples at `4k+0.5` on every axis via `sample_pos`; lattice
      is global, never chunk- or section-anchored. 3D tiles are 128×128 columns with a 1-sample
      border. Skip any lattice cell whose 4-cell neighbourhood is above
      `top_solid_y - CAVE_MIN_ROOF` or below `y=1`, and never sample `Uniform` air sections
      above terrain. No 3D domain warp.
- [x] F1.2 Is the density function an SDF with a gradient, or a threshold on noise?
      **Threshold on `[0,1]` density** (`density > CAVE_T`), with `unit()` as the only mapping.
- [x] F1.4 Do both modes feed one baseline type with no mode flag downstream?
      **Yes.** One baseline; downstream reads solidity only, so no mode flag exists after F1.5.
- [x] F1.5 What is the exact order of the 3D passes per column?
      **Fixed:**
      1. `HeightTile` → `base_top`.
      2. Fill solid `<= base_top`; water `base_top+1 ..= SEA`, plus lake and river levels.
      3. 3D detail band edits solidity (excluded where `base_top <= SEA+2`).
      4. Cave carve; roof tested against the **highest solid after step 3**.
      4b. `cave_water_repair` — **no-op in v1** (D9), reserved for v2.
      5. Span scan, `stone_at`, surface stack.
      6. Ores → boulders → trees → plants (H2.1).
      Carve never floods; air below `SEA` stays air (D7.1).

### F2 Caves
- [x] F2.1 Which cave families ship first?
      **Cheese + spaghetti, 4 taps per 3D sample.** Cheese: fBm 2 octaves, `1/48`, scaled by
      `curve_cheese_y(y-SEA)` so chambers fade near the roof. Spaghetti: two independent
      single-octave fields `a`,`b`, `~1/64`, `tunnel = 1 - smoothstep(0, SPAG_R,
      max(|a|,|b|))`, minimum radius ≥ 3 blocks, gated to a `y` band. Combined
      `density = max(cheese, spaghetti)`. Noodle, ravines, aquifers, and lava pools are later
      version bumps. `CAVE_TAP_BUDGET = 4` is enforced by the compiler.
- [x] F2.5 How do caves open to the surface?
      **Mask-gated roof exception.** Default roof `CAVE_MIN_ROOF = 4`. An entrance field
      ([0,1], fBm ~1/192, `curve_entrance`) lowers the roof smoothly:
      `roof = lerp(4, 0, smoothstep(ENTRANCE_T, ENTRANCE_T+0.05, entrance))` with
      `ENTRANCE_T ≈ 0.82`, and eases the density threshold. Hard exclusions entrance cannot
      override: `y<1`, `top_solid_y <= SEA+1`, river/lake beds, within 2 blocks of a bank.
      Mouths require `macro_slope_ge(MOUTH_SLOPE_MIN)`. Mouth columns are flagged so F4.1
      treats the top span as open to the sky.
- [x] F2.2 Is a cave a threshold on a noise pair, or a distance to a curve? (Answered for
      spaghetti: noise pair.)
- [x] F2.3 Do caves respect a depth window, and what is it?
      **Depth *weights* in `[0,1]` (monotone curves), not hard cutoffs.** With
      `d = top_solid_y - y` and `s = y - SEA`:
      *Cheese* `= curve_cheese_depth(d) * curve_cheese_floor(y)`: 0 for `d < 12`, 1 by `d = 40`,
      flat to `d = 140`, 0 by `d = 200`; floor 0 below `y = 8`, 1 by `y = 24`. Zero where
      `top_solid_y <= SEA+1 && y > top-24` (sea floor, shelf, beaches stay sealed).
      *Spaghetti* `= curve_spag_y(y) * curve_spag_depth(d)`: 0 below `y = 12`, 1 from `y = 24`,
      flat to `y = 296`, 0 by `308`; depth 0 at `d < 4`, 1 by `d = 16`. Lowlands near sea
      cap the ceiling at `SEA-8` via a mask of `top_solid_y`, so coastal plains get no shallow
      tunnels. Combined `density = max(cheese*w, spag*w) > CAVE_T`. Weights are lattice-resolved
      from `HeightTile` tops (no extra 3D tap). The roof rule still applies. Compiler: weights in
      `[0,1]`, zero at `y = 0..7`. Tests: sealed under beach/sea floor, no cheese below 8,
      spaghetti 20 under a 300-high peak, no cell within 4 of the surface except mouths, cave air
      fraction 8–15% in the band.
      Cheese driven by `curve_cheese_y`; spaghetti gated to `[SPAG_Y0, SPAG_Y1]`. Values open.

### F3 Overhangs
- [x] F3.1 Overhangs, arches, and floating rock: which are in scope for the first pass?
      **Overhangs and arches, plus bounded floating rock.** 3D detail applies only inside a
      band `[base_top-24, base_top+8]`; outside it the world is heightfield plus caves.
- [x] F3.2 Are they caused by 3D noise on the surface band, or by explicit primitives?
      **3D noise on the surface band:** `density3d = clamp((h_f32-(y+0.5))/8, -1, 1) +
      DETAIL_AMP * band_fade(y) * mask * n3`, with `mask = curve_detail(roughness) *
      coast_mask * curve_slope(macro_slope)`. `band_fade` reaches 0 at both band edges, so no
      seam. Floaters must exceed `FLOAT_MIN ≈ 0.15`; no connectivity pass in v1.
- [x] F3.3 How far below the surface does 3D detail apply before it is wasted cost?
      **`DETAIL_DOWN = 24`, `DETAIL_UP = 8`**, both world parameters, in the height proof.
- [x] F3.4 Is there a rule preventing floating islands below the sea floor?
      **Yes:** no detail or mouth carve where `base_top <= SEA+2`, on river/lake beds, or within
      2 blocks of a bank.

### F4 Materials in volume
- [x] F4.1 Is material chosen by depth, by strata, by slope, or by a priority list?
      **Per solid span, not per column.** `resolve_spans(column) -> [(y0,y1,kind)]`. One span →
      the E4.5 column stack unchanged. Multiple spans → the top span keeps full E4.5; lower
      span tops get a reduced `cave_floor` rule set (stone/gravel, no snow, no beach, no biome
      topsoil unless open to sky); ceilings are always stone. Depth is per span, measured from
      the nearest air above, capped at filler depth. "Open to sky" is decided by skylight reach,
      not span count. Debug view shows span count and each top's kind.
- [x] F4.2 Are cave walls given a different material, and by what rule?
      **Walls, ceilings and span undersides are always `stone_at`** (G1.2). Never topsoil, snow,
      beach, or bed material underground. Cave floors (span tops not open to sky) are `stone_at`
      with **gravel patches**: `hash01(seed,"cave_floor",x,z) < GRAVEL_P(zone)` **and** a 2D
      low-frequency patch field (`~1/24`) above `GRAVEL_PATCH_T`, so patches clump. `GRAVEL_P`
      curves over two 2D inputs: columns to the nearest mouth, and depth below `top_solid_y`
      (fading to 0 below `DEEP_Y+4`). Deep rock is never gravel. Gravel is 1 block thick. Mouth
      columns may keep biome topsoil on the floor **only** where the span is open to the sky
      (`CAVE_MOUTH_BLEND = 4`). Keyed `(x,z)` only. New predicates `cave_floor`, `near_mouth(N)`.
- [x] F4.3 Is gravel/sand placed under water automatically?
      **Yes, by E4.7's slot-2 banding** (shore/shelf/gravel/clay by depth and temperature), and by
      the river bed rule in D3.3. Not by this stage.
- [x] F4.4 Are ores placed here or in geology?

---

## G. Geology and materials

### G1 Strata
- [x] G1.1 Are strata horizontal layers, a warped stack, or a 3D field?
      **Warped absolute-`y` bands + 2D faults.**
      `band_index = floor_div(y + 0.5 + strata_warp(x,z) + fault(x,z), THICKNESS)`.
      `strata_warp` is one fBm (2 octaves, ~1/96) scaled by `STRATA_WARP_AMP ≈ 8`, on the
      4-block 2D lattice, bilinear. `fault` is a piecewise-constant offset from a ~256-block
      cell grid (hash-keyed, `0` or `±3`), smoothstepped over 4 blocks. Table indexed by
      `band_index mod N`, so negative `y` is safe. Costs one add, one `floor_div`, one lookup
      per solid block.
- [x] G1.2 Is the generic stone resolved through one function?
      **`stone_at(x,y,z)` is the only resolver.** Below `DEEP_Y` → deep rock; inside the 4-block
      dither band → hashed deep rock; else strata band. Rule 8 stores a symbolic `STONE`, not a
      block id. Cave walls, cliffs, span undersides, slope stone, and ore host checks all call
      it. Golden test: cave wall, cliff face, and subsoil at the same coordinate return the same
      id.
- [x] G1.3 Do strata continue under oceans unchanged?
      **Yes.** The surface stack (sand/gravel) sits above them; strata never override surface
      rules.
- [x] G1.4 Is there a depth floor where everything becomes one rock?
      **`DEEP_Y = 64`, single deep rock below it**, with a 4-block ragged transition driven by
      `hash01(seed, "deep_dither", x,y,z)` at thresholds 1.0/0.75/0.5/0.25. Strata lookup is
      skipped below `DEEP_Y`, so it costs nothing deep. Only replaces the stone family; never
      surface, bed, beach, snow, slope, water, or bedrock.

### G2 Deformation
- [x] G2.1 Folds: sinusoidal displacement of the layer coordinate?
      **Yes, via `strata_warp` only.**
- [x] G2.2 Faults: hard offsets, and how are they keyed so they stay straight across chunks?
      **Piecewise-constant 2D cells, hash-keyed, smoothstep ramp over 4 blocks.** Materials
      only. Faults never move height in v1.
- [x] G2.3 Are deformation fields 2D or 3D?
      **2D**, evaluated once per column and reused for every `y`.
- [x] G2.4 **Resolved: no scarps in v1.** Faults are layer offsets only, and "visible fault
      scarps" are dropped from v1 acceptance. Scarps are deferred behind a version bump as a
      height-side node on its own seed stream (alongside the fold warp in G2.1). Until then a
      fault is legible only as layers stepping inside a cliff or terrace face.

### G3 Veins
- [x] G3.1 Vein shape: ellipsoid clusters, curves, or noise threshold?
      **Feature candidates, two-pass.** Discover per 32³ region with
      `hash64(seed, "ore:<name>", region)`; each candidate has a stable id
      `hash64(name, region, index)`, an ellipsoid or short random-walk shape, and
      `ORE_MAX_EXTENT <= 8`. Apply reads the 3×3×3 neighbouring regions, tests membership in a
      fixed order, and replaces only host rock. Overlaps resolve to lowest candidate id.
- [x] G3.2 Is vein placement host-rock dependent?
      **Yes.** Host set = stone family plus deep rock, by stable name. Never surface, bed,
      beach, snow, water, air, or bedrock.
- [x] G3.3 Is vein clustering done in discovery or application?
      **Discovery** (counts per region from a depth curve).
- [x] G3.4 Do veins affect erosion resistance? **Moot:** no erosion solver in v1.

---

## H. Features and scatter

### H1 Discovery
- [x] H1.1 What is the feature region size, and does it exceed the largest feature?
      **32³, aligned to `SECTION_SIZE`** (`REGION_SIZE = SECTION_SIZE`). Centres always inside
      their own region; every type declares `MAX_EXTENT` and the compiler rejects
      `MAX_EXTENT > 32`. Apply gathers exactly 3×3×3 regions. Ores ≤ 8; trees ≤ 12
      (canopy radius + lean). Structures > 32 are not v1.
- [x] H1.2 How is a candidate identified stably across runs and versions?
      `hash64(seed, "feat:<name>", region)` drives RNG; id is stable per candidate; overlaps
      resolve lowest-id-wins.
- [x] H1.3 Is the index stored, recomputed, or cached on disk?
      **Cached in memory by `(description hash, region)`**, pure function, listable in a debug
      view.
- [x] H1.4 How are candidates deduplicated when regions overlap?
      3×3×3 gather plus lowest-id-wins; centres are region-owned, so no duplicates.

### H2 Application
- [x] H2.1 What is the write order between features, and is it deterministic?
      **1 terrain → 2 ores → 3 boulders → 4 trees (logs then leaves) → 5 plants.** A constant,
      not per world.
- [x] H2.2 What clips a feature at the chunk boundary: block writes, or region ownership?
      **Block writes**, with a bounding-box cull before any per-block test; `Uniform` sections
      are skipped.
- [x] H2.3 Can a feature overwrite terrain, water, or another feature?
      **Only through its declared mask:** `replace_host` (ores/boulders), `air_only` (plants,
      leaves), `air_or_leaf` (logs), `replace_surface`. Nothing writes water, bedrock, or
      air-to-solid outside its mask. Compiler rejects contradictory masks.
- [x] H2.4 Is application idempotent for a repeated chunk generation?
      **Yes** — pure function of `(description, section)` plus the 3×3×3 candidate lists.

### H3 Trees
- [x] H3.2 Shape grammar: trunk, branches, canopy volume, roots?
      **Data templates over a closed primitive set:** `trunk`, `branch`, `canopy`
      (ellipsoid/cone/layered disc), `root_flare`. Per-tree variant from
      `hash64(seed, "tree:<species>", candidate_id)`. Leaves only within the canopy volume and
      within `LEAF_MAX_DIST = 4` of a trunk/branch, checked on the template's own parts — no
      world scan, no floating leaves. Compiler rejects `MAX_EXTENT > 12` or a top above
      `WORLD_HEIGHT-2`.
- [x] H3.3 Spacing rule: Poisson, minimum distance, or density field?
      **Jittered grid + priority rejection.** `TREE_CELL = 6`, `(cell, group)` keyed RNG gives
      jitter, priority, acceptance, species. Density only changes acceptance, never the grid.
      A candidate loses to any higher-priority candidate within `MIN_DIST` in the 3×3 cells
      (ties: lower cell index). Spacing resolves before 2D height filters.
- [x] H3.4 Slope and altitude limits per species?
      **Yes:** `slope_drop <= TREE_MAX_DROP`, not underwater/river/lake/bank, biome must allow
      the species, and the top span must be open to the sky.
- [x] H3.5 Do trees replace leaves in water, and do they float or stand on the seabed?
      **Trees never touch water.** Rejected at discovery if the centre is underwater, a bed, within
      2 blocks of a bank, or within `TREE_WATER_MARGIN = 2` of any column with a non-empty water
      range (1-column-dilated 2D mask from the tiles). At apply, logs write only air or leaves and
      leaves only air; a target that is water is **skipped, not replaced**, so a shore canopy loses
      only its over-water blocks. Trunk base is sunk to the lowest of the 4 neighbouring tops and
      rejected if that is below `SEA+1`.
- [x] H3.6 Is tree growth ever simulated, or generation-only?
      **Generation-only.** No sapling, no growth tick, no leaf decay, no tree runtime state.
      After generation, trees are ordinary blocks; player-placed logs/leaves are ordinary edits.
      Growth would be a later runtime feature writing through `can_edit`/the edit path.
- [x] H3.1 Species count and per-biome table?
      Defined by content-pack records with climate ranges and biome affinity weights; count is
      data, not a constant.

### H4 Small scatter
- [x] H4.1 Grass, flowers, boulders, snow caps: same pipeline or a cheaper one?
      **Same pipeline (stage 5) but hash-placed per column**, no candidate list: keyed hash vs
      a density curve, plus a low-frequency patch field so flowers clump.
- [x] H4.2 Are they rendered as blocks, cross-quads, or instanced props?
      **Real blocks with a `render_shape` enum** (`cube`, `cross_quad`, `snow_layer`) and no
      collision. Cross-quads never occlude, never block skylight, are not solid for span scan or
      `top_solid_y`, emit into the cutout pass with no greedy merging, and disappear at LOD1+.
      `replaceable = true` so placing over them works.
- [x] H4.3 Do they survive edits nearby?
      **Yes** — they are ordinary blocks in the section; only their own block is edited.

---

## I. Chunk data model

### I1 Storage
- [x] I1.1 Is a block `u16` with a registry, a palette per section, or a bit-packed ID?
      **`Section = Uniform(BlockId) | Paletted { palette: Vec<u16>, bits: u8,
      data: Box<[u64]> }`.** `bits ∈ {1,2,4,8,16}`, smallest that fits, packed LSB-first into
      `u64` words and never straddling a word. Palette is unordered and append-only within a
      section; nothing may depend on index values. Growth repacks in one pass; demotion to
      `Uniform` is lazy (save or timer). Disk stores **stable names**, never runtime ids.
      Section carries an opaque/solid count so empty/full checks are O(1). Hashes are over
      sorted palette plus decoded indices, never raw words. Max 65,536 block types.
- [x] I1.2 Is the array `YZX` or `XYZ`, and what is the index formula?
      **`index = x + 32*(z + 32*y)`**, x fastest, one `y`-layer contiguous.
      `section_index(x,y,z)` and its inverse are defined once; no loop inlines it. Shifts
      `x | z<<5 | y<<10`, local coords masked `& 31`. One bulk decode into a flat
      `[u16; 32768]` scratch is the only decode path. Index order is part of the save format
      version.
- [x] I1.3 Are sections 32³ with 4 per column, and is a section the unit everywhere?
      **32³ sections, 12 per column.** The chunk type, mesher, save records, and LOD units
      stay unchanged; only column metadata grows.
- [x] I1.4 Is there a uniform-section optimisation (all air, all stone)?

### I2 Baseline
- [x] I2.1 What exactly is hashed for a baseline: blocks, palette, or both?
      **Decoded block ids** (palette sorted, indices decoded). Stored per section as
      `baseline_hash` plus an `edited` bit; taken at first generation, never rewritten.
- [x] I2.2 Is the baseline ever mutated in place, or always replaced?
      **Sections are mutated in place by edits** — that is what makes them effective sections —
      while `baseline_hash` remains the record of what generation produced.
- [x] I2.3 How does a stage request neighbour blocks without loading a neighbour chunk?
      **Per-worker padded scratch.** Each worker owns one reusable `PaddedBlocks` `[u16; 34³]`:
      brief read borrow of the target plus 26 neighbours (Uniform fills by constant, Allocated by
      bulk decode), copy, release all borrows, then run lock-free.
      `padded_index(x,y,z) = (x+1) + 34*((z+1) + 34*(y+1))`, `x,y,z ∈ -1..=32`. Vertical
      neighbours come from the column's 12-slot array; out-of-world vertically is bedrock below /
      air above; horizontal world edge mirrors the edge row. Light uses a parallel nibble scratch
      and exchanges column borders instead of borrowing live neighbours.
- [x] I2.4 What padding does a stage receive, and is it copied or borrowed?
      **1 block on all 6 faces, 12 edges, and 8 corners, copied** (~79 KB per worker). A job
      records the revision of every section it copied, so a result is discarded if any changed.
      Unit test: padded copy equals direct world reads for every cell, including Uniform
      neighbours, edges, and corners.

### I3 Effective chunk
- [x] I3.1 Is the effective chunk materialised, or is the overlay read on demand?
      **No effective-chunk type.** The `Section` enum is the only block store, held in the
      column's 12 slots. Main-thread gameplay uses a short read borrow plus a per-frame
      last-decoded section cache; jobs use the padded scratch (I2.3) and never live borrows.
      Writes are a short write lock on one section, main thread only.
- [x] I3.2 What is the revision type, and does it fit in `u64` forever?
      **Per section `section_rev: u64`**, bumped once per edit batch that touches it and on
      Uniform↔Allocated promotion. Column revision is derived `max(section_rev)` and never stored.
      **`border_rev` per face** bumps when a face-adjacent cell of the neighbour changes, so a
      neighbour's mesh/AO goes stale only when a cell it actually read changed. Revisions are
      runtime state starting at the saved world revision; never on disk. Derived data (light,
      mesh, LOD, `HeightTile` edits) is keyed by `section_rev`. Optional 64-entry decoded LRU is
      a pure cache, never a second source of truth.
- [x] I3.3 Does collision read the effective chunk or a cached snapshot?
      **Direct reads through `world.get_block(x,y,z) -> BlockId`.** Column `floor_div(x,32)`,
      section `floor_div(y,32)` (0..=11), local `& 31`. Out of world: `y<0` bedrock,
      `y>=WORLD_HEIGHT` air, outside `WORLD_HALF_EXTENT` the boundary wall. **An unresolved
      column returns `Unloaded`, and physics treats it as solid** so the player cannot fall
      through. One writer (main thread), so main reads need no lock; workers never use this API.
      `get_region(aabb, &mut buf)` takes one borrow per section for the player's swept AABB;
      raycasts cache the last section. Edits affect collision in the same tick. Test: `get_block`
      matches the padded scratch everywhere, and an edit is visible to collision and raycast
      immediately.
- [x] I3.4 What is the memory cost per loaded chunk at full radius?
      **Targets:** ≤40 KB resident per typical column (sparse sections + light nibbles + mesh),
      ≤2.5 MB per worst-case LOD0 section mesh, measured per biome class in CI. Exceeding is a bug,
      not a tuning knob. Counters exist per class (S1.7).

---

## J. Overlay and persistence

### J1 Edits
- [x] J1.1 Is an edit `(pos, old, new, revision)`, or does it coalesce?
      **Per-block records with old value:**
      `{x,y,z, old: BlockName, new: BlockName, revision: u64, author: u32, batch: u32}`, stored
      as **stable names**, never runtime ids. One revision per committed batch (so undo reverts
      a batch atomically). Append-only; coalescing happens on read only.
- [x] J1.2 Who assigns revision: main thread, world, or a monotonic clock?
      Monotonic `u64` from the world header, incremented once per committed batch.
- [x] J1.3 Is there an author field for future multiplayer?
      **Yes**, `u32` stable player id, `0 = system/tool`.
- [x] J1.4 Are edits to air recorded, or is removal implicit?
      **Every committed block change is recorded**, including `air → block` and `block → air`
      (undo needs `old = air`). Nothing is recorded when `old == new`.

### J2 Ordering
- [x] J2.1 Does last-write-wins apply, and what is the tie-break?
      **Total order `(revision, batch, seq)`.** `revision: u64` per committed batch,
      `batch: u32` within the revision, `seq: u32` position in the batch. Highest wins at a
      coordinate; within a batch the later `seq` wins and the net effect is first old / last new.
      `author` is metadata, used only as a defensive tie-break (then log offset), and an equal key
      raises `InvariantViolated` in debug/CI. **Saved sections are authority; the log is never
      replayed onto them** except journal recovery, once, for records newer than the section's last
      save. Undo is a new batch with a new revision; compaction keeps first-old/last-new in total
      order. Tests: out-of-order arrival still writes in revision order, two batches on one block,
      equal-key invariant, crash-during-recovery idempotence, compaction net-diff equality.
- [x] J2.2 Is the overlay a map, a log, or a log with periodic snapshots?
      **Segmented append-only log per 32³ region**, loaded and pruned with its sections.
- [x] J2.3 Can an edit be reverted to baseline, and does that remove the record?
      **Undo only if the block still equals the record's `new` value**, otherwise reported as a
      conflict, never forced. Records are never removed by undo.

### J3 Durability
- [x] J3.1 Journal flush policy?
      **Group commit on a dedicated IO thread.** Game thread pushes batches into a bounded
      channel; IO thread appends. **Flush every 250 ms or 64 batches; fsync on pause, quit,
      before any section save, and every 10 s.** If the channel fills, coalesce in memory rather
      than block the game thread. Intervals are config, not spec constants.
- [x] J3.2 Is compaction incremental or full?
      **Incremental:** compact old segments to net diffs (first old, last new, no per-batch
      history) on save, in batches.
- [x] J3.3 What happens on a partial journal write?
      Framing `[len u32][revision u64][payload][crc32]`; a torn tail is detected and truncated
      on recovery. Replay only records newer than the last committed section save.
- [x] J3.4 Is `fsync` used, and how often?
      Section save is write-temp → fsync → rename → fsync directory; the covered journal prefix
      is dropped only after that.

### J4 Format
- [x] J4.1 Does the save store generated baselines, or only edits plus hashes?
      **Effective (post-edit) sections are saved for every generated column; a saved column is
      never regenerated.** Header: description hash, generator version, platform tag, save
      format version. Each section also stores `baseline_hash` (decoded ids as first generated)
      plus an `edited` bit. An optional verify mode regenerates unedited sections and reports
      drift. Dense v3 saves load by promoting uniform sections.
- [x] J4.5 **Fork to confirm.** J4.1 contradicts "baseline is regenerable + disposable 2 GB
      cache" in the pipeline doc. With always-saved effective sections there is no baseline
      cache, no regeneration path, and migration must transform stored data rather than
      regenerate it.
- [x] J4.2 Is the baseline written at all, or regenerated on load?
      **Effective sections only**, plus hashes for verification.
- [x] J4.3 Is there a per-world or global cache budget, and what evicts first?
      Unclear now that baselines are not cached on disk.
- [x] J4.4 Is the save format documented byte-for-byte before shipping?
      **Yes.** [`docs/dev/save-format.md`](../dev/save-format.md) pins the header, the edit journal
      (framing, policy, writer contract), the v2 `TSEC` section record (name-sorted palettes, derived
      word count, `baseline_hash` derivation), backup rotation, and the migration journal. Code
      exists for the header, journal framing/policy, and section record; the writer, IO thread,
      migration, and `world-repair` are the first persistence milestone and MUST NOT change a byte.

### J5 Migration
- [x] J5.1 Pin, migrate, fork: who chooses, and when is the choice shown?
      **Explicit dialog, only when the world cannot open as-is** (T1.7). Order: preflight (free
      space ≥ world + backup + 10%, verify every crc, take an exclusive lock) → verified backup →
      migrate → header switch → report. **Dry-run** scans read-only and writes nothing.
- [x] J5.2 How are unmappable edits reported, and can the report be exported?
      **`migration-report-<date>.json` is always written**, even on failure or cancel: sections
      read/rewritten/unchanged; unknown/deprecated names with counts and aliases; edit-log entries
      per section and unmappable/conflicting edits with examples; `baseline_hash` drift; warnings;
      bytes and elapsed; backup path; tool/engine versions. Exportable from the dialog; CLI prints a
      summary and exits 0 ok / 1 warnings / 3 failed.
- [x] J5.3 Is migration resumable?
      **Yes.** Sorted section order, per-section temp→fsync→rename, and an append-only
      `migration.journal` (section key, new crc, old crc, per-record crc, torn tail truncated).
      A section in either format is accepted by crc, so resume is idempotent. The header carries
      `migration_state = InProgress`, which opens only the resume dialog (Resume / Restore backup /
      Cancel); a restored backup is verified before replacing.
- [x] J5.4 Does migration ever touch the original files?
      **Only in the in-place Migrate path, after a verified backup** (hardlinks explicitly do not
      count). **Fork** copies to a new `world_id` and migrates the copy; **Pin** writes nothing.
      Both are tested by comparing the original directory hash before and after. Derived caches are
      deleted, not migrated.

---

## K. Streaming and residency

### K1 Shape
- [x] K1.1 Horizontal radius in chunks, and is it square, circular, or frustum-biased?
      **Square Chebyshev ring, `VIEW_RADIUS = 16`** (1,089 columns), runtime config. Membership
      is `max(|dx|,|dz|) <= R` on `floor_div` column coords. Load nearest-first by ring, then
      Euclidean distance, then index. **Meshing ring is `R-1`**, so the outer generated ring is
      never meshed without neighbours. Readiness per column requires its 8 neighbours resolved.
      Unvisited columns are never generated for the ring; distant LOD is a separate decision.
- [x] K1.4 What is the load-distance vs unload-distance hysteresis?
      **Load at `R`, unload at `R + UNLOAD_MARGIN` (2).** At most `(2*18+1)² = 1,369` resident
      vs 1,089 visible. Checked on column change, not per frame. Unload is queued into an LRU
      with a ~5 s grace period; `R+1..R+2` stay resident but unmeshed and unprioritised, so they
      cost memory only. Dirty columns are saved before eviction. Memory budget is a hard cap and
      evicts farthest-first regardless of margin.
- [x] K1.3 Is the ring finite (2,000-block world) or unbounded for the first world?
      **Finite square world**, half-extent `WORLD_HALF_EXTENT` (default 1,024 blocks, so ~2,048
      across; multiple of `SECTION_SIZE`; part of the description hash). Columns exist only for
      `|x|,|z| < WORLD_HALF_EXTENT`. An edge-ocean mask (`curve_edge(dist/EDGE_BAND)`,
      `EDGE_BAND ≈ 256`) pushes `base` toward the ocean floor before hydrology, so the outer ring
      is deep sea and no landmass is cut. Beyond the boundary nothing streams or generates; tile
      halos clamp by mirroring the edge row. An invisible wall at `WORLD_HALF_EXTENT-2` plus a
      one-time notice; entities and projectiles clamp the same way. Edits and features never
      write outside the column set. `Z_HALF_SPAN = WORLD_HALF_EXTENT`, so both `z` edges are the
      coldest.
- [x] K1.2 Vertical policy for a 384-tall world: all four sections always, or lazy?
      **Full column always:** all 12 sections generate, light, mesh, and upload together.
      A column is the readiness unit. Cost per column is 12× the old one.
- [x] K1.5 With full-column residency at radius R, what is the memory and load-time cost?
      **Sparse sections inside a dense fixed array.** 12 slots, each `Uniform(id)` or
      `Allocated(Box<Section>)`. Uniform costs one slot value and no allocation; only mixed
      sections allocate a palette plus packed indices. Readiness is still per column: ready only
      when all 12 slots resolve, so "missing" never exists downstream. Slot lookup is O(1);
      neighbour reads treat `Uniform` as constant. Generation writes `Uniform` for homogeneous
      sections; edits may demote lazily. Save format mirrors the slot kinds, and old dense saves
      load by promoting uniform sections. Cache per-column `top_solid_y` min/max so mesher and
      lighting skip uniform air bands without scanning.

### K2 Priority
- [x] K2.1 Are the four classes (urgent, visible, prefetch, background) final?
      **Strict class order, distance within a class:**
      `0 urgent` (player column + 8 neighbours, pending edits, collision/lighting needs) →
      `1 visible in frustum inside R-1` → `2 in ring, outside frustum` → `3 prefetch` (only
      when speed > `PREFETCH_MIN_SPEED`, capped ring) → `4 background` (compaction, cache, LOD).
- [x] K2.2 Within a class, what orders work: distance, direction, revision age?
      **Chebyshev ring, then Euclidean distance, then column index.** Frustum test is coarse
      with `FRUSTUM_SLACK ≈ 20°`; hysteresis prevents head-turn reshuffles. Re-evaluated at most
      every 100 ms or on column change. Priorities never affect output — only scheduling.
- [x] K2.3 Can a job be preempted mid-stage, or only before it starts?
- [x] K2.4 What is the queue depth cap per class?
      **Per-class caps on column-level requests:** urgent 64 (overflow is a bug: log
      `InvariantViolated` and admit), visible 1,200, ring 1,200, prefetch 128, background 64
      (runtime config). Requests are keyed `(coord, stage, revision)`, so a re-queue replaces rather
      than duplicates; total depth is bounded by distinct columns. Overflow: classes 3/4 drop the
      farthest; 1/2 drop farthest and count `dropped_before_start` (re-requested automatically);
      class 0 is never dropped or blocked. Each class has its own queue; the scheduler picks the
      highest non-empty class with the K2.1 starvation age boost. In-flight is 1 per worker plus
      `GENERATE_AHEAD = 12`. Counters per class: depth, high-water mark, drops; a sustained cap
      raises the K4.3 lag signal.

### K3 Cancellation
- [x] K3.1 Epoch per request, per chunk, or per world?
      **Per request: `(coord, stage, revision)`.** `revision` is a per-key `u64` bumped by an
      edit to that column, an unload, a re-queue from a class change, or a radius change. A
      finished result is applied only if all three still match; otherwise discarded and its
      buffers freed. No in-job abort checks; jobs stay ≤ ~10 ms. Three counters:
      `dropped_before_start`, `discarded_stale`, `applied`. Pure-function results (tiles,
      hydrology, candidates) are keyed by `(description hash, coord)` and go into the derived
      cache rather than being discarded. Dependent stages re-check their inputs' revisions at
      admission. A teleport drops unstarted jobs beyond the new `R+2` in one pass and bumps no
      revisions.
- [x] K3.2 Is a dropped result counted separately from a cancelled job?
      **Yes:** `dropped_before_start` vs `discarded_stale`.
- [x] K3.3 How long can a stale job run before it is killed?
      **Never killed** — jobs run to completion; staleness is decided at apply time.
- [x] K3.4 Does cancellation free memory immediately?
      **Yes** — the S1.7 accounting subtracts the reservation on discard.

### K4 Budgets
- [x] K4.1 Work per frame per stage, in ms or items?
      **`MAX_COLUMNS_PER_FRAME = 2`, `MAX_UPLOAD_BYTES_PER_FRAME = 4 MiB`**, runtime config.
      Class 0 is exempt from the column cap but not the byte cap. `APPLY_TIME_CAP = 3 ms` is a
      safety stop. A column's upload may split across frames per section. Post-teleport the caps
      rise (8 columns / 16 MiB) until the player's 3×3 is resident.
- [x] K4.2 Max in-flight jobs per stage?
      **`GENERATE_AHEAD = 12` ready columns**; workers pause when the ready queue is full.
- [x] K4.3 Degradation order under load, exactly?
      **Load-shedding ladder, triggered by lag, not memory.** Lag = class 1 queue wait p95 >
      `LAG_WAIT` (500 ms) or the ready queue empty while class 1 is pending, for `LAG_WINDOW = 2 s`.
      Shed one step per window, stop when lag clears: 1 background (compaction, cache, LOD rebuild)
      → 2 stop new LOD2 → 3 stop new LOD1 → 4 stop prefetch → 5 pull fog in by 2 columns and drop
      `VIEW_RADIUS` by 2 (min 8, log once). **Never shed class 0 or class 1.** Edit workers
      untouched. Effective radius is `min` of this and the S1.7 ladder. Restore in reverse, one step
      per `RESTORE_WINDOW = 10 s` lag-free. In-flight jobs are not cancelled (K3.1); only unstarted
      jobs drop. Step shows in overlay mode 0. Teleport boosts the 3×3 first and only sheds after
      `LAG_WINDOW`.
- [x] K4.4 What is the teleport rule: drop everything, or keep the nearest column?
      **One idempotent pass on a column jump (>1 column).** Recompute membership; drop every
      unstarted request beyond `R+2` (counted `dropped_before_start`); re-derive classes; rebuild
      class 0 first (new 3×3 plus pending edit rebuilds); **bump no revisions**; free resident
      columns beyond `R+2` immediately except dirty ones (saved first); LOD outside its radii
      evicted; in-flight jobs finish and are applied if still valid, else `discarded_stale`;
      boost caps until the 3×3 is resident; reset the lag timer; clear prefetch. Input and gravity
      pause behind the loading cover until the 3×3 + 8-neighbour footprint is ready; short hops
      where the 3×3 is already resident skip the cover. Jumps clamp to the world edge first.

---

## L. Lighting

### L1 Sky light
- [x] L1.1 Is skylight 15 levels with flood fill, or a coarser model?
      **15-level BFS per column, 1-block border into the 8 neighbours; 4-bit packed nibbles per
      `Allocated` section.** `Uniform` air = constant 15, `Uniform` solid = constant 0, carried by
      the slot. Pass 1: vertical seed 15 down from `WORLD_HEIGHT-1` to the first opaque block via
      the cached top span (O(1) for open air). Pass 2: lateral BFS seeded only at sky/shadow
      boundary cells, `-1` per step, no loss straight down. Opacity from the manifest: air and
      cross-quads 0, leaves 1, opaque 15. Lit only when the column and 8 neighbours are resolved;
      border read once into a padded scratch buffer.
- [x] L1.2 How is vertical propagation through a 384-tall column handled without loading above?
      **One job per column, all 12 sections, seeded from the ceiling.** No sky summary, no
      cross-job dependency. Edits relight incrementally in the touched section plus a 15-block
      halo. Uniform/Allocated promotion allocates or drops the nibble array. Light is never saved
      or hashed.
- [x] L1.3 Is sunset/sunrise colour applied per block or in the shader only?
      **Shader only.** Light data is two 4-bit channels; colour is
      `max(sky_color(t) * L(sky), block_tint * L(blk)) + ambient`, where `L` is a 16-entry LUT
      (no `pow` in shaders) and `sky_color` is shared with fog (O3.1). Block light uses a fixed warm
      tint. Biome tint and time of day multiply in the shader. Never saved, hashed, or part of
      identity.
- [x] L4.3 Do LOD seams blend light or accept a step?
      **Accept, with one ease.** No blending pass and no second light representation. LOD faces use
      the vertical-scan sky value with `blk = 0`; within `LOD_BLEND_COLUMNS = 2` of a boundary the
      LOD1 value eases toward the adjacent LOD0 face when resident. Fog hides the rest. The N2.4 pop
      fixtures include a brightness diff, and a dusk fixture covers sky/fog divergence.

### L2 Block light
- [x] L2.1 Which emitters exist in the first world?
      **Torch (14) and lava (15).** Emission is a per-id manifest field `0..=15`, frozen by T1.5.
      Torch: cross-quad, no collision, `replaceable = false`, needs a support face and breaks with
      it (one edit batch). Lava: liquid, block-light opacity 0, collision per manifest — **not
      generated by terrain in v1** (F2.1), only placed by tools and fixtures. Same BFS as skylight,
      `-1` per step; water does not attenuate block light. LOD faces carry `blk = 0`. Tests: torch
      in air, torch underwater, lava underwater, two emitters 10 apart with a block placed and
      removed, emitter on a section and column border.
- [x] L2.2 Is light stored per block, per section corner (AO), or both?
      **Per block, 4-bit nibbles**, two channels (sky, block). AO is per vertex in the mesh (L4.1).
- [x] L2.3 Is light RGB or single-channel?
      **Single channel.** Colour comes from the shader and biome tint.
- [x] L2.4 Does water attenuate light?
      **Skylight only, `WATER_SKY_ATTEN = 1` per block.** Block light passes water with the
      normal `-1` per step, so underwater lamps work. Manifest fields: `sky_atten` (0 default,
      1 water) and `block_opacity` (0 water). This supersedes the earlier "water opacity 2".
      Renderer adds `MIN_UNDERWATER_AMBIENT` plus a blue-green tint so depths read as water, not
      as a bug.

### L3 Consistency
- [x] L3.1 What is the exact border rule for a neighbour edit?
      The edited section plus a 15-block halo plus the 8-neighbour border is relit.
- [x] L3.2 Is light computed on the effective chunk, and does an overlay edit dirty it?
      **Yes** — light is a pure function of saved blocks, recomputed on load, never persisted.
- [x] L3.3 Is incremental repair allowed to differ from full recompute?
      **No — bit-identical is required.** Two-phase dark-removal then re-light; each cell is a max
      over sources, so order cannot matter. CI fuzzes ≥1M random edits and asserts byte equality;
      release samples 1 in 1000 repairs and self-heals on mismatch.

### L4 LOD and AO
- [x] L4.1 What is the AO rule in the greedy mesher, and how does it affect merging?
      **4 corners per face, 0..3, packed 2 bits each, in the merge key.** Only opaque full cubes
      occlude. Triangulation flips on the anisotropy test. Flat ground keeps uniform AO 3, so it
      still merges. Water and cross-quads skip AO; LOD1+ merges on block id only.
- [x] L4.2 Is coarse light mipped, recomputed, or sampled from level 0?
- [x] L4.3 Do LOD seams blend light or accept a step?

---

## M. Meshing

### M1 Greedy rules
- [x] M1.1 What merge key includes: material, light, AO, tint, transparency?
      **`(block id, face direction, sky nibble, block nibble, packed AO)`** for opaque; water
      merges on `(normal, sky, block)` with no AO; LOD merges on block id only.
- [x] M1.2 Is the mesher chunk-local with a neighbour border, and how wide?
      **Section-local, 1-block border** decoded once from the padded scratch buffer.
- [x] M1.3 Are diagonal AO occluders included in the key?
      **Yes** — 4 corner values, 2 bits each.
- [x] M1.4 Is output ordering deterministic and sorted?
      **Yes** — fixed face order, fixed per-section ranges (opaque, cutout, water). Mesh is
      derived and never hashed, but ordering is still deterministic for tests.

### M2 Water and transparency
- [x] M2.1 Separate pass, separate mesh, or per-face material flag?
      **Separate ranges inside one per-section buffer**, drawn in a blended pass after opaque and
      cutout.
- [x] M2.2 Are water faces culled against water, air, or both?
      **Only water/air faces exist.** Water/water never emits; water/solid never emits (the solid
      face is drawn by the opaque mesher, since water does not occlude). This supersedes the
      "water/solid interfaces" wording in D7.1.
- [x] M2.3 Is there a surface-only water mesh, and does it animate?
      **Top faces are greedy-merged per section** (a full ocean section is one quad). Static
      geometry; only a fragment UV scroll animates. Surface at full block height in v1.
- [x] M2.4 Cutout blocks (leaves, grass): alpha test, separate pass, or geometry?
      **Cutout range in the same buffer**, no greedy merging, two crossed quads for scatter.

### M3 Scale
- [x] M3.1 Vertex format?
      **8-byte packed quad record, vertex pulling, no index buffer.**
      `x:5 y:5 z:5 | w1:5 h1:5 | normal:3 | ao:8 | sky:4 blk:4 | tile:16 | tint:4`.
      Six vertices per quad from a shader table; triangulation flip computed in the shader from
      AO; UV derived from local position with a repeat sampler; section origin per draw/indirect.
      Bit layout is a renderer format bump only.
- [x] M3.2 Is a mesh ever split into sub-meshes for culling or draw order?
      **One mesh per section, only if it emits a face** (typical column: 2–5 meshes). Sections
      sub-allocate inside a **per-column vertex arena** so buffer objects stay at 1–2 per column;
      draws are indirect/multi-draw; frustum culling per section AABB. Edits rebuild the touched
      section plus border-affected neighbours, never the column. Upload splits per section under
      the K4.1 cap.
- [x] M3.3 What is the target vertex and triangle budget per chunk?
      **≤25,000 quads per emitting section (≤150k triangles at 6 verts/quad), ≤2.5 MB mesh**
      worst case, **≤3,000 LOD nodes** globally (N2.4). Benchmarks assert per biome class; exceeding
      is a bug.
- [x] M3.4 Is meshing parallel per chunk with no cross-chunk writes?
      **Yes**, per section, reading only the padded scratch copy.

---

## N. LOD hierarchy

### N1 Levels
- [x] N1.1 Five levels confirmed: 1, 2, 4, 8, 16 blocks per sample?
      **No — three levels in v1:** LOD0 = 1 block/cell, LOD1 = 2, LOD2 = 4. All use 32³ cell
      arrays in the `Section` layout, so LOD1 covers 64³ blocks and LOD2 128³. Radii:
      `LOD0_RADIUS = 16`, `LOD1_RADIUS = 32`, `LOD2_RADIUS = 64` (runtime config).
- [x] N1.2 Section size stays 32³ samples at every level?
      **Yes**, same `Section` type, same index order.
- [x] N1.3 Is level 0 the only authority for collision and edits?
      **Yes.** LOD is derived, never saved, never hashed, never authority.
- [x] N1.4 How is a coarse section generated without its children?
      **Downsample rule, level to level:** per 2×2×2 group, if any solid take the most frequent
      solid id (ties lowest id); else water if any water; else air. Scatter dropped, ores dropped,
      logs/leaves kept. Unsaved columns beyond `R` generate LOD0 on workers then downsample, so
      LOD1 always agrees with future LOD0.

### N2 Selection
- [x] N2.1 Screen-space error, distance, or both?
      Distance (Chebyshev radii) in v1.
- [x] N2.2 What is the hysteresis to prevent level thrash?
      **2 columns**, same style as K1.4.
- [x] N2.3 Is selection CPU-side, GPU-side, or both with CPU fallback?
      CPU-side in v1; indirect draws only.
- [x] N2.4 What is the max node count for the target radius?
      **`LOD_NODE_BUDGET = 3,000` emitting LOD sections** (soft target ~2,000) at `R=16`,
      `LOD1_RADIUS=32`, `LOD2_RADIUS=48`, measured at world centre (worst case). Non-emitting
      sections are not nodes. Exceeding it fails CI, never a runtime clamp; runtime may lower LOD
      radii but never raise them past budget. Counters in overlay mode 8.

### N3 Consistency
- [x] N3.1 Conservative any-solid downsample rule confirmed?
      **Most-frequent-solid wins** (ties: lowest id). Any-solid is rejected: it inflates distant
      terrain and duplicates the D7.1 water rule badly. The pipeline doc is updated to match.
- [x] N3.2 How is a coarse/fine boundary stitched?
      **Skirts:** a downward quad strip of `SKIRT_DEPTH = 2` cells along the lower-resolution
      edge. A LOD(n+1) column draws only where its covered LOD(n) columns are not
      resident-and-meshed, so there is no overlap and no gap.
- [x] N3.3 Is a coarse section hidden while its children generate?
      **Yes** — the boundary rule above is the whole mechanism.
- [x] N3.4 How much pop is acceptable, measured how?
      **≤1.0% pixel diff** (per-channel threshold > 12/255) at each switch distance, same pose one
      column either side, fog and time of day fixed, on mountain/coast/forest seeds. Fixtures:
      ridgeline, coastline, canopy edge, river crossing. Hysteresis is part of what is tested, so
      moving back and forth does not toggle.
- [x] N3.5 How does a coarse section get its light?
      **No BFS, no stored light.** Per-face approximation at mesh time: sky = 15 if the air-side
      cell is open to the ceiling, else `15 - SHADE_PER_CELL * cells_below_first_opaque`, minus
      `WATER_SKY_ATTEN` per water cell above. Block light 0, AO 0. Merge key includes the sky
      nibble. A shader curve eases the LOD0/LOD1 step; fully exposed coarse faces match LOD0.

### N4 Invalidation
- [x] N4.1 Does a level-0 edit invalidate all ancestors, lazily?
      **Yes** — covering LOD1/LOD2 sections rebuild as class 4 work.
- [x] N4.2 What is the latency budget for L3/L4?
      **LOD1 ≤2 s p95, LOD2 ≤10 s p95** after the edit commits, idle pool, class 4 work that never
      delays class 0/1 (shed first under K4.3 lag). Edits bump `section_rev` and mark covering LOD
      sections dirty, **coalesced with `DEBOUNCE = 500 ms`** for LOD1 and 2 s for LOD2 (which reads
      the rebuilt LOD1 first). Until replacement the **old mesh keeps drawing** (no hole, no
      flicker); a stale-revision result is discarded and re-queued. Entering the LOD0 ring
      prioritises a dirty column's LOD as class 3. Overlay mode 8 shows age per LOD section and
      flags over-budget; `lod_stale_over_budget` feeds nightly metrics. Dirty marks are runtime
      state, never saved.
- [x] N4.3 Does an edit at coarse level ever write down to level 0?
      **Never.** LOD is read-only derived data.

---

## O. Rendering and precision

### O1 Draw path
- [x] O1.1 One draw per chunk, batched, or indirect?
      **Multi-draw-indirect from the per-column arena** (M3.2); LOD1+ uses the same records and
      the same passes, so there is no LOD-specific pass.
- [x] O1.2 Is frustum culling per chunk, per section, or hierarchical?
      **Per section AABB against 6 planes (conservative positive-vertex test), in
      camera-relative coords; per column AABB tested first** (32×384×32, tightened to the cached
      `top_solid_y` min/max), so a rejected column skips all its sections. Meshless sections are
      never tested. Budget < 0.5 ms at `R = 16`, asserted by a benchmark. The visible-section
      list feeds the indirect draws and is the single hook for future occlusion.
- [x] O1.3 Is occlusion culling in scope for the first world?
      **No. Frustum only in v1.** No horizon test, no GPU queries. Counters (columns/sections
      tested, sections drawn, triangles) exist so a later decision is measured.
- [x] O1.4 Are opaque, cutout, and water separate passes with separate order?
      **Fixed order:** 1 opaque (depth write, front-face cull) → 2 cutout (alpha test, depth write,
      no cull) → 3 depth copy after cutout, skipped when no water section is near → 4 water
      (blend, depth test, no write, no cull, far→near by column distance) → 5 particles/break
      effects → 6 overlay/highlight (depth tested, after water, so it reads through the surface)
      → 7 per-pass fog in each fragment shader plus underwater tint → 8 UI. Cutout leaves alpha-test
      and never blend, so they need no per-quad sort. Blend states and depth modes are constants in
      one renderer module. Screenshot fixtures: river in front of a tree, canopy over the sea,
      submerged camera, highlight under water.

### O2 Precision
- [x] O2.1 At what travel distance does `f32` become visible?
      Not within a 2,048-block world: `f32` spacing near 2048 is ~2.4e-4 blocks.
- [x] O2.2 Camera-relative rendering, origin rebase, or both?
      **Camera-relative from day one, no floating origin.** World positions are `i32` blocks;
      camera and entities `f64`. Per frame `cam_block = floor(cam_pos)` (`i64`) and
      `cam_frac = cam_pos - cam_block` (`f32` in `[0,1)`). Per draw
      `delta = (section_origin - cam_block)` converted exactly to `f32` (< 2^24). Shader:
      `pos = delta + local_pos - cam_frac`. View matrix is rotation-only. Same convention in every
      pass. Picking and collision stay in world space (`f64`/`i32`). Unit test asserts exact
      round-trip up to max view distance; screenshot fixture at ±2000 matches the origin one.
- [x] O2.3 Does rebasing invalidate anything, or is it a render-only transform?
      **Moot** — there is no rebase. Nothing cached depends on the camera.

### O3 Edge
- [x] O3.1 Does fog hide the terrain edge at max radius?
      **Distance fog, one shared pinned polynomial function in every fragment shader.**
      `FOG_START = 128`, `FOG_END = LOD2_RADIUS_BLOCKS - 64` (default 1,984), both derived from the
      active LOD radii so an `S1.7` radius cut moves fog with it. Euclidean distance from the
      camera-relative position, never from depth, so all passes agree. Mild height term
      (`1 + K*clamp((SEA+32-y)/64, 0, 1)`), capped so `f` stays in `[0,1]`. Colour = sky at the
      view's horizon band. Far clip = `FOG_END + 32`. Underwater replaces it with the L2.4 curve
      and `MIN_UNDERWATER_AMBIENT`. Water thickness tint applies before fog. Renderer config only,
      never identity.
- [x] O3.2 Is there a sky/terrain blend at the horizon?
      **Yes, via fog to the sky horizon colour**, plus a renderer-only flat water quad at `SEA`
      extending to the far clip beyond the last column, drawn in the water pass with the same fog,
      so the horizon never shows a cut.
- [x] O3.3 Are water and fog composited in a defined order?
      **Water thickness tint first, then fog**, in the water pass's fragment shader.

---

## P. Performance and budgets

- [x] P1.1 Reference hardware for every target number?
      **2020 mid-range desktop**, 1080p, 60 FPS, 4–6 core CPU, GTX 1660 / RX 5600 class.
- [x] P1.2 Frame budget split: generation, lighting, meshing, upload, render?
      **CPU at 16.6 ms:** render CPU (culling ≤0.5 ms, submission, uploads) ≤10 ms; apply ≤3 ms
      (count/byte caps primary, `APPLY_TIME_CAP` backstop); world/input/UI/physics/edits ≤1 ms;
      ~2.5 ms slack never spent by design. GPU target ≤12 ms, budgeted separately. **Boost**
      (3×3 unresolved): apply may use 8 ms and 8 columns/16 MiB, frame cap 33 ms, cover hides it.
- [x] P1.3 Target radius and FPS: 64 chunks at 60 FPS confirmed as the gate?
      **`VIEW_RADIUS = 16` at 60 FPS, no frame over 20 ms while moving**, with LOD2 at the
      horizon. The old 64-chunk gate is superseded by this plus the LOD radii.
- [x] P1.4 Memory ceiling per world, and what happens at the ceiling?
- [x] P1.5 Max worker threads, and is it `cores - 1`?
      `N = available_parallelism`; IO 1; `EDIT_WORKERS = 1 (N<=8) else 2`;
      `STREAM_WORKERS = max(2, N - 2 - EDIT_WORKERS)`. Runtime config.
- [x] P1.6 Does a chunk load ever block the main thread, and for how long?
      **Never.** No disk IO, no worker lock waits, no job waits, no GPU readbacks, no fence waits
      except swapchain present. Results arrive on a bounded ready queue polled with `try_recv`;
      IO is on its own thread. A debug watchdog flags any main-thread call >2 ms outside render
      submission and apply, naming the call site. Overrun defers to the next frame; it never borrows
      from render. Render >10 ms for N frames feeds the K4.3 lag signal.
- [x] P1.7 What is the cost of a single edit end-to-end (remesh latency)?
      **50 ms p95, 100 ms p99**, input → visible frame. The game thread writes the block
      immediately (same frame for collision, pick, and the log); reserved edit workers relight
      (≤10 ms) then remesh (≤15 ms); upload ≤4 ms. The touched section uploads before neighbour
      remeshes. The old mesh keeps drawing until the new one is ready; duplicate rebuilds
      coalesce to one. Bulk edits are exempt and batched.
- [x] P1.8 GPU upload budget per frame in bytes?
      4 MiB (K4.1); 16 MiB during teleport recovery.
- [x] P1.9 Are generation and meshing ever on the same pool?
      **Yes** — one streaming pool and priority queue, with 1 worker reserved for class 0. Edit
      jobs never share the streaming pool.
- [x] P1.10 Is there a "fast mode" that lowers quality to hit budget?
      **No fast mode.** Only the K4.3 ladder and the S1.7 memory ladder (shared effective radius),
      both visible, logged once per rung, restoring in reverse, plus **user presets** (Low/Medium/
      High/Custom) that set renderer and streaming config only — `VIEW_RADIUS`, LOD radii (never past
      the N2.4 budget), fog, upload caps, `MEM_CAP`. Defaults: Low 8/no LOD2, Medium 12/LOD1 only,
      High 16/LOD2 48. **Never reduced by any preset or rung:** anything that decides a block, light
      correctness (BFS, AO), or scatter/cross-quad meshing. Block hashes are identical at every
      preset and rung — a world only looks farther or nearer. Presets live in settings, never in the
      description, and never invalidate saved data.
- [x] P1.11 What is the startup budget before the player can move?
      **Movement unlocks when the player's 3×3 is resolved, lit, meshed, and uploaded, and the
      8 neighbours of that 3×3 are generated** (the 5×5 generation footprint, 25 columns).
      Reference target **≤3 s** from world-open, asserted by a benchmark on cold and warm caches.
      A loading cover shows progress; it fades over ~0.5 s after unlock. **Spawn is chosen on the
      CPU** from the `HeightTile`: land with `top_solid_y > SEA+1`, not a river/lake bed,
      `slope_drop <= 1`, searched in deterministic spiral order within `SPAWN_SEARCH_RADIUS = 256`;
      no valid land is `DescriptionInvalid`, never a silent water spawn. Saved worlds use the saved
      position (clamped inside the edge). Startup work is class 0 with boosted caps (8 columns /
      16 MiB). `STARTUP_TIMEOUT = 20 s` shows the stuck stage and coord with Retry / Save and Quit.
      Input, gravity, and entity simulation stay paused until unlock.
- [x] P1.12 What is measured before a stage is considered done?
      Pinned counters (Q1.1), a profile capture, and a golden hash.

---

## Q. Observability and tooling

- [x] Q1.1 Exact metric list per stage?
      Pinned set: per-stage latency p50/p95, queue depth, in-flight jobs,
      resident/ready/queued columns, meshes and quads, upload bytes, light repairs, edit latency,
      LOD counts. Names and units fixed in the spec.
- [x] Q1.2 Is there a per-chunk trace view, and how is it triggered?
      Per-edit timeline (input → relight → mesh → upload → shown), logged per edit.
- [x] Q1.3 Capture file contents?
      **Binary, versioned header + CRC:** description hash, seed, platform tag, camera path,
      edit log, settings, stage timings.
- [x] Q1.4 Replay determinism: same visible result, or same block hashes?
      Regenerate from the description, re-apply the log, compare per-section hashes.
- [x] Q1.5 Debug views: field probe, height, biome, strata, LOD, light, overlay split?
      **One overlay, one pinned index** (`dbg <name>` or a number key): 1 field probe (any graph
      node, crosshair value + false-colour lattice), 2 height (macro / post-detail / `HeightTile` /
      diff), 3 biome best/second/margin, 4 fired surface-rule index, 5 strata band and fault offset,
      6 span count and kinds, 7 sky/block light and AO toggle, 8 LOD level and coarse light, 9 water
      quads/thickness/levels/outlet, 0 revisions, queue depths, dropped/discarded/applied,
      overdue/failed; F-keys for bounds, per-class memory, timelines, culling counters.
      Reads CPU-side tiles only, never recomputes on GPU; approximate values are labelled; zero
      cost when off. Corner readout: world coords from `i32`, version, description hash, seed,
      platform tag. A "copy report" key writes the Q1.9 bundle for the crosshair column.
- [x] Q1.6 Is there a terrain authoring tool in-engine, and when?
      **No in-engine editor in v1.** Graph, curves, biomes, rules, and templates are files loaded
      as a new description version; dev builds may hot-reload into a scratch world, never into one
      with saved sections. `describe-diff` and `terrain-gen` dumps are the feedback loop. A tuning
      UI is a later milestone and would write files, not live state.
- [x] Q1.8 How does a player report a bad world with enough data to reproduce?
      **One zip, `<1 MB` target / 5 MB cap, no terrain.** `manifest.json` (engine, generator,
      description hash, seed, platform tags, world id, save format, trigger), `description.toml`,
      `pack.lock` plus pinned record hashes (custom packs only if opted in), `header.json` (name
      hashed by default), `failure.json` (typed error, attempts, device), `stages.json` (per-stage
      hashes for the failing column and its 8 neighbours, regenerated from the description; saved
      `baseline_hash` drift if any), `counters.json`, `logs.txt` (~2,000 lines, scrubbed),
      `system.json`. Scrubbing removes paths, user/machine names, player ids, IPs, with a grep test.
      Replay regenerates and compares stage hashes, warns on a different platform tag, and refuses a
      removed generator version.
- [x] Q1.7 Are counters exported to Tracy, a HUD, or both?
      **Both**, when enabled.
- [x] Q1.9 Is there a headless generator binary for CI?
      **`terrain-gen`**, depending only on generation/hashing/section/save-format crates (no
      window, GPU, audio, or renderer; enforced by a CI crate-graph check). Subcommands: `gen`,
      `golden check|update`, `seams`, `sweep`, `verify`, `replay --report`, `describe-diff`,
      `world-migrate --dry-run`, and tile/height/biome/hydrology dumps to PNG/raw. Options
      `--threads N` (default 1) and `--order forward|reverse|shuffled`; output must be identical
      for any value. Exit codes `0/1/2/3`; `--json` for CI. The game's error banner can write a
      report bundle (description, header, coords, logs — no terrain) for players to attach.
- [x] Q1.10 Is there a map screenshot / top-down exporter?
      **`terrain-gen map`** — headless, reads generated data only (never saved sections), modes
      `height|biome|surface|rivers|climate|strata|caves|slope|lod`, `--scale`, `--out`, `--json`,
      `--ndjson`. Deterministic colour tables checked in; PNG carries `tEXt` metadata (description
      hash, version, seed, platform, region, mode) and a legend, nearest-neighbour only; golden image
      hash per mode. JSON has fixed schemas, row-major `x` fastest, ints as ints and floats as
      decimal plus `f32` bits; region capped (4,096² default), memory bounded by streaming tiles.
      Used by `calibrate-blend`, sweep checks, R1.13 acceptance, and bug reports. `--threads`
      defaults to 1; output identical for any count/order.
- [x] Q1.11 Are warnings rate-limited so a bad chunk does not spam the log?
      **Two limiters plus an exemption.** Error and above are never suppressed by id (panic,
      stall, corrupt section, device loss, invariant), but are capped at 50/s with a summary.
      Warn and below: key `(message_id, stage, coord_bucket)` where the bucket is the 128-block
      field tile, **once per 60 s**, then `(+N suppressed)` at the next emission; first hit after a
      quiet window always emits. Global token bucket 200 lines/s (burst 400), one summary line per
      second with top ids. **Suppression never drops counters** — hits/emitted/suppressed/last_seen
      are in overlay mode 0, report bundles, and CI JSON, so tests still assert exact counts. Log
      file rotates 10 MB × 5 on the IO thread; a full channel drops Warn-and-below and never blocks
      the main thread.

---

## R. Testing and acceptance

- [x] R1.1 Which stages get unit tests with fixture fields?
      **Tier 1, unit, every commit, <~60 s total, no GPU:** graph compiler rejections
      (cycles, unknown nodes, chained warps, range, banned `erosion` id, biome→height edge),
      description parse/emit/hash, curves, lattice/`macro_cell`/`floor_div` at negatives and
      extremes, hydrology on hand-made bowls/plains/pits, D8 ties, lake gates and spill, U-profile
      and bed clamps, surface-rule precedence slots, `stone_at` agreement, feature spacing order
      independence, downsample ties, section pack/unpack at every bit width, edit-log batch undo.
      A pure stage without a tier-1 test fails a coverage lint.
- [x] R1.2 What invariants hold for every chunk?
      **Tier 2 property tests** (fixed CI seeds + nightly random, failing inputs committed):
      fields stay in `[0,1]` with a zero clamp counter; curves never overshoot; index/inverse
      round-trip; padded copy equals `get_block`; shuffled order and 1 vs N workers give identical
      hashes; every non-sea cell has a receiver and no cycles after fill; incremental light equals
      full recompute (1M edits nightly); greedy meshing covers exactly the visible faces; noise
      stays within `NOISE_BOUND`. Tier 3 (golden/seam/sweep) covers cross-stage behaviour only,
      and a tier-3 failure must first land as a tier-1/2 regression test.
- [x] R1.3 Golden hash coverage: how many coords, which biomes, which LODs?
      **~40 branch-forcing fixtures** found by a search tool over a fixed seed set: ocean floor,
      shelf, beach (warm/cold), plains, hills, peak near `MAX_TERRAIN_Y`, terrace wall, river
      (narrow/wide/mouth), open and closed lake, cave mouth, cheese, spaghetti, overhang, tree on
      slope, ore vein across a region border, biome dither border, snow line, deep-rock
      transition, tile-edge basin, LOD1/LOD2 coast and mountain. Hashes over **decoded block
      ids**, per section and per column, plus per-stage hashes (field tile, height, biome arrays,
      hydrology tile, candidates) so a diff names the first changed stage. Stored per platform
      tag; also run with 1 vs N workers and shuffled order. Regeneration is tool-only with a diff
      report. Derived light/meshes are covered by equality tests, not goldens.
- [x] R1.5 Seam tests: every pair of adjacent tiles, both directions?
      **Three layers, all bit equality.** (1) Exhaustive 5×5 window of 128-block field tiles and
      of columns; every adjacent pair, both axes, both directions, each side generated cold in its
      own worker: border lattice samples, hydrology/basin outputs (levels, outlet ids, edge
      accumulation), biome arrays, candidate lists, shared-section blocks, and the 1-block margins
      for slope/AO/light. (2) Random far pairs to the world edge, including negative coords and the
      centre. (3) Order and parallelism: forward/reverse/shuffled, 1 vs N workers, same region
      built from two tile origins. Failures report pair, axis, stage, first differing cell, both
      values, and emit a promotable fixture.
- [x] R1.4 Cross-process determinism test?
      Covered by goldens per platform tag plus the 1-vs-N-worker and order runs.
- [x] R1.6 Migration tests: old world, edited, unmappable edit?
      **Checked-in fixture worlds per shipped save version** (v1/v2/v3), each as unedited, edited
      (place/break, batch, undo), and unmappable (removed block in the log, unknown block in a
      section). Asserts: migrated output equals a **golden decoded-block hash**; report JSON equals
      golden modulo timing/paths; Fork and Pin leave the original directory hash unchanged;
      kill-and-resume at random points (including mid-rename) yields identical final bytes; dry-run
      equals the real report; restore-from-backup reproduces the original; mixed-format refuses
      normal open. Small fixtures, every PR. A new save version must ship its fixtures.
- [x] R1.7 Property tests for noise bounds and curve monotonicity?
      **Answered in R1.2.**
- [x] R1.8 Performance regression thresholds and where they run?
      **Two tiers.** *PR (shared runners, <2 min):* deterministic counters only — allocation or
      instruction counts for tile/column/light/mesh, plus quads, vertices, bytes per section,
      resident bytes per class, tap counts, LOD node count. **Fail at >5%** vs a checked-in baseline.
      *Nightly (one pinned machine = the R1.13 reference):* wall-clock for tile, hydrology, column
      per biome class, light, mesh, edit p95/p99, steady frame p99, ring fill, cold/warm startup,
      and a 30-minute soak. 5 runs, median; **fail at >15%**, and report >10% run-to-run spread as an
      unstable machine rather than a pass. A second gate fails at >25% cumulative drift vs the
      30-day baseline, so slow ratcheting cannot hide. Baselines are tool-updated only, with a
      reason, per platform tag. Failures open an issue with both values and auto-bisect.
      Absolute budgets from R1.13 (edit p95 ≤ 50 ms, frame p99 ≤ 12 ms) fail regardless.
- [x] R1.9 Visual acceptance: rivers reach sea, lakes in basins, 3+ biomes with shadow,
      strata on cliffs — is that the final list?
- [x] R1.10 Are golden hashes regenerated deliberately, with a diff report?
      **Yes** — tool-only, diff report required, valid only for a version bump.
- [x] R1.11 Fuzz test for save/cache corruption?
      **Yes.** Targets header, sections, journal, description, `migration.journal`, `pack.lock`;
      mutations include bit/byte flips, truncation at every length class, zero-fill, duplicated
      blocks, swapped fields, oversized length prefixes, invalid UTF-8. Asserts the **exact
      `ErrorKind` and `Disposition`**, never message text, plus a **byte-identical directory** after
      the attempt. Table-driven expectations (section crc → refuse and report all; torn journal tail
      → silent truncation; unknown name → placeholder round-trip; header crc →
      `HeaderChecksumMismatch`; edited description → hash mismatch; missing → `DescriptionMissing`
      plus viewer). No panic, hang, or unbounded allocation; length prefixes are bounded before
      allocating. PR runs ~200 fixed mutations (<20 s); nightly runs coverage-guided fuzzing with a
      persistent corpus, minimising and committing every failure.
- [x] R1.12 Is there a "seed sweep" that generates N random seeds and checks invariants?
      **Two tiers.** PR: 16 fixed seeds × 2 regions (coastal, mountainous), <3 min. Nightly:
      64 rotating seeds (16 fixed + 48 date-keyed so failures reproduce) × 3 regions, plus
      negative coords, world edges, and polar `z`. Invariants: nothing above `MAX_TERRAIN_Y`;
      `y=0` bedrock everywhere; water only at `y<=SEA`, in a lake at its level, or in a river
      channel at its stepped level; every river reaches sea or lake; no cave breach under
      `SEA+1`, rivers, lakes, or banks; one surface rule per column with no topsoil under water;
      all fields in `[0,1]` with a zero clamp-fire counter; no leaf beyond `LEAF_MAX_DIST`; no
      tree writing water or bedrock; seam/order/worker equality on one window; hashes stable
      across two runs. Failures print seed/region/check/coordinate and emit a fixture. Runtime
      >1.5× target fails as a perf regression.
- [x] R1.13 What is the definition of done for the first terrain milestone?
      **Every clause passes in CI or a scripted run** at `VIEW_RADIUS = 16` on a fixed seed set:
      *Content (scripted, asserted by queries):* every river reaches sea or lake; lakes at outlet
      level; ≥3 biomes per seed with measurable lee/windward humidity contrast; strata on cliff and
      terrace faces; cave mouths, overhangs, trees, scatter, and ores present and respecting write
      masks; finite edge with deep-sea band, wall, fogged horizon; edge-to-edge walk on both axes
      and a diagonal with no void and no unloaded-ground fall.
      *Correctness:* goldens, 5×5 + random seams, and 16/64-seed sweeps green per platform tag;
      identical output across worker counts and orders; edits survive restart and a kill test
      loses at most the journal window; corrupt section refuses with its path; unknown id
      round-trips byte-identical; panic and hang injection pass; replay reproduces hashes.
      *Performance:* edit p95 ≤ 50 ms / p99 ≤ 100 ms; steady walking no frame > 20 ms (p99 ≤ 12 ms);
      post-teleport 3×3 within 1 s, ring within a stated bound, no frame > 33 ms; memory ≤ cap
      without reaching radius shrink; 30-min soak plateaus with no lost dirty data.
- [x] R1.14 What would make us reject the whole approach?
      Any of: (1) determinism cannot be held on one platform tag after a bounded investigation;
      (2) edit p95 stays > 100 ms after P1.7 optimisations; (3) 20 ms cannot be met at radius 8;
      (4) hydrology seams keep failing after halo and basin-size fixes; (5) worst-biome memory
      exceeds 2× the K1.1 estimate; (6) the ring cannot fill at walking speed with the pool busy.
      Fallbacks: (1) tighten banned ops or move to fixed point; (2) smaller sections or narrower
      remesh scope; (3,5) cut radius or go to vertical bands; (4) coarser independent hydrology
      pass; (6) cut cave/detail cost.

---

## S. Failure and robustness

- [x] S1.1 Typed error enum per stage, and what carries context?
      **One `TerrainError { stage: Stage, kind: ErrorKind, coord: Option<Coord>,
      context: Vec<Frame> }`** crossing every boundary. `Stage` and `ErrorKind` are closed enums
      (`ErrorKind` `#[non_exhaustive]`) with typed payloads, never strings as discriminants:
      `CorruptSection{path,kind}`, `DescriptionHashMismatch{expected,found}`,
      `UnknownGeneratorVersion`, `NewerThanEngine`, `JournalTornTail` (recoverable),
      `IoError{op,path,os_error}`, `UnknownBlock{name,section}` (degrades, never refuses),
      `DescriptionInvalid{key,rule}`, `GraphInvalid{node,rule}`, `JobPanicked`,
      `JobOverdue`, `JobFailed`, `ResourceCap{class,bytes}`, `InvariantViolated{check,coord}`.
      Every kind declares a fixed `Severity` (Fatal / SessionFailed / Recoverable / Degraded) and
      `Disposition` (RefuseLoad / FailJob / DegradeAndContinue / RetryOnce / Report) in one
      wildcard-free table; callers branch on those, never on message text. Frames append as the
      error crosses boundaries. `thiserror` impls, no `anyhow` in library crates (allowed only in
      binary `main()`). Panics convert to `JobPanicked` at the job boundary. Reports serialise
      with a stable serde tag; tests assert severity/disposition for every variant and a golden
      JSON per variant.
- [x] S1.2 Worker panic: contain, retry once, or fail the chunk?
      **Contained at the job boundary** (`panic = "unwind"`). Log coord/stage/version/hash/seed/
      platform + backtrace + a repro record, free the reservation, reset scratch, keep the worker.
      Class 1/2 retry once at the same priority; a second panic marks `(coord, stage)` **Failed**
      for the session — never meshed, applied, or saved, so neighbours never treat it as resolved.
      Class 0 escalates immediately to a blocking error with Retry / Skip / Save and Quit. Saved
      data is never touched: a panic in a save job aborts before the rename. Compute-once cells
      are reset so waiters recompute; a pure-function panic is recorded as `Failed(key)`.
      `panics_caught > PANIC_LIMIT` (20 in 60 s) pauses new generation with a banner.
- [x] S1.3 A stage that never completes: timeout, and what the player sees?
      **Soft deadline `max(250 ms, 10 × p95)`** per stage, tracked by a watchdog reading each
      worker's `(job key, start time)`. On expiry: log + backtrace, mark `Overdue`, bump the
      revision so the eventual result is stale, re-queue once on another worker. Second expiry →
      `Failed`, same handling as S1.2. Class 0 escalates immediately. The stuck worker is **not**
      killed; if still running after `HARD_STUCK = 60 s` it is counted lost, a replacement worker
      is spawned, and the lost thread is leaked with its memory still accounted. `> STUCK_LIMIT`
      (2) pauses generation and offers Save and Quit. All generation loops have fixed upper
      bounds; compute-once waits time out after 5 s; no job holds a lock across another job.
      Failed is session-scoped, never persisted.
- [x] S1.4 Corrupt baseline cache: detect via hash, delete, regenerate?
- [x] S1.5 Corrupt overlay: refuse to load, or load and report?
      **Hard load error.** Each section carries crc32/xxh3-64 over its payload, checked on read.
      On mismatch, bad framing, unknown palette name, or impossible bit width: abort the load, name
      world/section/path/kind, exit to the world list. Nothing substituted, regenerated, or
      written; the file is untouched. Recovery is an explicit opt-in `world-repair` CLI (restore
      backup, quarantine to `.corrupt` and regenerate as never-saved with an edit-count report, or
      salvage). One rolling backup generation per section. Journal tail corruption is distinct: a
      torn tail is truncated on recovery.
- [x] S1.6 GPU device loss: recover, or fatal with a clear message?
      **Fatal for the session.** One handler for device-lost callbacks and submit/present errors,
      with no GPU calls after the first signal: 1 stop rendering and freeze the sim (no new edits);
      2 flush and fsync the journal, let an in-flight section save finish its rename; 3 write a
      report bundle with `DeviceLost{backend,adapter,driver,reason}` and last save time; 4 show a
      blocking error and exit to the world list, or stderr + exit 3 if the surface is gone. Nothing
      GPU-side is persisted. **No device recreation, no re-upload, no software fallback in v1.**
      Test: inject device loss at random frames during edits and loading; assert journal flushed,
      edits intact after reload, report written, clean exit. `terrain-gen` is unaffected.
- [x] S1.7 Out-of-memory policy: evict cache, lower radius, or crash?
      **Hard `MEM_CAP_BYTES`** (default `min(60% RAM, 4 GiB)`), tracked by central allocator
      accounting with admission-time size estimates so workers cannot overshoot together. Ladder,
      stopping at ≤90%: LOD2 → LOD1 → derived caches → `R+1..R+2` and LRU columns (dirty saved
      first) → drop `VIEW_RADIUS` by 2 (min 8, log once, notice). Restore +2 only after ≤70% for
      60 s. Never evicted: player 3×3, pending edits, in-flight jobs. Exhausted at min radius:
      pause new generation/meshing, banner the largest consumers, class 0 edits continue.
      Per-class byte counters each second. CI soak asserts peak ≤ cap.
- [x] S1.8 Non-finite field output: clamp, log, or assert?
      **Debug/CI assert; release maps NaN → 0.5 (neutral) and clamps inf**, increments
      `nonfinite_clamped`, logs once per node per 60 s, and marks the tile **tainted** (never cached,
      flagged in the report bundle). Height clamps non-finite to `SEA` the same way. Saved sections
      hold block ids, so they are unaffected. CI sweeps and goldens assert the counter is exactly 0.
- [x] S1.9 Bad content pack: refuse to open the world, or substitute placeholder?
      **Refuse**, before any section is read: schema, stable-name grammar, duplicate ids, required
      records (a catch-all per biome, every referenced id resolves), property-hash baseline, stream
      vocabulary. `ManifestIncompatible{id,reason}` / `DescriptionInvalid{key,rule}` names the first
      offender and lists all failures. Nothing written. Distinct from T1.6, where a *valid* manifest
      missing a saved name degrades to a placeholder. `--lenient` exists only on `terrain-gen` for
      authoring and can never write to a world with saved sections.
- [x] S1.10 Partial world load: allowed or forbidden?
      **Forbidden.** Open is all-or-nothing across header checksum, description hash, generator
      version support, manifest validation, and journal recovery. Any failure leaves the directory
      byte-identical. Runtime `Failed` columns are session-scoped and never saved, so they are not a
      partial load.

---

## T. Versioning and migration

- [x] T1.1 Exact rule for when `GeneratorVersion` bumps.
      Any generation-code change that alters output bytes; any new node type, predicate, or
      feature shape. Performance-only changes never bump.
- [x] T1.2 Is the version an integer, a semver, or a content hash?
      **`(generator_version: u16, description_hash: [u8; 16])`** (A3.5). Monotonic `u16`, never
      reused; xxh3-128 over canonical graph, content pack, world parameters, and stable-name set.
      The **seed is not in the hash**, so one description serves many worlds. A checked-in versions
      table pairs them, and CI asserts the pair for every shipped version. Loader recomputes the
      hash and refuses a mismatch.
- [x] T1.3 How many old generator versions does the engine keep evaluable?
      **Current + previous `K = 2`**, each a frozen module behind
      `Generator { version, tile(..), column(..) }`. "Frozen" means bug fixes only if output bytes
      are provably identical. Every shipped version keeps its goldens and seam suite in CI.
      Shared infrastructure is common and usable only if its output is covered by a golden for
      every version that uses it.
- [x] T1.4 Does a new engine release ever auto-upgrade a world?
      **Never.** A pruned version offers Migrate / Fork / Cancel. A world newer than the engine
      refuses to open (no downgrade). Migration never rewrites saved sections; the frontier seam
      is not smoothed in v1.
- [x] T1.5 Content pack versioning: additive only, or breaking allowed with migration?
      **Additive only.** Ids are stable names; properties are frozen per id (a change is a new
      id); removal is deprecation (`deprecated = true`, still loads); renaming is forbidden;
      aliases are opt-in, load-time only, never on disk; records are addressable by
      `(name, record_version)` and hashed into the description. CI diffs the pack against the last
      release and fails on remove/rename/property change.
- [x] T1.6 How is an unknown block stored and rendered after content removal?
      **Placeholder, data untouched.** Reserved `UNKNOWN_BASE..` runtime ids, allocated per
      distinct name and kept in a per-world unknown table; saved sections keep the original name,
      so round trips are lossless. Placeholder: opaque, collidable, opacity 15, no emission, high
      hardness, no drops, magenta/black checker. Never host rock, never replaceable, can't be
      placed, pick-block gives nothing. Load logs the distinct names once; world info lists them.
- [x] T1.7 Is there a world "fork" UI, and what does it copy?
      **Explicit Pin / Migrate / Fork / Cancel dialog**, only when the world cannot open as-is, no
      default button. Migrate takes a verified full backup first, rewrites only formats and never
      regenerates sections, is resumable and atomic per section, and records history. Fork copies
      to a new identity and migrates the copy, leaving the original byte-untouched.
- [x] T1.8 Migration report format and destination?
      **`<world>/migration-report-<date>.json`**: sections read/rewritten/unchanged; unknown and
      deprecated names with counts and aliases applied; edit-log entries per section and
      unmappable/conflicting counts with examples; sections whose `baseline_hash` fails; platform
      and pruned-version warnings; elapsed time and bytes; backup path. CLI parity:
      `world-migrate --dry-run` prints the same report.
- [x] T1.9 Are parameter changes world-level or per-region overrides?
      **World-level only.** One description, one value per key, applied uniformly. No zones, no
      per-region or per-chunk overrides in the schema (lint-enforced). Regional variety comes from
      fields in the graph. Per-region overrides would be a later version bump with their own hashing,
      seam, and migration story.
- [x] T1.10 Does the engine support two generator versions in one world?
      **No** — one version per world; migration changes it wholesale.
- [x] T1.11 How is a bug in an old generator fixed without changing its output?
      Covered by "frozen": fix only if bytes are identical, else new version.
- [x] T1.12 What is the deprecation policy for old generator versions?
      **Current + 2 previous frozen evaluators; a version is Deprecated when its successor's
      successor ships, removed the release after that, never before `MIN_SUPPORT_DAYS = 180`.**
      So every version gets ≥2 releases and 6 months of support. States in the versions table:
      Current / Supported / Deprecated / Removed, with dates. Deprecated worlds open normally with
      one notice per engine version ("supported until release X"). Removed versions show the T1.7
      dialog with Pin disabled. **Save-format readers live longer: ≥4 releases and 12 months**, so
      saved terrain is always readable even when its generator is gone. Removal deletes the
      evaluator, its goldens, and its seam fixtures together, keeps the versions row forever, and is
      CI-checked against the 180-day floor.

---

## Answer log

| Round | Domain | Decision | Consequence |
|---|---|---|---|
| 1 | A0 | World description precedes the first generated chunk | Every world is pinned from birth; no generate-first retrofit |
| 1 | A1 | `u64` seed; streams derived by hashing `(seed, name, coord)` | No 128-bit migration; seed file stays compatible |
| 1 | C3 | `top_solid_y(h) = floor(h)`; height stays `f32`; v0.0.7 `round()` dropped | One quantisation point in the pipeline; surface/water/caves/features share it |
| 1 | C4 | World is 384 blocks tall | Vertical residency, sky-light columns, and section layout all change |
| 2 | A2 | Same machine, same binary; no cross-platform promise | `f32` stays; no fixed-point rewrite; golden hashes are per platform |
| 2 | C1 | Sea level 128 | 128 blocks of sea, ~256 of land and air; height clamp still open |
| 2 | C4 | Flat bedrock at y=0 | No noisy floor field; one invariant to protect |
| 2 | I1 | 32³ sections, 12 per column | Chunk type, mesher, saves, and LOD units unchanged |
| 2 | K1 | Full column residency, all 12 sections | Readiness is per column; 12× memory and load cost per column |
| 3 | B1 | OpenSimplex2 only; `[0,1]` via one `unit()` helper | One algorithm to keep stable; all curves and thresholds live on `0..1` |
| 3 | B3 | Data graph, compiled per world, closed node enum | Composition changes are data edits; new node types need a version bump |
| 3 | B4 | 4-block macro lattice, bilinear; block-centre samples; nodes declare lattice | One grid for shape; interpolation and cave alignment share the same convention |
| 4 | A2 | Per-machine determinism wins; platform tag recorded on the world | No fixed-point rewrite; CI compares per-platform goldens |
| 4 | B1 | fBm divides by exact amplitude sum; per-octave bound is measured, not assumed | Range guarantee needs a measured constant or per-octave clamp |
| 4 | B3 | Constants stored as `f32` bits; canonical hash over bits only | Formatting can never fork a world; comments are lint-checked |
| 4 | B4 | 128-block tile + 1 sample border; global lattice, never chunk-anchored | Tiles are pure, evictable, and border samples agree bit-for-bit |
| 5 | B1 | `NOISE_BOUND` measured with a 2% margin; octaves scaled into `[-1,1]` | Range proof is real; noise impl changes require re-measure + version bump |
| 5 | B2 | One 2D warp node (2 taps), halo = `ceil(amp/MACRO_CELL)+1`; monotone cubic curves | Warp cost is per lattice sample; curves cannot overshoot |
| 5 | B5 | CPU-only for block-deciding work | GPU stays a display path; world identity has no GPU term |
| 5 | C1 | `height = SEA + base(cont) + relief(erosion) * shape(peaks) * mask(cont)` | Coastline is a level set of continentalness; mountains retune independently |
| 6 | C1 | Ocean floor 32 deep, 6-block beach, knots pinned on `c` | Shallow seas by design; beach width measured, not assumed |
| 6 | C1 | `MAX_TERRAIN_Y = 352`, proven by the compiler, 32 blocks headroom | No clamp artefacts; tall-graph rejection is a compile error |
| 6 | C2 | Field renamed `roughness`; `erosion` means the solver only | Naming collision removed; CI lints against regression |
| 6 | C2 | Ridged + shape curve; terrace on height, mask-gated, anchored to `SEA` | Peaks and mesas are authored data; vertical risers possible |
| 7 | K1 | Sparse sections in a 12-slot column; dense array, uniform slots | Readiness stays per column; ocean air costs no allocation |
| 7 | D5/D4 | No erosion solver in v1; `heightfield_post` no-op reserved | Rivers and carving stay independent of a future solver |
| 7 | D7 | Water is blocks; fill `top+1..=SEA`; uniform water sections | One water model; no derived/implied water anywhere |
| 7 | D2 | D8 macro routing, priority-flood, `u32` accumulation, 512 tiles + halo inflow | Tile-order independence; flow ignores per-block noise |
| 7 | D3 | River at 300 cells, `sqrt` width 1–10, headwater fade | Rivers taper in; width is a smooth ribbon, not 4-block steps |
| 8 | D2 | Flood with `EPS = 1/64`, exact in `f32`; lakes are `filled > h + depth` | Flat lakes drain deterministically; lake level comes from the outlet only |
| 8 | D3 | U-profile `clamp(w/2,2,6)`, sand/gravel bed, carve-only meander ≤8 blocks | Rivers read as valleys; routing stays pure D8 |
| 8 | D7 | Per-basin lake levels, closed basins allowed, stepped river water | No global water table; rivers never rise along flow |
| 9 | E1 | `temp = lat(z) + noise - lapse(h_macro)`, `LAPSE = 1/320` | Climate is lattice-only; snow line follows altitude and latitude |
| 9 | E2 | Fixed `+x` wind march, 32 cells, curve-shaped shadow + coast term | Rain shadow reuses hydrology height; wind direction is world data |
| 9 | E3 | Nearest point in climate space; blend + keyed dither over ~24 blocks | Adding a biome is content-only; terrain stays smooth, materials dither |
| 10 | E3 | Tile stores best/second/margin `u8`; `w` reconstructed from quantised margin | Biome blending is deterministic and costs 3 bytes/cell |
| 10 | E4 | Ordered predicates, one evaluator; integer slope drop; fixed 9-slot precedence | Surface rules are data; "grass under the ocean" becomes impossible |
| 10 | E3 | Biomes never change height in v1 | Cycle height→climate→biome→height is rejected by the compiler |
| 11 | F1 | 4×4×4 lattice, trilinear; global, `4k+0.5`; skip above roof / below y=1 | ~6.7M 3D samples per ring instead of 428M |
| 11 | F2 | Spaghetti + cheese, 4 taps, `CAVE_TAP_BUDGET = 4` | Two readable cave looks; budget is a compile-time gate |
| 11 | F2 | Entrance mask lowers the roof smoothly; hard sea/bank exclusions | Real cave mouths without swiss-cheese surfaces |
| 11 | F3 | Detail band `[-24,+8]`, `FLOAT_MIN` floor, no connectivity pass | Overhangs only where visible; slivers rejected |
| 11 | F4 | Materials resolve per solid span, column stack as fast path | Overhang undersides and cave floors get correct rules |
| 12 | G1 | Warped `y` bands + 2D fault offsets; `THICKNESS = 6`, `STRATA_WARP_AMP = 8` | Clean layers on cliffs at ~1 add + 1 `floor_div` per solid block |
| 12 | G2 | Folds/faults deform materials only; compiler rejects edges into height | Geology can land without moving water or coastlines |
| 12 | G1 | `DEEP_Y = 64`, deep rock with 4-block ragged transition | Deep volume skips strata entirely; clear "deep" identity |
| 12 | G3 | Ores are two-pass features in 32³ regions, host-rock sets, lowest-id wins | Veins are inspectable, cacheable, chunk-order independent |
| 12 | G1 | `stone_at(x,y,z)` is the single stone resolver | Cave, cliff, and subsoil stone agree by construction |
| 13 | H1 | 32³ regions, `MAX_EXTENT ≤ 32`, 3×3×3 halo, cached candidates | One halo rule for ores, trees, and structures |
| 13 | H2 | Fixed write order with per-type masks; nothing writes water or bedrock | Feature order is a spec constant, not a world setting |
| 13 | H3 | Data templates, 4 primitives, `LEAF_MAX_DIST = 4` | No floating leaves; new species is a content edit |
| 13 | H3 | Jittered grid, priority rejection, `TREE_CELL = 6` | Deterministic spacing independent of chunk order |
| 13 | H4 | Scatter is real blocks with `render_shape`; no collision, no occlusion | Mesher, saves, and pick-block stay uniform |
| 14 | I1 | `Uniform | Paletted{bits 1–16, packed u64}`; `index = x + 32*(z + 32y)` | Palette memory scales with section variety; one decode path |
| 14 | J4 | Saved effective sections are the source of truth; never regenerate | Removes the baseline cache model; conflicts with the pipeline doc (J4.5) |
| 14 | J1 | Per-block records with old value, batch revision, stable names | Undo and honest migration reports become possible |
| 14 | J3 | Group-commit journal: 250 ms / 64 batches, fsync on save and every 10 s | Disk never blocks the game thread; bounded loss |
| 15 | J4 | Confirmed: effective sections are truth; edit log is audit only; no baseline cache | Pipeline doc updated to match; migration transforms stored data |
| 15 | K1 | Square ring `R = 16`, mesh ring `R-1`, unload at `R+2`, LRU grace | 1,089 visible / ≤1,369 resident columns; no thrash |
| 15 | K2 | Urgent > frustum > ring > prefetch > background; distance within class | Camera direction reorders work but never changes membership |
| 15 | K4 | 2 columns / 4 MiB per frame, 3 ms cap, 12 columns ready ahead | Fill rate is bounded and measured, not spiky |
| 16 | L1 | 15-level BFS per column, whole column, ceiling-seeded, 4-bit nibbles | Uniform air sections light for free; no sky summary needed |
| 16 | L2 | Water attenuates skylight only (`WATER_SKY_ATTEN = 1`); block light passes | Deep oceans go dark; underwater lamps still work |
| 16 | L4 | Per-vertex 2-bit AO in the greedy merge key | Good shape readability at the cost of extra vertices near geometry |
| 16 | L3 | Incremental relight must be bit-identical to full recompute | Fuzz + sampled self-heal keeps lighting honest |
| 17 | M3 | One mesh per emitting section, sub-allocated in a per-column arena | 2–5 meshes per column; culling stays per section |
| 17 | M3 | 8-byte packed quad record, vertex pulling, no index buffer | ~5× less vertex bandwidth than the 40-byte layout |
| 17 | M2 | Water emits only water/air faces; solids draw their own faces | 32-deep ocean = one quad per section, no double faces |
| 17 | N1 | Three LODs (1/2/4 blocks/cell), 32³ cells, radii 16/32/64 | LOD is derived and disposable; LOD2 covers the world |
| 17 | N3 | Most-frequent-solid downsample + `SKIRT_DEPTH = 2` skirts | No overlap/gap; seams cost two cell strips |
| 18 | P1 | Reference: 2020 mid-range desktop, 1080p/60, radius 16, no frame >20 ms | Concrete gate for every budget |
| 18 | P1 | Edit latency 50 ms p95 / 100 ms p99; reserved edit workers | Edits never queue behind streaming; old mesh persists until replaced |
| 18 | P1 | Streaming pool `N-2-EDIT`, edit workers 1–2, 1 reserved for class 0 | Long generation jobs cannot starve urgent work |
| 18 | Q | Pinned counter set to Tracy + HUD; binary capture with hashes | Reproducible reports; graphable measurements |
| 19 | R1 | ~40 search-found branch fixtures; hashes per section, column, and stage | A diff names the first stage that changed |
| 19 | R1 | Exhaustive 5×5 seam windows + far pairs + order/worker equality | Order dependence and border bugs are caught, not sampled |
| 19 | R1 | Seed sweep: 16 PR seeds, 64 nightly, invariants only | Rare geometries get checked without goldens |
| 19 | S1 | Corrupt section = hard load error; repair is an opt-in CLI | Nothing is silently regenerated or substituted |
| 19 | S1 | Hard memory cap with an eviction ladder ending in radius shrink | Degrades instead of crashing; dirty data never dropped |
| 20 | T1 | Version = `(u16, description_hash)`; seed excluded from the hash | One description serves many worlds; CI asserts the pair |
| 20 | T1 | Engine keeps current + 2 frozen evaluators, each with goldens | Old worlds expand consistently; pruned versions offer migrate/fork |
| 20 | T1 | Content pack is additive only; properties frozen per id | Nothing can be silently reinterpreted from saved data |
| 20 | T1 | Unknown names become placeholders, saved data untouched | Lossless round trips; missing content is visible, not fatal |
| 20 | T1 | Pin / Migrate / Fork dialog with JSON report and `--dry-run` | Migration is explicit, resumable, and auditable |
| 21 | E4 | Derived 130×130 `HeightTile`, one `floor(h)`, read by slope/trees/surface | Single per-block height source; slope precedes 3D detail |
| 21 | F1 | Fixed six-step column order (height → fill → detail → carve → spans → features) | No stage can seal or float another's output |
| 21 | G2 | No fault scarps in v1; scarps deferred behind a version bump | Faults read as layer offsets only |
| 21 | N3 | Most-frequent-solid downsample confirmed; pipeline doc updated | Distant terrain is not inflated by any-solid |
| 21 | A2 | Explicit banned/allowed float ops, enforced by clippy + CI grep | Op pinning is mechanical, not a convention |
| 21 | J1 | Every committed block change is logged, including air | Undo works for both place and break |
| 22 | O1 | Pass order pinned: opaque → cutout → depth copy → water → particles → overlay → fog → UI | Screenshot fixtures fix the ordering decisions |
| 22 | O1 | Frustum-only, per column then per section, < 0.5 ms at `R = 16` | The visible list is the single hook for future occlusion |
| 22 | O2 | Camera-relative rendering from day one, no rebase | No shader ever sees a world-scale float |
| 22 | O3 | One fog function to sky at `FOG_END`, derived from LOD radii | Passes agree; a radius cut moves fog with it |
| 22 | K1 | Finite square world, edge-ocean mask, invisible wall, sea quad | The horizon never shows a cut and no land is sliced |
| 23 | O3 | `LOD2_RADIUS = 48` (1,536), `FOG_END = 1,472`, LOD ring clipped to the world | No LOD node exists beyond the boundary |
| 23 | K3 | Epoch `(coord, stage, revision)`, run to completion, discard at apply | Bounded waste; stale results never applied |
| 23 | I2 | Per-worker `34³` padded scratch; 1-block border copied, never borrowed | Lock-free jobs; ~79 KB per worker |
| 23 | A1 | `description.toml` + structural hash; all parameters inline and required | Text edits cannot change identity; presets are authoring only |
| 24 | I3 | No effective-chunk type; `section_rev`/`border_rev` key all derived data | One store; invalidation is one comparison |
| 24 | I3 | Collision reads sections directly; `Unloaded` is treated as solid | No fall-through; edits visible the same tick |
| 24 | S1 | Panics contained, retried once, then `Failed` and never resolved | A bad column cannot poison neighbours or saves |
| 24 | S1 | Soft stage deadlines + watchdog; lost workers replaced, never killed | Stalls are reported and bounded, not silent |
| 24 | Q1 | `terrain-gen` CLI drives goldens, seams, sweeps, verify, replay | One implementation, reproducible per thread count |
| 25 | R1 | DoD is scripted: content queries, correctness suites, and perf numbers | No clause is a screenshot; rejection criteria are written down |
| 25 | R1 | Tier 1 unit + tier 2 property tests; tier 3 covers cross-stage only | Failures localise to a stage before they reach a golden |
| 25 | S1 | One `TerrainError` with `Severity`/`Disposition`; no string branching | Refuse/fail/degrade decisions are data, not message matching |
| 25 | Q1 | One indexed debug overlay; no in-engine editor in v1 | Diagnosis is in-game; authoring is files + `describe-diff` |
| 25 | K4 | Lag-triggered shedding: background → LOD2 → LOD1 → prefetch → fog | Players see fog close in, never a hitch or a hole |
| 26 | E3 | 4 climate biomes (plains, forest, desert, tundra) + ocean/beach overrides | "Mountains" is not a biome; alpine comes from snow/stone/strata |
| 26 | E4 | Sea floor banded by depth 1–4 / 5–14 / 15–24 / ≥25 with climate variants | Ocean floor reads as depth, not as one sand plane |
| 26 | D8 | 512-block owned tile, `HYDRO_BORDER = 16 cells`, 160×160 stored | Border is recomputed, never copied; compiler-sized halo |
| 26 | P1 | Unlock on 3×3 ready + 5×5 generated, ≤3 s; CPU spawn search | Loading cover instead of falling through terrain |
| 26 | S1 | Device loss = flush, report, exit; no recovery in v1 | The GPU is never needed to keep the world consistent |
| 27 | A3 | xxh3-128 for identity, low 64 bits for streams, splitmix64 for per-sample | Three roles, all pinned with golden vectors |
| 27 | A3 | Stream names resolve to frozen `u16` ids at compile time | No string hashing in hot paths; typos are compile errors |
| 27 | A4 | Explicit key types, never packed; derived `Hash` never feeds output | Deterministic iteration; no bit-layout bugs |
| 27 | B5 | One eager tile bundle (fields → height → hydro → climate → biome → HeightTile) | Amortised 1/16 per block; no partial tile states |
| 27 | L2 | Torch (14) and lava (15) only | Block light is exercised without a content pass |
| 28 | A3 | All hashes `[u8; 16]`; T1.2 corrected | One width everywhere; hex text is 32 chars |
| 28 | C4 | Build range `1..=383`, one `can_edit` gate | Rejected edits write no record and bump no revision |
| 28 | J2 | Total order `(revision, batch, seq)`; log never replayed | Ordering cannot change the world |
| 28 | K2 | Per-class caps (64/1200/1200/128/64) | Bounded memory; urgent work never blocked |
| 28 | N2 | `LOD_NODE_BUDGET = 3,000`; pop ≤1% pixel diff | Distant terrain is bounded and measured |
| 29 | D9 | Repair dropped from v1; `WATER_MARGIN = 2` prevents intersections | Invariant asserted by sweep and debug check instead |
| 29 | F4 | Cave walls `stone_at`; gravel patches by mouth distance + depth | No topsoil or snow underground; deep rock never gravel |
| 29 | E2 | `(n * (1 - k*shadow)) + m`, one clamp | Exact order pinned by a golden sample |
| 29 | S1 | Non-finite → taint + clamp (debug assert); bad pack refuses; no partial load | Nothing broken is ever cached or written |
| 29 | R1 | PR counters >5%, nightly wall-clock >15% on a pinned machine | Noise-free PR gate; slow drift caught at 25% |
| 30 | K4 | Teleport: drop beyond `R+2`, keep valid inside, reset lag timer | Idempotent; no revisions bumped |
| 30 | P1 | Frame: 10 render / 3 apply / 1 other / 2.5 slack; nothing blocks | Boost to 8 ms apply only while 3×3 resolves |
| 30 | Q1 | Warnings limited per (id, stage, tile); counters never suppressed | A bad region logs once, tests still see counts |
| 30 | T1 | Deprecation: ≥2 releases + 180 days; save readers 4 releases/12 months | Saved terrain outlives its generator |
| 30 | D3 | Endorheic lakes allowed; basin cap prevents tile-dependent levels | "River ends in sea or lake" is the invariant |
| 31 | A1/A3/A4/A5 | Header fields, refuse-never-auto-create, stateless streams, Y-up metres, names-on-disk blocks | Foundations closed: identity, randomness, units, content |
| 31 | B1/B3/B4/B5 | Per-node params, no node cache, ≤128 nodes, declared 2D/3D classes, tap ceilings | The graph is compile-time bounded and cost-predictable |
| 31 | C1/C2 | Beach ≥3 blocks (flatten + slope cap), valleys from hydrology only | No dry valleys; coastlines always walkable |
| 31 | D7/D8 | No water table; hydrology tiles pure, ≤2 in flight, world mode in v1 | One whole-world pass replaces per-tile halos |
| 31 | E1/E3 | No seasonality; `BLEND_W` calibrated by transect (20–28 blocks median) | Width is measured, not asserted |
| 31 | F2/F4 | Cave depth weights (cheese/spaghetti), gravel patches | Caves are bounded by depth and sealed near water |
| 31 | H3 | Trees never touch water; no growth simulation | Skip-not-replace keeps canopies off water |
| 31 | J5 | Migration: verified backup, resumable journal, JSON report, atomic per section | Fork/Pin leave the original byte-identical |
| 31 | L1/L4 | Colour is shader-side; LOD light step accepted with a 2-column ease | Light stays two 4-bit channels |
| 31 | N4 | LOD1 ≤2 s, LOD2 ≤10 s, debounced, stale mesh keeps drawing | Background only; edits never wait on LOD |
| 31 | P1/Q1 | No fast mode (ladder + presets only); small report zip; `terrain-gen map` | Quality never changes generation output |
| 31 | R1/T1 | Migration fixtures, table-driven corruption fuzz, world-level params only | Schema cannot grow per-region overrides |
| 32 | A3 | Implementation pins SHA-256 (`[u8;32]`) + SplitMix64; spec updated | No new crates; interview A3.1/A3.5 superseded by the pin |
| 33 | J4.4 | Save format documented byte-for-byte (header, journal, section record, backups, migration) | Design closed; writer/IO/migration are the next milestone, and no byte may change |
