# Decision records

One file per decision. Each record states the context, the decision, the alternatives, and the counterarguments, so a later reader can tell why the engine looks the way it does.

| ID | Decision | Status |
|---|---|---|
| [0001](0001-rust-for-engine-core.md) | Rust for the engine core | Accepted |
| [0002](0002-wgpu-for-graphics.md) | wgpu for graphics | Accepted |
| [0003](0003-winit-for-windowing.md) | winit for windowing | Accepted |
| [0004](0004-glam-for-math.md) | glam for math | Accepted |
| [0005](0005-bytemuck-for-gpu-layout.md) | bytemuck for GPU data layout | Accepted |
| [0006](0006-tracy-for-profiling.md) | Tracy for profiling | Accepted |
| [0007](0007-texture-array-for-block-textures.md) | Texture array for block textures | Accepted |
| [0008](0008-chunk-save-format.md) | Chunk save format (VCHK) | Accepted |
| [0009](0009-micropool-for-chunk-workers.md) | micropool for chunk worker jobs | Accepted |
| [0010](0010-two-level-lod-for-chunks.md) | Two LOD levels with conservative downsampling | Accepted; extended by v0.0.8 LOD2 |
| [0011](0011-voxel-light-and-baked-ao.md) | Voxel lighting with baked AO instead of shadows | Accepted |
| [0012](0012-fastnoise-lite-and-opensimplex2.md) | fastnoise-lite with OpenSimplex2 | Accepted |
| [0013](0013-player-centred-chunk-streaming.md) | Finite player-centred chunk streaming | Accepted |
| [0014](0014-seed-native-lod-hierarchy.md) | Seed-native multi-level LOD sections | Accepted |
| [0015](0015-texture-references-are-presentation.md) | Per-face texture references are presentation-only | Accepted |
| [0016](0016-block-texture-mipmap-policy.md) | Block texture mipmap and sampling policy | Accepted |
| [0017](0017-quad-pool-and-indirect-draws.md) | Quad pool with vertex pulling and indirect draws | Accepted; implemented in v0.0.9 slice 4 |
| [0018](0018-egui-for-game-ui.md) | egui for the gameplay UI | Accepted; scoped for v0.0.10 |
| [0019](0019-kira-for-game-audio.md) | kira with procedural placeholders for audio | Accepted; scoped for v0.0.10 |
| [0020](0020-minimal-entity-runtime.md) | Minimal entity runtime instead of an ECS | Accepted; scoped for v0.0.10 |

## Conventions

- **Numbering.** Four digits, sequential, never reused. The next decision is the highest number plus one.
- **Filenames.** `NNNN-short-slug.md`, where the slug names the decision, not the act: `0005-bytemuck-for-gpu-layout.md`, not `0005-i-chose-bytemuck.md`.
- **Never rewrite history.** An accepted decision is superseded, not edited into agreement with a later choice. Update its `Status` and `Supersedes` lines, then write the new record.
- **One decision per file.** If a document needs two "I chose" statements, it is two records.
- **Start from the template.** Copy `0000-template.md`, fill it in, and add a row to the table above.
- **Cross-reference by ID.** Refer to "ADR-0002" rather than repeating its reasoning.
