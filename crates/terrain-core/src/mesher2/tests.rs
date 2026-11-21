//! Shared fixtures for the mesher tests. Test cases live in the `greedy`,
//! `lod`, `record`, and `skirt` submodules.

mod greedy;
mod lod;
mod record;
mod skirt;

use super::*;
use crate::description::SECTION_SIZE;
use crate::padded::{PaddedBlocks, PaddedLight};
use crate::section::{BlockId, Section};

/// A section holding the given sparse cells in air.
fn section_with(cells: &[([u32; 3], BlockId)]) -> Section {
    let mut section = Section::uniform(0);
    for (position, block) in cells {
        section.set(position[0], position[1], position[2], *block);
    }
    section
}

/// A padded copy of a section with air neighbours.
///
/// Tests use explicit air sections rather than `None`: a missing neighbour
/// mirrors the edge row (the world-edge rule), which is not what an
/// interior section with unresolvable neighbours should read.
fn padded(section: &Section) -> PaddedBlocks {
    let air = Section::uniform(0);
    PaddedBlocks::from_sections(section, &[Some(&air); 26])
}

/// A 32x32 stone slab at `y = 0`.
fn slab() -> Section {
    let mut section = Section::uniform(0);
    for x in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            section.set(x, 0, z, 1);
        }
    }
    section
}

/// Full light.
fn lit() -> PaddedLight {
    PaddedLight::new(15, 0)
}

/// The default material table, asserting it resolves the stable ids.
fn props() -> MaterialProps {
    let props = MaterialProps::default_table();
    assert_eq!(props.material(0), Material::AIR);
    assert!(props.material(1).opaque);
    assert!(props.material(3).cross_quad);
    assert!(props.material(5).solid);
    assert!(!props.material(5).opaque);
    assert!(props.material(7).water);
    assert!(props.material(8).opaque);
    props
}

/// Every record from all three ranges, in output order.
fn raw(mesh: &SectionMesh) -> Vec<u64> {
    mesh.opaque
        .iter()
        .chain(&mesh.cutout)
        .chain(&mesh.water)
        .map(|quad| quad.bits())
        .collect()
}

/// The quads the last [`add_skirts`] call appended.
fn added_skirts(mesh: &SectionMesh, before: usize) -> &[Quad] {
    &mesh.opaque[before..]
}

/// A 32x32 stone cap on the top cell layer of a section.
fn cap() -> Section {
    let mut section = Section::uniform(0);
    for x in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            section.set(x, SECTION_SIZE - 1, z, 1);
        }
    }
    section
}

/// The deterministic skirt fixture: a top cap with all four skirts.
fn skirted_cap() -> SectionMesh {
    let section = cap();
    let blocks = padded(&section);
    let mut mesh = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
    assert_eq!(
        add_skirts(&mut mesh, SkirtEdges::ALL, MeshLod::Lod0),
        8,
        "the cap has four skirted sides at SKIRT_DEPTH cells each"
    );
    mesh
}

/// A top-face count for the face code.
fn count_normal(quads: &[Quad], normal: u8) -> usize {
    quads.iter().filter(|quad| quad.normal() == normal).count()
}

/// The single `+Y` quad with the given origin.
fn top_quad(mesh: &SectionMesh, at: (u8, u8, u8)) -> Quad {
    mesh.opaque
        .iter()
        .copied()
        .find(|quad| quad.normal() == NORMAL_POS_Y && (quad.x(), quad.y(), quad.z()) == at)
        .expect("the fixture has a top face at the requested cell")
}

/// The AO value of one packed corner.
fn corner(ao: u8, index: u32) -> u8 {
    (ao >> (2 * index)) & 0b11
}
