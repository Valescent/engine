# World generation research

Generation is deterministic and cheap enough to stream, but it is still a per-column cost on the
critical path of the first ring. This report covers performance and the structural features that are
out of scope today.

## Current state

- Five OpenSimplex2 fields per column: continentalness, erosion, temperature, humidity, and a 3D
  cave field. Seeds are mixed with SplitMix64 per field (ADR-0012).
- Height is `32 + (continentalness - 0.5) * 160 + (erosion - 0.5) * 32`, clamped to `1..=126`.
- Biome is a hard threshold on temperature, humidity, and erosion: desert, hills, plains.
- Caves carve cells where the 3D field exceeds `0.60`, from `y = 1` to the column height, before
  water fill.
- Trees are chunk-local: two candidate hashes per chunk, a trunk, and a leaf canopy that never
  crosses a chunk border.
- No structures, no biome blending, no rivers, no overhangs. Generation is CPU-only by spec.

Cost: about 2.5 ms per column (four chunks). The cave field is sampled for every cell in the chunk,
which is the dominant term; height and biome are computed once per column.

## Options

### 1. Padded 2D field cache

Height, biome, and the 2D noise values are per `(x, z)`. A chunk asks for its own 32x32 columns,
and the eight neighbours recompute the same columns. A padded 34x34 cache per job, or a small
per-world LRU keyed by column, removes the duplicate work and gives tree placement the neighbour
heights it needs for future cross-chunk structures.

Cheap, no format change, easy to test: assert the cached and uncached heights match.

### 2. Interpolated cave noise

Sample the 3D cave field on a coarser lattice (for example every 4 blocks) and trilinearly
interpolate inside. Terrain-scale caves tolerate that loss of detail; the threshold stays the same.
This is the classic Minecraft-style density interpolation and cuts cave noise calls by up to 64x.

Validate by comparing cave sets at a fixed seed and checking the surface still reads as caves, then
by generation time. A conservative version interpolates only below the surface.

### 3. Batch columns per job

Generate four or sixteen columns per generation job instead of one. Amortises generator setup,
improves cache locality for the 2D fields, and reduces round trips through the pool (see
[Scheduling](scheduling.md)). The cost is a coarser unit of work and more memory in flight.

### 4. Two-phase structures

Structures need positions that can cross chunk borders. The standard shape is two-phase:

1. A deterministic per-region hash decides whether a structure exists and where its origin is.
2. After a column's terrain is generated, every structure whose bounding box intersects the column
   is applied, or its blocks are queued for the chunk that owns them.

This is also how trees could grow past a single chunk, and it is the prerequisite for villages,
ruins, or larger cave entrances. It needs a stable structure ID and deterministic ordering so the
same seed always produces the same world.

### 5. Biome blending

Hard biome thresholds produce visible seams in height and surface material. Blend the two nearest
biomes' heights and surface palettes over a band, or smooth the height field before choosing blocks.
The result is subjective; tests can cover continuity (no step larger than one block across a
boundary) while screenshots cover the look.

### 6. Rivers and erosion

A river field carved below sea level, plus an erosion term that widens valleys, adds recognisable
terrain without new block types. Both are additional 2D fields; measure the generation cost before
adding them to the default path.

### 7. Density-based terrain

A 3D density field with surface extraction would give real overhangs and arches. It replaces the
heightmap model, changes cave and water logic, and costs far more noise samples. Keep it as a
separate experiment, not a default.

### 8. GPU generation

Out of scope by the v0.0.7 spec. Revisit only if CPU generation becomes the measured bottleneck at
distance 48+, and after the mesher and light paths are GPU-friendly.

## Recommendation

1. Padded 2D field cache and batched columns. Low risk, immediate load-time win.
2. Interpolated cave noise. Largest generation cost, contained change.
3. Two-phase structures, starting with a single small structure type.
4. Biome blending as a quality pass.

Preserve determinism: no global RNG, no order dependence, and a test that the same seed and
coordinates produce identical chunks across runs and across cached/uncached paths.

## Measurement

- `generation_ms / generated_columns` from the debug frame line is the per-column cost.
- Load-phase wall time from first column to last ready column at distances 12 and 24.
- Compare cave volumes at a fixed seed before and after interpolation.
- Add a headless generation benchmark next to `bench_hot_paths`.

## References

- [ADR-0012: fastnoise-lite and OpenSimplex2](../../decisions/0012-fastnoise-lite-and-opensimplex2.md)
- [Terrain pipeline design](../../design/terrain-pipeline.md)
- [Terrain interview](../../design/terrain-interview.md)
