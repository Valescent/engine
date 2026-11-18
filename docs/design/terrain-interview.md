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
| A | Foundations and identity | 14 | 0 |
| B | Field graph and sampling | 16 | 0 |
| C | Macro landform | 15 | 0 |
| D | Erosion and hydrology | 20 | 0 |
| E | Climate and biomes | 15 | 0 |
| F | Volume synthesis | 16 | 0 |
| G | Geology and materials | 13 | 0 |
| H | Features and scatter | 14 | 0 |
| I | Chunk data model | 12 | 0 |
| J | Overlay and persistence | 15 | 0 |
| K | Streaming and residency | 15 | 0 |
| L | Lighting | 13 | 0 |
| M | Meshing | 14 | 0 |
| N | LOD hierarchy | 15 | 0 |
| O | Rendering and precision | 12 | 0 |
| P | Performance and budgets | 12 | 0 |
| Q | Observability and tooling | 11 | 0 |
| R | Testing and acceptance | 14 | 0 |
| S | Failure and robustness | 10 | 0 |
| T | Versioning and migration | 12 | 0 |

---

## A. Foundations and identity

### A0 Order
- [x] A0.1 Does the versioned description ship before the first generated chunk?
      **Yes.** No world exists without a generator version; no generate-first retrofit.

### A1 World description
- [ ] A1.1 Is the description one TOML/JSON file, a binary header, or both?
- [ ] A1.2 Which fields are mandatory at creation: seed, generator version, parameter set,
      content hash, format version, world name, created time, engine build?
- [ ] A1.3 Is the parameter set inline, a named preset plus overrides, or a hash only?
- [x] A1.4 Is `u64` the final seed width, or is a 128-bit seed needed to make worlds
      unguessable and collision-free?
      **u64 stays.** Stream seeds come from hashing `(seed, stream name, coordinate)`.
- [ ] A1.5 What does a missing or malformed description do: refuse to load, auto-create,
      or enter read-only mode?

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
- [ ] A2.3 Which operations are banned because they are not bit-stable: `sin`, `cos`, `pow`,
      `exp`, `sqrt`, FMA contraction, `f32::mul_add`, auto-vectorised reductions?
- [ ] A2.4 Is the simulation tick tied to generation? Does generation ever read time?

### A3 Keyed randomness
- [ ] A3.1 What hash function addresses a named stream? Is it fixed forever once shipped?
- [ ] A3.2 How is a stream name built: `stage/domain/chunk/feature`? Are coordinates folded
      as bytes, integers, or both?
- [ ] A3.3 Does every consumer get an independent stream, or do siblings share and offset?
- [ ] A3.4 Can a stream be exhausted? What happens at 2^64 draws?

### A4 Coordinates and units
- [ ] A4.1 Are negative coordinates floored or truncated when converting world to chunk?
- [ ] A4.2 Is the chunk key `(i32, i32, i32)` or a packed integer?
- [ ] A4.3 Is vertical order Y-up confirmed for every stage, including save and meshing?
- [ ] A4.4 Is one block one metre everywhere, including camera, physics, and noise
      frequencies?

### A5 Content model
- [ ] A5.1 Are block IDs stable integers with a registry, or packed names?
- [ ] A5.2 Where do block properties live: code, content file, or both? Which is authority?
- [ ] A5.3 Is the content hash over the file bytes, the parsed definitions, or both?
- [ ] A5.4 Can a world load with a subset of its content pack, and how is that reported?

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
- [ ] B1.5 What is the exact `raw` bound per fractal type? `fBm` of OpenSimplex2 is not
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
- [ ] B1.6 Are frequency, gain, lacunarity per field or per world, and are they stored as
      decimal text or `f32` bits in the pinned graph?
- [x] B1.3 Are octave count, lacunarity, gain, and frequency per field or per world?
      **Per node.** Each graph node carries its own and is hashed into the world identity.
- [ ] B1.4 Is fractal type (fBm, ridged, billow, ping-pong) a source property or a transform?
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
- [ ] B3.2 How is evaluation order pinned? Is it topological, declaration order, or both?
      **Topological, ties broken by node ID.** Canonical form also fixes key order and
      normalises `-0.0`.
- [x] B3.8 How are graph constants serialised?
      **Exact `f32` bits as `u32` hex** plus a decimal comment for humans. The hash covers
      bits only — never comments, whitespace, or key order. Integers are plain decimal.
      NaN/inf are rejected at load. `-0.0` normalises to `+0.0` before hashing. Hash algorithm
      is pinned (xxh3-128 or SHA-256) over canonical bytes; the loader recomputes and rejects
      a mismatch with the world description. A comment that disagrees with its bits is a lint
      warning, never a silent fix.
- [ ] B3.3 Are intermediate values cached per tile, and what is the eviction rule?
- [ ] B3.4 What is the maximum graph depth and node count for the first world?

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
- [ ] B4.4 Is sampling separable (evaluate x, z, y independently) or fully 3D?

### B5 Cost and parity
- [x] B5.3 Is a GPU sampling path allowed to differ in the last bits, and how is that
      reconciled with golden hashes?
      **CPU-only for anything that decides a block.** Field sampling, `floor(h)`, carve
      thresholds, feature placement, and material/biome selection run on the CPU evaluator.
      GPU is for rendering, culling, display lighting, upload, post, and debug visualisation.
      A GPU preview may read CPU tiles or approximate, but never writes back to tiles, chunks,
      or saves. Revisit only with the cross-platform project and proven bit equality.
- [ ] B5.1 What is the measured cost per field sample on the reference machine?
- [ ] B5.2 Which fields may be evaluated lazily vs eagerly?
- [ ] B5.4 How is field cost budgeted per chunk at the target radius?

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
- [ ] C1.7 Does the beach plateau need a minimum width floor (e.g. flatten `|base| < 1` to 0)
      so a steep coastline cannot produce a zero-block beach?
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
- [ ] C1.3 Is elevation the sum of contributions, or a spline lookup per continentalness?
- [ ] C1.4 Is there a flat "spawn plateau" near origin, or is spawn found, not made?

### C2 Shaping
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
- [ ] C2.3 Are valleys carved by the height function or left to hydrology?

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
- [ ] C3.4 Does interpolation use the four corners or a bicubic patch, and is that stable?

### C4 Vertical layout
- [x] C4.1 Bedrock: flat at y=0, noisy depth, or none?
      **Flat bedrock at y=0.** No cave, erosion, or water may breach it.
- [ ] C4.2 How many air blocks above the highest terrain before the build limit?
- [x] C4.3 Is the world 128 tall for every column, including ocean floors?
      **No: the world is 384 tall.** This supersedes the 128-block constant and forces
      vertical residency, sky-light columns, and a section layout change.
- [ ] C4.4 What happens at y<0 and y>=128: clamp, void, or error?

---

## D. Erosion and hydrology

### D1 Depression filling
- [ ] D1.1 Priority-flood with epsilon, or plain fill to the lowest outlet?
- [ ] D1.2 Is the filled surface stored, or only the implied water depth?
- [ ] D1.3 How is the tile border made deterministic when a basin crosses a seam?
- [ ] D1.4 Is filling run before or after erosion, and does it iterate?

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
- [ ] D3.5 Do rivers reach the sea, or can they end in a lake? Is a closed basin allowed?

### D4 Thermal erosion
- [x] D4.1–D4.3 **Deferred with hydraulic erosion.** v1 ships no erosion solver of any kind.
      River carving is separate and unaffected.
- [ ] D4.4 When the solver lands, does it run on the macro height before routing (changing
      rivers) or after (rivers fixed)? This determines whether adding erosion is a visual-only
      version bump or a world-changing one.

### D5 Hydraulic erosion
- [x] D5.1 Droplet-based, grid-based, or neither for the first world?
      **Neither. Deferred.** v1 terrain comes from the graph plus accumulation carving.
      Extension point reserved: a named `heightfield_post` stage between macro height and flow
      routing, a no-op in v1, pure function of `(description, tile)`. Adding a solver later is
      a version bump with its own goldens. Rivers, lakes, and carving must not read any erosion
      output, so adding it later does not change their logic.
- [ ] D5.2 If droplets: count, lifetime, inertia, capacity constant, deposit/erode rates,
      evaporation?
- [ ] D5.3 Is the droplet RNG keyed per tile so results do not depend on tile order?
- [ ] D5.4 How are erosion results clamped so a tile cannot dig below its neighbours' floor?

### D6 Resistance
- [ ] D6.1 What is the resistance map's resolution and value range?
- [ ] D6.2 Is resistance uniform until geology lands, and does that bump the generator
      version?
- [ ] D6.3 Does resistance affect thermal, hydraulic, or both?

### D7 Water bodies
- [x] D7.1 Is the sea a filled volume or a surface plane with a depth field?
      **Water is a normal block everywhere**, including 32-deep oceans. Generation fills
      `top_solid_y+1 ..= SEA` per column, and sections fully covered by water or solid become
      `Uniform` (K1.5), so no allocation. Sections mixing water with one other block use a
      1-bit palette. The mesher emits faces only at water/air and water/solid interfaces, never
      water/water. Lighting treats water as a skylight attenuator. Air below `SEA` stays air:
      caves and player spaces are not flooded by definition. Flow behaviour is a separate
      decision.
- [ ] D7.2 Are lakes global-level, per-basin, or per-tile with shared outlet heights?
      **Per basin, spill at outlet, closed basins allowed.** Level = outlet's filled height,
      floored once. A lake needs `max depth >= 2` and `area >= 12 cells` (`LAKE_MIN_CELLS`),
      else it stays dry. Open lakes feed the outlet as a river source with their accumulation;
      endorheic lakes are allowed and not drained in v1. Never filled above
      `MAX_LAKE_Y = MAX_TERRAIN_Y - 32`. A lake at or below `SEA` is just sea and is not
      recorded. Level/extent/outlet are per-tile pure outputs; boundary basins use the halo.
- [ ] D7.3 Is the water table a field, and does it feed wells or underground lakes?
- [x] D7.4 River and lake water share one level rule, or different?
      **Stepped, blocks only.** River level per cell `floor(h_center - 1)`, then made monotone
      downstream: `level[c] = min(level[c], level[receiver]+1)`. Per-block level is the
      bilinearly interpolated value floored once. Clamped to `bed_y+1 ..= bank_top-1`; at a lake
      outlet `>=` lake level; at the sea it meets `SEA` and never exceeds it. Static water only:
      no simulation, no currents.

### D8 Tiling
- [ ] D8.1 Tile size 512 with a 64 border: is that confirmed, and is the border counted in
      the tile or added?
- [ ] D8.2 Are tiles generated in a fixed order, or order-independent by construction?
- [ ] D8.3 What is the cost of one tile, and how many can be in flight?
- [ ] D8.4 How is a tile invalidated when climate or geology changes?

### D9 Volume repair
- [ ] D9.1 Which interactions trigger a repair, in priority order?
- [ ] D9.2 The `32³` window plus 8 margin: is the margin in the input, the output, or both?
- [ ] D9.3 Is the repair idempotent, and how is that tested?
- [ ] D9.4 What is the fallback order when two repairs conflict in one window?

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
- [ ] E1.3 Is humidity independent noise, distance to water, or rain-shadow output?
      Partly: `humidity = unit(noise) * (1 - strength*shadow) + maritime*coast`.
- [ ] E1.4 Is seasonality a field at all in the first world?

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
- [ ] E2.4 Is the shadow a multiplier on humidity or an additive drying term?
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
      `m = d2_second - d2_best`, `w = smoothstep(0, BLEND_W, m)` (1 deep inside, 0.5 at the
      border). Top two biomes only. Material/tree/cover pick best if
      `hash01(seed, "biome_dither", x, z) < w`, else second. Hash keyed on `(x,z)` only, never
      `y`, so a column has one biome and chunk order does not matter. Height and snow line use
      undithered lattice climate, so terrain stays smooth.
- [x] E3.5 What does a field tile store per cell for biomes?
      **Three parallel `u8` arrays:** `biome_best`, `biome_second`, `biome_margin`, with
      `q = clamp(floor(m/BLEND_W*255 + 0.5), 0, 255)`. Dither reconstructs
      `w = 0.5 + 0.5*smoothstep01(q/255)`: `q=0` is the border (`w=0.5`), `q=255` is deep
      inside (`w=1`). One helper `biome_pick(tile, cell, x, z) -> u8` owns the logic. Nearest
      lattice cell decides the cell; no interpolation. Single-biome worlds set
      `second = best`, `q = 255`. Climate (`temp`, `humidity`) is stored separately as `f32`
      lattice arrays and never feeds terrain through the biome arrays. ~3 bytes/cell, ~3.7 KB
      per 128-block tile.
- [ ] E3.6 How is `BLEND_W` calibrated so the transition really spans 24 blocks, and what
      measurement proves it? `d2` margin is not proportional to distance in blocks.
- [ ] E3.2 How many biomes exist and how are they named and versioned?

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
- [ ] E4.6 **Inconsistency to fix.** E4.3 reads per-block neighbour tops, but B4.2 says a field
      tile stores lattice samples only. Slope therefore needs a *derived per-block height tile*
      with a 1-block halo: a second tile type that must be defined.
- [ ] E4.7 Underwater surfaces: does the sea-floor stack vary by depth (sand near shore,
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
- [ ] F1.4 Do both modes feed one baseline type with no mode flag downstream?
- [ ] F1.5 What is the exact order of the 3D passes per column: surface detail, then cave
      carve, then fill? A cave carved before detail can be sealed by it, and detail written
      after carve can float inside a cave.

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
- [ ] F2.2 Is a cave a threshold on a noise pair, or a distance to a curve? (Answered for
      spaghetti: noise pair.)
- [ ] F2.3 Do caves respect a depth window, and what is it?
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
- [ ] F4.2 Are cave walls given a different material, and by what rule?
- [ ] F4.3 Is gravel/sand placed under water automatically?
- [ ] F4.4 Are ores placed here or in geology?

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
- [ ] G2.4 **Ordering question.** Fault ramps and `strata_warp` deform the index, not height, so
      the "visible fault scarp" only appears where terracing happens to align. Is a real
      height scarp acceptable as a later bump, or should faults be dropped rather than look
      like layers sliding inside a cliff?

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
- [ ] H3.5 Do trees replace leaves in water, and do they float or stand on the seabed?
      Rule says no water writes at all; confirm trees simply never generate in water.
- [ ] H3.6 Is tree growth ever simulated, or generation-only?
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
- [ ] I1.4 Is there a uniform-section optimisation (all air, all stone)?

### I2 Baseline
- [ ] I2.1 What exactly is hashed for a baseline: blocks, palette, or both?
- [ ] I2.2 Is the baseline ever mutated in place, or always replaced?
- [ ] I2.3 How does a stage request neighbour blocks without loading a neighbour chunk?
- [ ] I2.4 What padding does a stage receive, and is it copied or borrowed?

### I3 Effective chunk
- [ ] I3.1 Is the effective chunk materialised, or is the overlay read on demand?
- [ ] I3.2 What is the revision type, and does it fit in `u64` forever?
- [ ] I3.3 Does collision read the effective chunk or a cached snapshot?
- [ ] I3.4 What is the memory cost per loaded chunk at full radius?

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
- [ ] J1.4 Are edits to air recorded, or is removal implicit?

### J2 Ordering
- [ ] J2.1 Does last-write-wins apply, and what is the tie-break?
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
- [ ] J4.5 **Fork to confirm.** J4.1 contradicts "baseline is regenerable + disposable 2 GB
      cache" in the pipeline doc. With always-saved effective sections there is no baseline
      cache, no regeneration path, and migration must transform stored data rather than
      regenerate it.
- [x] J4.2 Is the baseline written at all, or regenerated on load?
      **Effective sections only**, plus hashes for verification.
- [ ] J4.3 Is there a per-world or global cache budget, and what evicts first?
      Unclear now that baselines are not cached on disk.
- [ ] J4.4 Is the save format documented byte-for-byte before shipping?

### J5 Migration
- [ ] J5.1 Pin, migrate, fork: who chooses, and when is the choice shown?
- [ ] J5.2 How are unmappable edits reported, and can the report be exported?
- [ ] J5.3 Is migration resumable?
- [ ] J5.4 Does migration ever touch the original files?

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
- [ ] K1.3 Is the ring finite (2,000-block world) or unbounded for the first world?

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
- [ ] K2.3 Can a job be preempted mid-stage, or only before it starts?
- [ ] K2.4 What is the queue depth cap per class?

### K3 Cancellation
- [ ] K3.1 Epoch per request, per chunk, or per world?
- [ ] K3.2 Is a dropped result counted separately from a cancelled job?
- [ ] K3.3 How long can a stale job run before it is killed?
- [ ] K3.4 Does cancellation free memory immediately?

### K4 Budgets
- [x] K4.1 Work per frame per stage, in ms or items?
      **`MAX_COLUMNS_PER_FRAME = 2`, `MAX_UPLOAD_BYTES_PER_FRAME = 4 MiB`**, runtime config.
      Class 0 is exempt from the column cap but not the byte cap. `APPLY_TIME_CAP = 3 ms` is a
      safety stop. A column's upload may split across frames per section. Post-teleport the caps
      rise (8 columns / 16 MiB) until the player's 3×3 is resident.
- [x] K4.2 Max in-flight jobs per stage?
      **`GENERATE_AHEAD = 12` ready columns**; workers pause when the ready queue is full.
- [ ] K4.3 Degradation order under load, exactly?
- [ ] K4.4 What is the teleport rule: drop everything, or keep the nearest column?

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
- [ ] L1.3 Is sunset/sunrise colour applied per block or in the shader only?
      Shader only (implied); still to confirm.

### L2 Block light
- [ ] L2.1 Which emitters exist in the first world?
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
- [ ] L4.2 Is coarse light mipped, recomputed, or sampled from level 0?
- [ ] L4.3 Do LOD seams blend light or accept a step?

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
- [ ] M3.3 What is the target vertex and triangle budget per chunk?
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
- [ ] N2.1 Screen-space error, distance, or both?
      Distance (Chebyshev radii) in v1.
- [x] N2.2 What is the hysteresis to prevent level thrash?
      **2 columns**, same style as K1.4.
- [ ] N2.3 Is selection CPU-side, GPU-side, or both with CPU fallback?
      CPU-side in v1; indirect draws only.
- [ ] N2.4 What is the max node count for the target radius?

### N3 Consistency
- [x] N3.1 Conservative any-solid downsample rule confirmed?
      **Most-frequent-solid, not any-solid.** Differs from the earlier design note; log it.
- [x] N3.2 How is a coarse/fine boundary stitched?
      **Skirts:** a downward quad strip of `SKIRT_DEPTH = 2` cells along the lower-resolution
      edge. A LOD(n+1) column draws only where its covered LOD(n) columns are not
      resident-and-meshed, so there is no overlap and no gap.
- [x] N3.3 Is a coarse section hidden while its children generate?
      **Yes** — the boundary rule above is the whole mechanism.
- [ ] N3.4 How much pop is acceptable, measured how?
- [x] N3.5 How does a coarse section get its light?
      **No BFS, no stored light.** Per-face approximation at mesh time: sky = 15 if the air-side
      cell is open to the ceiling, else `15 - SHADE_PER_CELL * cells_below_first_opaque`, minus
      `WATER_SKY_ATTEN` per water cell above. Block light 0, AO 0. Merge key includes the sky
      nibble. A shader curve eases the LOD0/LOD1 step; fully exposed coarse faces match LOD0.

### N4 Invalidation
- [x] N4.1 Does a level-0 edit invalidate all ancestors, lazily?
      **Yes** — covering LOD1/LOD2 sections rebuild as class 4 work.
- [ ] N4.2 What is the latency budget for L3/L4?
- [x] N4.3 Does an edit at coarse level ever write down to level 0?
      **Never.** LOD is read-only derived data.

---

## O. Rendering and precision

### O1 Draw path
- [ ] O1.1 One draw per chunk, batched, or indirect?
- [ ] O1.2 Is frustum culling per chunk, per section, or hierarchical?
- [ ] O1.3 Is occlusion culling in scope for the first world?
- [ ] O1.4 Are opaque, cutout, and water separate passes with separate order?

### O2 Precision
- [ ] O2.1 At what travel distance does `f32` become visible?
- [ ] O2.2 Camera-relative rendering, origin rebase, or both?
- [ ] O2.3 Does rebasing invalidate anything, or is it a render-only transform?

### O3 Edge
- [ ] O3.1 Does fog hide the terrain edge at max radius?
- [ ] O3.2 Is there a sky/terrain blend at the horizon?
- [ ] O3.3 Are water and fog composited in a defined order?

---

## P. Performance and budgets

- [x] P1.1 Reference hardware for every target number?
      **2020 mid-range desktop**, 1080p, 60 FPS, 4–6 core CPU, GTX 1660 / RX 5600 class.
- [ ] P1.2 Frame budget split: generation, lighting, meshing, upload, render?
      Partly: apply ≤ 3 ms (K4.1); edit sub-budgets in P1.7.
- [x] P1.3 Target radius and FPS: 64 chunks at 60 FPS confirmed as the gate?
      **`VIEW_RADIUS = 16` at 60 FPS, no frame over 20 ms while moving**, with LOD2 at the
      horizon. The old 64-chunk gate is superseded by this plus the LOD radii.
- [ ] P1.4 Memory ceiling per world, and what happens at the ceiling?
- [x] P1.5 Max worker threads, and is it `cores - 1`?
      `N = available_parallelism`; IO 1; `EDIT_WORKERS = 1 (N<=8) else 2`;
      `STREAM_WORKERS = max(2, N - 2 - EDIT_WORKERS)`. Runtime config.
- [ ] P1.6 Does a chunk load ever block the main thread, and for how long?
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
- [ ] P1.10 Is there a "fast mode" that lowers quality to hit budget?
- [ ] P1.11 What is the startup budget before the player can move?
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
- [ ] Q1.5 Debug views: field probe, height, biome, strata, LOD, light, overlay split?
      Several are named per domain (span count, biome margin, fired rule, strata band, sky
      nibble, AO, LOD light, water quads). A single index of them is still needed.
- [ ] Q1.6 Is there a terrain authoring tool in-engine, and when?
- [x] Q1.7 Are counters exported to Tracy, a HUD, or both?
      **Both**, when enabled.
- [ ] Q1.8 How does a player report a bad world with enough data to reproduce?
- [ ] Q1.9 Is there a headless generator binary for CI?
- [ ] Q1.10 Is there a map screenshot / top-down exporter?
- [ ] Q1.11 Are warnings rate-limited so a bad chunk does not spam the log?

---

## R. Testing and acceptance

- [ ] R1.1 Which stages get unit tests with fixture fields?
- [ ] R1.2 What invariants hold for every chunk (no floating sand, water level, bedrock)?
- [ ] R1.3 Golden hash coverage: how many coords, which biomes, which LODs?
- [ ] R1.4 Cross-process determinism test?
- [ ] R1.5 Seam tests: every pair of adjacent tiles, both directions?
- [ ] R1.6 Migration tests: old world, edited, unmappable edit?
- [ ] R1.7 Property tests for noise bounds and curve monotonicity?
- [ ] R1.8 Performance regression thresholds and where they run?
- [ ] R1.9 Visual acceptance: rivers reach sea, lakes in basins, 3+ biomes with shadow,
      strata on cliffs — is that the final list?
- [ ] R1.10 Are golden hashes regenerated deliberately, with a diff report?
- [ ] R1.11 Fuzz test for save/cache corruption?
- [ ] R1.12 Is there a "seed sweep" that generates N random seeds and checks invariants?
- [ ] R1.13 What is the definition of done for the first terrain milestone?
- [ ] R1.14 What would make us reject the whole approach and fall back to v0.0.7 style?

---

## S. Failure and robustness

- [ ] S1.1 Typed error enum per stage, and what carries context?
- [ ] S1.2 Worker panic: contain, retry once, or fail the chunk?
- [ ] S1.3 A stage that never completes: timeout, and what the player sees?
- [ ] S1.4 Corrupt baseline cache: detect via hash, delete, regenerate?
- [ ] S1.5 Corrupt overlay: refuse to load, or load and report?
- [ ] S1.6 GPU device loss: recover, or fatal with a clear message?
- [ ] S1.7 Out-of-memory policy: evict cache, lower radius, or crash?
- [ ] S1.8 Non-finite field output: clamp, log, or assert?
- [ ] S1.9 Bad content pack: refuse to open the world, or substitute placeholder?
- [ ] S1.10 Partial world load: allowed or forbidden?

---

## T. Versioning and migration

- [ ] T1.1 Exact rule for when `GeneratorVersion` bumps.
- [ ] T1.2 Is the version an integer, a semver, or a content hash?
- [ ] T1.3 How many old generator versions does the engine keep evaluable?
- [ ] T1.4 Does a new engine release ever auto-upgrade a world? (Should not.)
- [ ] T1.5 Content pack versioning: additive only, or breaking allowed with migration?
- [ ] T1.6 How is an unknown block stored and rendered after content removal?
- [ ] T1.7 Is there a world "fork" UI, and what does it copy?
- [ ] T1.8 Migration report format and destination?
- [ ] T1.9 Are parameter changes world-level or per-region overrides?
- [ ] T1.10 Does the engine support two generator versions in one world? (Should not.)
- [ ] T1.11 How is a bug in an old generator fixed without changing its output?
- [ ] T1.12 What is the deprecation policy for old generator versions?

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
