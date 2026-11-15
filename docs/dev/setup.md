# Development setup

## Toolchain

`rust-toolchain.toml` pins Rust 1.99.1 with `rustfmt` and `clippy`. With [rustup](https://rustup.rs) installed, `cargo` in this directory uses that toolchain automatically and installs it on first use.

## Platform prerequisites

### Windows

- Visual Studio Build Tools with the **Desktop development with C++** workload (or Visual Studio with it selected). The MSVC linker is the only requirement.
- A GPU driver with Vulkan or Direct3D 12 support. Update it before filing rendering bugs.
- Use PowerShell or Git Bash. All tooling in this repository is POSIX `sh` and runs under Git Bash.

### Linux

A windowing backend and its development headers are needed by `winit`. On Debian/Ubuntu:

```console
sudo apt install build-essential pkg-config libxkbcommon-dev libwayland-dev \
  libx11-xcb-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
```

On Fedora:

```console
sudo dnf install gcc pkg-config libxkbcommon-devel wayland-devel \
  libxcb-devel libX11-devel
```

### macOS

Install the Xcode command-line tools:

```console
xcode-select --install
```

## Daily commands

```console
cargo run
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all
```

## Commit hook

Enable once per clone:

```console
git config core.hooksPath .githooks
```

The hook validates the commit message. It uses `cog` when available and a built-in check otherwise; install the full validator with:

```console
cargo install cocogitto --locked
```

CI enforces the same rule, so the hook is a convenience rather than the gate.

## Graphics backend

`wgpu` selects a backend automatically. To force one:

```console
# Windows
$env:WGPU_BACKEND = "dx12"   # or "vulkan"
# Linux / macOS
WGPU_BACKEND=vulkan cargo run
```

Bugs that reproduce on one backend and not another are worth reporting with the backend named.

## Releases

Releases are cut from GitHub Actions: **Actions → Release → Run workflow** with a version. The workflow runs `scripts/release.sh`, which rolls `[Unreleased]` in `CHANGELOG.md`, bumps the manifest, commits, tags, and pushes; the tag then publishes a GitHub release. A dry run is available and pushes nothing.

The same script works locally:

```console
sh scripts/release.sh 0.0.3 --dry-run
```
