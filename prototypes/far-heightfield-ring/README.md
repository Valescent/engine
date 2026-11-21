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
    1      2.0     128.0       3072       3392       6144        256
    2      4.0     256.0       3072       3392       6144        256
    3      8.0     512.0       3072       3392       6144        256
    4     16.0    1024.0       3072       3392       6144        256
totals: vertices=17793 surface_triangles=32768 skirt_triangles=1024 total_triangles=33792
full-resolution 1 m grid over the outermost extent: 2097152 triangles
reduction: 62.06x, 1.61% of full resolution (budget < 5.00%: ok)
max_seam_gap: 6.115 m
skirt_depth: 8.000 m (covers gap: yes)
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
- **Snapping.** Each level snaps its centre to its own cell size, so vertices do not swim as the
  camera moves; the rings only shift when the camera crosses a whole cell.
- **Seams.** Shared coordinates sample the same height, so shared vertices match exactly. The
  remaining crack is the T-junction between a coarse edge and the two finer edges beneath it. The
  prototype measures that gap (`max_seam_gap`) and adds a downward skirt on every inner ring
  boundary to cover it.

## Tests

`cargo test` checks exact triangle counts per level, all five levels, snapping stability, shared-vertex
equality, seam coverage (`max_seam_gap < skirt depth`), the under-5% triangle budget, height bounds and
determinism, and deterministic preview output.

## Limitations

- **No rendering and no integration.** This is geometry and measurement only; the engine work would
  add the ring behind the voxel window as a second representation.
- **Heightfield only.** No overhangs, caves, or edited terrain. The ring is a horizon fill, not a
  replacement for the voxel world.
- **Seams are covered, not removed.** The measured 6.1 m gap is hidden by an 8 m skirt. A shipping
  version would reduce the coarse amplitude or add stitch vertices instead.
- **No water or materials.** The preview tints by level, not by surface type.
