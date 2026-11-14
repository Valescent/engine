# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.1] - 2026-11-14

### Added

- Repository scaffold: Cargo manifest, roadmap, architecture decision records, and PolyForm Shield 1.0.0 license.
- Conventional Commits and changelog enforcement in continuous integration.
- Release automation: one command bumps the version, tags it, and publishes a GitHub release.
- Documentation site built with mdBook and published to GitHub Pages.
- v0.0.1 foundation: a `winit` window with a `wgpu` renderer, depth buffering, and a static perspective camera.
- Voxel scene stored in a `HashSet<IVec3>` and rebuilt as a single mesh on change.
- `Space` toggles the raised centre voxel at runtime.
- Fly camera: WASD movement, mouse look with cursor capture, delta-time updates, and frame-time logging.
