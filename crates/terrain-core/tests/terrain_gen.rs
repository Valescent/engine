//! Integration tests for the `terrain-gen` acceptance harness.
//!
//! Each test drives the real binary, not the library, so it also covers argument
//! parsing, exit codes, and the stable stdout the CI gates read.

#[path = "terrain_gen/calibrate.rs"]
mod calibrate;
#[path = "terrain_gen/counters.rs"]
mod counters;
#[path = "terrain_gen/gen.rs"]
mod r#gen;
#[path = "terrain_gen/golden.rs"]
mod golden;
#[path = "terrain_gen/map.rs"]
mod map;
#[path = "terrain_gen/migrate.rs"]
mod migrate;
#[path = "terrain_gen/record.rs"]
mod record;
#[path = "terrain_gen/repair.rs"]
mod repair;
#[path = "terrain_gen/replay.rs"]
mod replay;
#[path = "terrain_gen/support.rs"]
mod support;
#[path = "terrain_gen/verify.rs"]
mod verify;
