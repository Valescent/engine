# Far heightfield ring prototype

Answers one question from
[Geometry representation research](../../docs/dev/research/geometry-representation.md): how much
geometry does a clipmap ring need to cover the horizon, how large are the seams between levels, and do
skirts hide them?

CPU-only, dependency-free, deterministic. It does not render terrain; it builds the ring geometry,
measures it, and writes a top-down coverage preview.

## Run

```console
cargo run --release
```

Expected output (heights come from a fixed seed, so this is stable):

```text
far heightfield ring: camera=(0.000, 0.000)
level  cell(m) extent(m) ring cells   vertices  triangles skirt tris
    0      1.0      64.0       4096       4225       8192          0
    1      2.0     128.0       3072       3520       6144        512
    2      4.0     256.0       3072       3520       6144        512
    3      8.0     512.0       3072       3520       6144        512
    4     16.0    1024.0       3072       3520       6144        512
totals: vertices=18305 surface_triangles=32768 skirt_triangles=2048 total_triangles=34816
full-resolution 1 m grid over the outermost extent: 2097152 triangles
reduction: 60.24x, 1.66% of full resolution (budget < 5.00%: ok)
max_seam_gap: 6.115 m
skirt_depth: +/-8.000 m (covers gap: yes)
preview=preview.ppm
```

`preview.ppm` is a 480x480 binary PPM. Each pixel uses the innermost level whose snapped square covers
it, shaded by height and tinted by level, so the ring layout is visible at a glance. The file is
generated output and is ignored by git.

## What it does

- **Heightfield.** Three octaves of value noise (wavelengths 64, 16, and 4 m, amplitudes 12, 6, and
  2 m around a 20 m base). Deterministic, no external crates.
- **Clipmap.** Five levels, each 64 cells per side. Level 0 is a 64 m square at 1 m cells; each later
  level doubles the cell size and extent, and omits the inner half-square that the previous level
  covers. That is the ring.
- **Snapping.** Every level shares the base level's 1 m snapped centre, so the snapped squares nest
  exactly: each level's inner hole matches the previous level's square. The rings therefore shift
  together when the camera crosses a 1 m cell instead of snapping independently, which would let a
  coarse hole drift up to one fine cell past the previous square and leave an uncovered strip.
- **Seams.** Shared coordinates sample the same height, so shared vertices match exactly. The
  remaining crack is the T-junction between a coarse edge and the two finer edges beneath it. The
  prototype measures that gap (`max_seam_gap`) and adds a two-sided skirt (8 m up and 8 m down) on
  every inner ring boundary, because the finer sample can sit on either side of the coarse edge.

## Tests

`cargo test` checks exact triangle counts per level, all five levels, shared-centre snapping, exact
nesting between adjacent levels, shared-vertex equality, two-sided skirt coverage and winding,
the under-5% triangle budget, height bounds and determinism, and deterministic preview output.

## Limitations

- **No rendering and no integration.** This is geometry and measurement only; the engine work would
  add the ring behind the voxel window as a second representation.
- **Heightfield only.** No overhangs, caves, or edited terrain. The ring is a horizon fill, not a
  replacement for the voxel world.
- **Seams are covered, not removed.** The measured 6.1 m gap is hidden by an 8 m two-sided skirt.
  A shipping version would reduce the coarse amplitude or add stitch vertices instead.
- **No water or materials.** The preview tints by level, not by surface type.
