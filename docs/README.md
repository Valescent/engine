# Valescent Engine Documentation

Index and conventions for this folder. Keep this page current — it is the entry point when the docs folder grows past a handful of files.

## Layout

| Path | Contents |
|---|---|
| `roadmap.md` | Versioned milestones and their status. |
| `decisions/` | Architecture Decision Records. One file per decision, numbered and immutable once accepted. |
| `dev/` | Development setup and code conventions. |

Add new top-level folders only when a real document needs them, for example `guides/` for how-to material or `specs/` for formats and protocols. Do not create empty placeholders.

## Decision records

| ID | Decision | Status |
|---|---|---|
| [0001](decisions/0001-rust-for-engine-core.md) | Rust for the engine core | Accepted |
| [0002](decisions/0002-wgpu-for-graphics.md) | wgpu for graphics | Accepted |
| [0003](decisions/0003-winit-for-windowing.md) | winit for windowing | Accepted |
| [0004](decisions/0004-glam-for-math.md) | glam for math | Accepted |
| [0005](decisions/0005-bytemuck-for-gpu-layout.md) | bytemuck for GPU data layout | Accepted |

## Conventions

- **Numbering.** Four digits, sequential, never reused. The next decision is the highest number plus one.
- **Filenames.** `NNNN-short-slug.md`, where the slug names the decision, not the act: `0005-bytemuck-for-gpu-layout.md`, not `0005-i-chose-bytemuck.md`.
- **Never rewrite history.** An accepted decision is superseded, not edited into agreement with a later choice. Update its `Status` and `Supersedes` lines, then write the new record.
- **One decision per file.** If a document needs two "I chose" statements, it is two records.
- **Start from the template.** Copy `decisions/0000-template.md`, fill it in, and add a row to the table above.
- **Cross-reference by ID.** Refer to "ADR-0002" rather than repeating its reasoning.
- **Roadmap stays flat.** Milestones live in one `roadmap.md`; per-version files are not worth the navigation cost until the file becomes unmanageable.
