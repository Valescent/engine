//! The frozen v0 voxel stack.
//!
//! v0 is the original chunk engine: a dense `32³` chunk map with `VCHK`
//! persistence, BFS sky/block light, a greedy mesher with LOD1/LOD2
//! downsamples, seeded generation, and player-centred chunk streaming. It is
//! still the default run path when `TERRAIN_DEMO` is unset.
//!
//! This stack is **frozen**. It receives fixes only when they keep the legacy
//! path working; new features belong to the terrain stack and the terrain demo.
//! The whole module is removed once the terrain demo becomes the default run
//! path.
//!
//! The crate root re-exports these modules, so `crate::world`,
//! `crate::mesher`, and `engine::world` keep resolving to `crate::v0::world`
//! and friends.

pub mod generation;
pub mod light;
pub mod mesh;
pub mod mesher;
pub mod streaming;
pub mod voxel;
pub mod world;
