//! Compile-time proof that the terrain stack is reachable only through
//! `engine::terrain`.
//!
//! The `compile_fail` doctests on [`engine::terrain`] prove that the flat
//! `engine::<module>` paths are crate-internal. This test proves the supported
//! facade paths resolve from an external crate.

use engine::terrain::error::TerrainError;
use engine::terrain::light2::{self, LightTable};
use engine::terrain::pipeline::Terrain;
use engine::terrain::section::Section;

// These signatures resolve only if `engine::terrain` re-exports the
// `terrain-core` modules and types.
const _: fn(u64) -> Result<Terrain, TerrainError> = Terrain::new;
const _: fn(u16) -> Section = Section::uniform;
const _: fn() -> LightTable = light2::default_table;

#[test]
fn facade_paths_resolve() {
    assert!(std::mem::size_of::<Terrain>() > 0);
    assert!(std::mem::size_of::<Section>() > 0);
    assert!(std::mem::size_of::<LightTable>() > 0);
}
