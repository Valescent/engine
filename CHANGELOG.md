# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Chunked world storage: 32×32×32 chunks, Euclidean chunk coordinates, and dirty-chunk tracking.
- Face culling through world coordinates, including across chunk borders.
- One GPU vertex buffer and one draw call per non-empty chunk, with a per-chunk origin uniform.
- Debug logging for dirty chunk count, total vertices, and rebuild time after an edit.

### Changed

- The demo scene is a 4×4 floor spanning four chunks with a raised voxel on the chunk boundary.
- `Space` toggles that boundary voxel and rebuilds only the affected chunks.
- The camera starts near `(37, 4.5, 38)` and is aimed at `(32, 0.5, 32)`.

### Fixed

- Chunk meshing treats neighbours outside the `i32` world bounds as air instead of overflowing.

## [0.0.2] - 2026-11-14

### Added

- Fly camera: WASD movement, mouse look with cursor capture, delta-time updates, and frame-time logging.

## [0.0.1] - 2026-11-14

### Added

- Repository scaffold: Cargo manifest, roadmap, architecture decision records, and PolyForm Shield 1.0.0 license.
- Conventional Commits and changelog enforcement in continuous integration.
- Release automation: one command bumps the version, tags it, and publishes a GitHub release.
- Documentation site built with mdBook and published to GitHub Pages.
- v0.0.1 foundation: a `winit` window with a `wgpu` renderer, depth buffering, and a static perspective camera.
- Voxel scene stored in a `HashSet<IVec3>` and rebuilt as a single mesh on change.
- `Space` toggles the raised centre voxel at runtime.
