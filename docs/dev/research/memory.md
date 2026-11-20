# Memory research

Resident memory is the nearest ceiling on render distance. At distance 24 the engine holds about
5,300 settled chunks in 626 MB, roughly 118 KiB per stored chunk before the v0.0.8 vertex packing
and somewhat less after it. At distance 32 that is roughly 230 MB more before meshes.

## Where the bytes go

| Source | Per chunk | At 5,394 chunks | Notes |
|---|---:|---:|---|
| Block array | 32 KiB | 173 MB | One `[Block; 32768]`, one byte per cell |
| Light field | 39 KiB | 211 MB | Two nibble planes over a 34^3 halo |
| Mesh buffers | ~40 KiB | ~215 MB | Opaque plus water, split across LODs |
| Allocator and wgpu | - | ~38 MB | Fragmentation, buffers, bind groups |

The per-chunk figures are the pre-v0.0.8 baseline. The 12-byte indexed vertex keeps mesh buffers
lower; the settled distance-24 working set was 626 MB before the headless gate was added.

## v0.0.9 packing

Block storage is now four bits per cell, 16 KiB per chunk, and `VCHK` is unchanged. The packed store
is a direct table, not a palette: the nine block IDs fit a nibble, so a mod or content set that needs
more than sixteen types will need a palette index instead.

Measured after the change (seed 42, `ENGINE_HEADLESS=1`): 190.6 MB of blocks and light at distance 24
and 334.6 MB at distance 32, down from 381.2 MB and 669.2 MB. Meshing costs about 15-25% more per
chunk because of the nibble decode; it runs on workers, so the trade is memory for off-frame CPU.

Light is sparse as well. The sky plane is always stored; the block-light plane exists only when a lamp
is within the 3x3x3 neighbourhood. Generated terrain places no lamps, so the typical field is 19.2 KiB
instead of 39 KiB. A lamp restores the full plane for the 27 chunks around it.

A light job is transiently far larger than a chunk: it copies a 3x3 footprint (27 chunks, 864 KiB)
and allocates a 96^3 working grid with three `u8` arrays (2.65 MB). With 44 jobs in flight the
engine can hold about 150 MB of scratch that never shows up in the per-chunk table.

## Options

### 1. Indexed and packed vertices — implemented in v0.0.8

The mesh used to emit six 40-byte vertices per quad and draw non-indexed. Two independent wins:

- **Indices**: four vertices plus six indices is 160 + 12 bytes (16-bit indices) instead of 240, a
  28% cut. The GPU post-transform cache also reuses shaded vertices. The mesher picks 16- or
  32-bit indices per chunk.
- **Packing**: positions fit in chunk-local `u8` triples, the face index replaces the normal, and
  the texture layer shares the second word. The live vertex is 12 bytes, down from 40.

Combined, the mesh side fell from ~215 MB to ~60-80 MB at distance 24, with the same or better
GPU vertex throughput. The cost was shader decode and mesher changes; both were contained.

### 2. Nibble block IDs

`Block` has nine values today, so four bits per cell is enough. That halves the block array from
32 KiB to 16 KiB and removes 86 MB at distance 24. Reads and writes become shift-and-mask; the
mesher, light snapshots, and serialization are the only consumers.

A palette plus bit-packed indices (the Minecraft chunk format) goes further: typical terrain uses
three to six block types, so two to three bits per cell is 8-12 KiB per chunk. That is a larger
win but needs a per-chunk palette, a bits-per-cell choice, and a new `VCHK` version. The nibble
step is the cheap 80% of it.

### 3. Compress `VCHK`

A `VCHK` file is a 22-byte header plus four bytes per non-air block, uncompressed. A generated
chunk is typically 20-60 KiB. Block data is highly structured, so LZ4 or zstd should give 3-10x.
LZ4 decompresses at gigabytes per second; zstd trades some speed for a better ratio. Either fits
inside a chunk load.

This needs a v4 format: a compression flag and length in the header, a decompressor on load, and a
guard for corrupt streams. It also makes the modified-chunk working set much smaller, which matters
for backups and cloud sync.

### 4. Region files and async saves

`world/chunks/` holds one file per modified chunk. At distance 24 that is only the edited set, so
filesystem pressure is low today, but a builder session can produce thousands of files. A region
file (for example 32x32 chunks per file with an offset table) reduces inode and directory pressure
and makes compression block-local.

Saving currently runs on the main thread: an unload writes four files, and `Ctrl+S` rewrites every
modified chunk. On a slow or synced disk that is a frame hitch. A single I/O thread with a queue,
plus atomic write-to-temp-then-rename, keeps the frame clean and the save crash-safe.

### 5. Shrink light scratch

The 96^3 propagation grid is `u8` per cell for blocks, sky, and block light. Two cheap steps:

- Reuse one scratch buffer per worker thread instead of allocating 2.65 MB per job.
- Store block presence as a bit (solid/transmissive) rather than a full `Block`, or pack sky/block
  light into one nibble array during propagation.

The first is pure allocation hygiene and should be done regardless. The second lowers the working
set per job by several times and improves cache behaviour during the BFS.

### 6. Allocator choice

The engine allocates a 32 KiB array per chunk and a multi-megabyte grid per light job, from eleven
worker threads. A mimalloc or rpmalloc global allocator tends to beat the system allocator in
multithreaded allocation-heavy workloads. It is a one-line experiment behind a feature or a
measured A/B, with no API change.

## Recommendation

1. Packed, indexed vertices. Done in v0.0.8.
2. Nibble block IDs. Simple, contained, and halves the block array.
3. Reuse light scratch per worker. Least effort, removes hidden spikes.
4. LZ4 `VCHK` v4 with atomic writes on an I/O thread. Smaller saves and no frame stalls.
5. Palette-packed chunks and region files only if distance 32+ becomes routine.

## Validation

- Track working set and per-chunk bytes at distances 12 and 24 with the same seed and camera.
- Add a headless round-trip test for any `VCHK` change, including an empty modified chunk.
- Assert mesh vertex counts and index counts stay within the chosen index width.
- Record the results in [Benchmark results](../benchmarks.md).

## References

- [Minecraft chunk format: paletted containers](https://minecraft.wiki/w/Java_Edition_protocol/Chunk_format) — bit-packed indices and per-section palettes.
- [LZ4 vs zstd](https://news.ycombinator.com/item?id=23213447) and [compression benchmarks](https://dev.to/konstantinas_mamonas/which-compression-saves-the-most-storage-gzip-snappy-lz4-zstd-1898) — ratio versus decompression speed.
- [Post-transform cache](https://www.opengl.org/wiki/Post_Transform_Cache) — why indexed triangles save vertex shading.
- [Nick McDonald: vertex pooling for greedy meshing](https://nickmcd.me/2021/04/04/high-performance-voxel-engine/) — one vertex pool per face orientation.
