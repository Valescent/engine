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
- [ ] A2.1 Is the promise same-binary-only, or cross-platform bit equality?
- [ ] A2.2 If cross-platform: fixed-point fields, quantised `f32`, or a recorded baseline?
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
- [ ] B1.1 Which noise algorithms are allowed: OpenSimplex2, OpenSimplex2S, Perlin, value,
      cellular, voronoi? Which are banned and why?
- [ ] B1.2 Is noise normalised to `[0, 1]` at the source, or `[-1, 1]` with an explicit
      mapping?
- [ ] B1.3 Are octave count, lacunarity, gain, and frequency per field or per world?
- [ ] B1.4 Is fractal type (fBm, ridged, billow, ping-pong) a source property or a transform?

### B2 Transforms
- [ ] B2.1 Which domain warps are supported, and how many octaves does a warp cost?
- [ ] B2.2 Are curves splines, piecewise linear, or an expression language?
- [ ] B2.3 Is `min`/`max`/`clamp` enough, or are smooth-min and terrace needed?
- [ ] B2.4 Does a transform operate on a scalar, a 2D field, or a sampled tile?

### B3 Composition
- [ ] B3.1 Is the graph declarative data, fixed code, or a compiled closure chain?
- [ ] B3.2 How is evaluation order pinned? Is it topological, declaration order, or both?
- [ ] B3.3 Are intermediate values cached per tile, and what is the eviction rule?
- [ ] B3.4 What is the maximum graph depth and node count for the first world?

### B4 Tiling and sampling
- [ ] B4.1 What is the canonical sample grid: per block, per 4-block macro cell, or per LOD
      sample?
- [ ] B4.2 Does a tile include a border, and how wide is it per consumer?
- [ ] B4.3 Is a sample at integer coordinates taken at the block centre, corner, or corner
      plus half?
- [ ] B4.4 Is sampling separable (evaluate x, z, y independently) or fully 3D?

### B5 Cost and parity
- [ ] B5.1 What is the measured cost per field sample on the reference machine?
- [ ] B5.2 Which fields may be evaluated lazily vs eagerly?
- [ ] B5.3 Is a GPU sampling path allowed to differ in the last bits, and how is that
      reconciled with golden hashes?
- [ ] B5.4 How is field cost budgeted per chunk at the target radius?

---

## C. Macro landform

### C1 Elevation
- [ ] C1.1 Which fields drive base elevation, and in what order?
- [ ] C1.2 What is the elevation range in blocks: min, sea level, max?
- [ ] C1.3 Is elevation the sum of contributions, or a spline lookup per continentalness?
- [ ] C1.4 Is there a flat "spawn plateau" near origin, or is spawn found, not made?

### C2 Shaping
- [ ] C2.1 How does erosion/roughness modulate slope rather than absolute height?
- [ ] C2.2 Are mountains ridged, plateaued, or both, and where is the switch?
- [ ] C2.3 Are valleys carved by the height function or left to hydrology?
- [ ] C2.4 Is there a coastal shelf, a beach band, and a cliff band, with what widths?

### C3 Quantisation
- [x] C3.1 For `h = 72.4`, is the top solid block at 72 or 73? State the exact rule.
      **One helper: `top_solid_y(h: f32) -> i32 = floor(h)`.** Surface, water, caves, and
      features all read it. Nothing else quantises a height.
- [x] C3.2 Does the rule hold for negative and clamped heights?
      **Yes.** `floor` is defined for the whole range; clamping happens after.
- [ ] C3.3 Is the macro grid bilinearly interpolated to blocks, or nearest-sampled with
      features? What happens at a cell edge?
- [x] C3.5 Is the height field kept as `f32` everywhere?
      **Yes.** Blending, slopes, caves, and features read the float. The v0.0.7 `round()` in
      the height formula is dropped; quantisation exists only at `top_solid_y`.
- [ ] C3.4 Does interpolation use the four corners or a bicubic patch, and is that stable?

### C4 Vertical layout
- [ ] C4.1 Bedrock: flat at y=0, noisy depth, or none?
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
- [ ] D2.1 D8 or D-infinity? What are the tie-breaking rules on equal neighbours?
- [ ] D2.2 Is accumulation normalised by area, and in what units?
- [ ] D2.3 How many iterations before flow is considered converged?
- [ ] D2.4 Is flow computed on the macro grid only, or also per block near rivers?

### D3 Rivers
- [ ] D3.1 What accumulation threshold starts a river, and does it vary by climate?
- [ ] D3.2 Is width derived from accumulation, from a channel field, or both?
- [ ] D3.3 Is the channel carved as a V, a U, or a depth profile with banks?
- [ ] D3.4 Do rivers meander, and is meander applied before or after carving?
- [ ] D3.5 Do rivers reach the sea, or can they end in a lake? Is a closed basin allowed?

### D4 Thermal erosion
- [ ] D4.1 What is the talus angle, and is it per material?
- [ ] D4.2 How many iterations, and how is mass conserved?
- [ ] D4.3 Does it run before hydraulic erosion, after, or interleaved?

### D5 Hydraulic erosion
- [ ] D5.1 Droplet-based, grid-based, or neither for the first world?
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
- [ ] D7.1 Is the sea a filled volume or a surface plane with a depth field?
- [ ] D7.2 Are lakes global-level, per-basin, or per-tile with shared outlet heights?
- [ ] D7.3 Is the water table a field, and does it feed wells or underground lakes?
- [ ] D7.4 River and lake water share one level rule, or different?

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
- [ ] E1.1 Is temperature latitude-based, altitude-based, noise-based, or a sum?
- [ ] E1.2 What is the lapse rate per block of altitude?
- [ ] E1.3 Is humidity independent noise, distance to water, or rain-shadow output?
- [ ] E1.4 Is seasonality a field at all in the first world?

### E2 Rain shadow
- [ ] E2.1 Which direction does the prevailing wind march, and is it constant per world?
- [ ] E2.2 How far does the march go, and how does it decay?
- [ ] E2.3 Does it reuse the hydrology grid exactly, including the border?
- [ ] E2.4 Is the shadow a multiplier on humidity or an additive drying term?

### E3 Biome selection
- [ ] E3.1 Table lookup with thresholds, nearest-point in climate space, or weighted blend?
- [ ] E3.2 How many biomes exist and how are they named and versioned?
- [ ] E3.3 Are transitions hard, dithered, or blended? Over how many blocks?
- [ ] E3.4 Can a biome alter height after selection, and does that reorder the pipeline?

### E4 Surface rules
- [ ] E4.1 Is the surface rule a per-biome palette or a predicate list?
- [ ] E4.2 How deep is topsoil, and does depth vary by slope and biome?
- [ ] E4.3 Is there a slope rule that exposes stone above a threshold angle?
- [ ] E4.4 Where does snow start: altitude, temperature, or both?

---

## F. Volume synthesis

### F1 Model
- [ ] F1.1 Heightmap plus carve is the first mode: is density mode in the same release?
- [ ] F1.2 Is the density function an SDF with a gradient, or a threshold on noise?
- [ ] F1.3 What resolution is evaluated: per block, or per cell then expanded?
- [ ] F1.4 Do both modes feed one baseline type with no mode flag downstream?

### F2 Caves
- [ ] F2.1 Which cave families ship first: cheese, spaghetti, noodle, worm, none?
- [ ] F2.2 Is a cave a threshold on a noise pair, or a distance to a curve?
- [ ] F2.3 Do caves respect a depth window, and what is it?
- [ ] F2.4 Are cave entrances intentional, or emergent from the surface intersection?

### F3 Overhangs
- [ ] F3.1 Overhangs, arches, and floating rock: which are in scope for the first pass?
- [ ] F3.2 Are they caused by 3D noise on the surface band, or by explicit primitives?
- [ ] F3.3 How far below the surface does 3D detail apply before it is wasted cost?
- [ ] F3.4 Is there a rule preventing floating islands below the sea floor?

### F4 Materials in volume
- [ ] F4.1 Is material chosen by depth, by strata, by slope, or by a priority list?
- [ ] F4.2 Are cave walls given a different material, and by what rule?
- [ ] F4.3 Is gravel/sand placed under water automatically?
- [ ] F4.4 Are ores placed here or in geology?

---

## G. Geology and materials

### G1 Strata
- [ ] G1.1 Are strata horizontal layers, a warped stack, or a 3D field?
- [ ] G1.2 How many rock types are in the first world?
- [ ] G1.3 Do strata continue under oceans unchanged?
- [ ] G1.4 Is there a depth floor where everything becomes one rock?

### G2 Deformation
- [ ] G2.1 Folds: sinusoidal displacement of the layer coordinate?
- [ ] G2.2 Faults: hard offsets, and how are they keyed so they stay straight across chunks?
- [ ] G2.3 Are deformation fields 2D or 3D?

### G3 Veins
- [ ] G3.1 Vein shape: ellipsoid clusters, curves, or noise threshold?
- [ ] G3.2 Is vein placement host-rock dependent?
- [ ] G3.3 Is vein clustering done in discovery or application?
- [ ] G3.4 Do veins affect erosion resistance?

---

## H. Features and scatter

### H1 Discovery
- [ ] H1.1 What is the feature region size, and does it exceed the largest feature?
- [ ] H1.2 How is a candidate identified stably across runs and versions?
- [ ] H1.3 Is the index stored, recomputed, or cached on disk?
- [ ] H1.4 How are candidates deduplicated when regions overlap?

### H2 Application
- [ ] H2.1 What is the write order between features, and is it deterministic?
- [ ] H2.2 What clips a feature at the chunk boundary: block writes, or region ownership?
- [ ] H2.3 Can a feature overwrite terrain, water, or another feature?
- [ ] H2.4 Is application idempotent for a repeated chunk generation?

### H3 Trees
- [ ] H3.1 Species count and per-biome table?
- [ ] H3.2 Shape grammar: trunk, branches, canopy volume, roots?
- [ ] H3.3 Spacing rule: Poisson, minimum distance, or density field?
- [ ] H3.4 Slope and altitude limits per species?
- [ ] H3.5 Do trees replace leaves in water, and do they float or stand on the seabed?
- [ ] H3.6 Is tree growth ever simulated, or generation-only?

### H4 Small scatter
- [ ] H4.1 Grass, flowers, boulders, snow caps: same pipeline or a cheaper one?
- [ ] H4.2 Are they rendered as blocks, cross-quads, or instanced props?
- [ ] H4.3 Do they survive edits nearby?

---

## I. Chunk data model

### I1 Storage
- [ ] I1.1 Is a block `u16` with a registry, a palette per section, or a bit-packed ID?
- [ ] I1.2 Is the array `YZX` or `XYZ`, and what is the index formula?
- [ ] I1.3 Are sections 32³ with 4 per column, and is a section the unit everywhere?
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
- [ ] J1.1 Is an edit `(pos, old, new, revision)`, or does it coalesce?
- [ ] J1.2 Who assigns revision: main thread, world, or a monotonic clock?
- [ ] J1.3 Is there an author field for future multiplayer?
- [ ] J1.4 Are edits to air recorded, or is removal implicit?

### J2 Ordering
- [ ] J2.1 Does last-write-wins apply, and what is the tie-break?
- [ ] J2.2 Is the overlay a map, a log, or a log with periodic snapshots?
- [ ] J2.3 Can an edit be reverted to baseline, and does that remove the record?

### J3 Durability
- [ ] J3.1 Journal flush policy: every edit, every N edits, every N seconds?
- [ ] J3.2 Is compaction incremental or full?
- [ ] J3.3 What happens on a partial journal write?
- [ ] J3.4 Is `fsync` used, and how often?

### J4 Format
- [ ] J4.1 Does save v4 keep `VCHK` readable forever, or only for one version?
- [ ] J4.2 Is the baseline written at all, or regenerated on load?
- [ ] J4.3 Is there a per-world or global cache budget, and what evicts first?
- [ ] J4.4 Is the save format documented byte-for-byte before shipping?

### J5 Migration
- [ ] J5.1 Pin, migrate, fork: who chooses, and when is the choice shown?
- [ ] J5.2 How are unmappable edits reported, and can the report be exported?
- [ ] J5.3 Is migration resumable?
- [ ] J5.4 Does migration ever touch the original files?

---

## K. Streaming and residency

### K1 Shape
- [ ] K1.1 Horizontal radius in chunks, and is it square, circular, or frustum-biased?
- [ ] K1.2 Vertical policy for a 128-tall world: all four sections always, or lazy?
- [ ] K1.3 Is the ring finite (2,000-block world) or unbounded for the first world?
- [ ] K1.4 What is the load-distance vs unload-distance hysteresis?

### K2 Priority
- [ ] K2.1 Are the four classes (urgent, visible, prefetch, background) final?
- [ ] K2.2 Within a class, what orders work: distance, direction, revision age?
- [ ] K2.3 Can a job be preempted mid-stage, or only before it starts?
- [ ] K2.4 What is the queue depth cap per class?

### K3 Cancellation
- [ ] K3.1 Epoch per request, per chunk, or per world?
- [ ] K3.2 Is a dropped result counted separately from a cancelled job?
- [ ] K3.3 How long can a stale job run before it is killed?
- [ ] K3.4 Does cancellation free memory immediately?

### K4 Budgets
- [ ] K4.1 Work per frame per stage, in ms or items?
- [ ] K4.2 Max in-flight jobs per stage?
- [ ] K4.3 Degradation order under load, exactly?
- [ ] K4.4 What is the teleport rule: drop everything, or keep the nearest column?

---

## L. Lighting

### L1 Sky light
- [ ] L1.1 Is skylight 15 levels with flood fill, or a coarser model?
- [ ] L1.2 How is vertical propagation through a 128-tall column handled without loading above?
- [ ] L1.3 Is sunset/sunrise colour applied per block or in the shader only?

### L2 Block light
- [ ] L2.1 Which emitters exist in the first world?
- [ ] L2.2 Is light stored per block, per section corner (AO), or both?
- [ ] L2.3 Is light RGB or single-channel?

### L3 Consistency
- [ ] L3.1 What is the exact border rule for a neighbour edit?
- [ ] L3.2 Is light computed on the effective chunk, and does an overlay edit dirty it?
- [ ] L3.3 Is incremental repair allowed to differ from full recompute?

### L4 LOD and AO
- [ ] L4.1 What is the AO rule in the greedy mesher, and how does it affect merging?
- [ ] L4.2 Is coarse light mipped, recomputed, or sampled from level 0?
- [ ] L4.3 Do LOD seams blend light or accept a step?

---

## M. Meshing

### M1 Greedy rules
- [ ] M1.1 What merge key includes: material, light, AO, tint, transparency?
- [ ] M1.2 Is the mesher chunk-local with a neighbour border, and how wide?
- [ ] M1.3 Are diagonal AO occluders included in the key?
- [ ] M1.4 Is output ordering deterministic and sorted?

### M2 Water and transparency
- [ ] M2.1 Separate pass, separate mesh, or per-face material flag?
- [ ] M2.2 Are water faces culled against water, air, or both?
- [ ] M2.3 Is there a surface-only water mesh, and does it animate?
- [ ] M2.4 Cutout blocks (leaves, grass): alpha test, separate pass, or geometry?

### M3 Scale
- [ ] M3.1 Vertex format: position, normal, uv, tint, light, AO — what is actually needed?
- [ ] M3.2 Is a mesh ever split into sub-meshes for culling or draw order?
- [ ] M3.3 What is the target vertex and triangle budget per chunk?
- [ ] M3.4 Is meshing parallel per chunk with no cross-chunk writes?

---

## N. LOD hierarchy

### N1 Levels
- [ ] N1.1 Five levels confirmed: 1, 2, 4, 8, 16 blocks per sample?
- [ ] N1.2 Section size stays 32³ samples at every level?
- [ ] N1.3 Is level 0 the only authority for collision and edits?
- [ ] N1.4 How is a coarse section generated without its children?

### N2 Selection
- [ ] N2.1 Screen-space error, distance, or both?
- [ ] N2.2 What is the hysteresis to prevent level thrash?
- [ ] N2.3 Is selection CPU-side, GPU-side, or both with CPU fallback?
- [ ] N2.4 What is the max node count for the target radius?

### N3 Consistency
- [ ] N3.1 Conservative any-solid downsample rule confirmed?
- [ ] N3.2 How is a coarse/fine boundary stitched: skirts, transition mesh, or accept?
- [ ] N3.3 Is a coarse section hidden while its children generate?
- [ ] N3.4 How much pop is acceptable, measured how?

### N4 Invalidation
- [ ] N4.1 Does a level-0 edit invalidate all ancestors, lazily?
- [ ] N4.2 What is the latency budget for L3/L4?
- [ ] N4.3 Does an edit at coarse level ever write down to level 0? (Should not.)

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

- [ ] P1.1 Reference hardware for every target number?
- [ ] P1.2 Frame budget split: generation, lighting, meshing, upload, render?
- [ ] P1.3 Target radius and FPS: 64 chunks at 60 FPS confirmed as the gate?
- [ ] P1.4 Memory ceiling per world, and what happens at the ceiling?
- [ ] P1.5 Max worker threads, and is it `cores - 1`?
- [ ] P1.6 Does a chunk load ever block the main thread, and for how long?
- [ ] P1.7 What is the cost of a single edit end-to-end (remesh latency)?
- [ ] P1.8 GPU upload budget per frame in bytes?
- [ ] P1.9 Are generation and meshing ever on the same pool?
- [ ] P1.10 Is there a "fast mode" that lowers quality to hit budget?
- [ ] P1.11 What is the startup budget before the player can move?
- [ ] P1.12 What is measured before a stage is considered done?

---

## Q. Observability and tooling

- [ ] Q1.1 Exact metric list per stage (name, unit, aggregation)?
- [ ] Q1.2 Is there a per-chunk trace view, and how is it triggered?
- [ ] Q1.3 Capture file contents: description, camera path, edits, timings, build hash?
- [ ] Q1.4 Replay determinism: same visible result, or same block hashes?
- [ ] Q1.5 Debug views: field probe, height, biome, strata, LOD, light, overlay split?
- [ ] Q1.6 Is there a terrain authoring tool in-engine, and when?
- [ ] Q1.7 Are counters exported to Tracy, a HUD, or both?
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
