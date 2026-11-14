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

## Conventions

- **Numbering.** Four digits, sequential, never reused. The next decision is the highest number plus one.
- **Filenames.** `NNNN-short-slug.md`, where the slug names the decision, not the act: `0005-bytemuck-for-gpu-layout.md`, not `0005-i-chose-bytemuck.md`.
- **Never rewrite history.** An accepted decision is superseded, not edited into agreement with a later choice. Update its `Status` and `Supersedes` lines, then write the new record.
- **One decision per file.** If a document needs two "I chose" statements, it is two records.
- **Start from the template.** Copy `0000-template.md`, fill it in, and add a row to the table above.
- **Cross-reference by ID.** Refer to "ADR-0002" rather than repeating its reasoning.
