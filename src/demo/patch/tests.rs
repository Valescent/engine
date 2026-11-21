//! Tests for the resident demo patch.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{IVec2, IVec3, Vec3};

use crate::description;
use crate::hydro;
use crate::mesher2;
use crate::player::BlockSource;
use crate::raycast::raycast_where;
use crate::renderer;

use super::super::persist::build_terrain_demo;
use super::{DemoSection, TerrainDemo};

#[test]
fn demo_raycast_hits_the_spawn_surface() {
    let demo = build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
    let origin = demo.spawn + Vec3::Y * 20.0;
    let hit = raycast_where(|cell| demo.is_solid(cell), origin, -Vec3::Y, 64.0)
        .expect("a downward ray hits the patch");
    #[allow(clippy::cast_possible_truncation)]
    let expected = demo.spawn.y.round() as i32 - 1;
    assert_eq!(hit.block.y, expected);
    assert_eq!(hit.normal, IVec3::Y);
}

#[test]
#[allow(clippy::cast_possible_truncation)]
fn demo_edit_breaks_a_block_and_relights() {
    let mut demo = build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
    let cell = IVec3::new(
        demo.spawn.x.floor() as i32,
        demo.spawn.y.round() as i32 - 1,
        demo.spawn.z.floor() as i32,
    );
    let air = demo.air;
    assert_ne!(demo.block_at(cell), Some(air), "the spawn block is solid");
    assert!(demo.edit(cell, air), "the break changes the patch");
    assert_eq!(demo.block_at(cell), Some(air));
    assert!(!demo.edit(cell, air), "a no-op edit reports no change");
    assert!(demo.quads > 0, "the patch still meshes after the edit");
}

#[test]
#[allow(clippy::cast_possible_truncation)]
fn terrain_demo_builds_packed_quads_for_one_column() {
    let started = Instant::now();
    let demo = build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
    let elapsed = started.elapsed();
    eprintln!(
        "terrain demo: {} sections, {} quads, {elapsed:?}",
        demo.sections.len(),
        demo.quads
    );

    assert!(
        demo.sections
            .iter()
            .any(|section| !section.mesh.opaque.is_empty()),
        "at least one section emits opaque quads"
    );
    assert_eq!(demo.origin_column, IVec2::new(2, -3));

    // Collision is live: the block under the spawn is solid and the spawn
    // sits exactly one block above it.
    let feet = IVec3::new(
        demo.spawn.x.floor() as i32,
        demo.spawn.y.round() as i32 - 1,
        demo.spawn.z.floor() as i32,
    );
    let solid = demo.solid_blocks_in(feet, feet);
    assert_eq!(
        solid,
        vec![feet],
        "the spawn column stands on solid terrain"
    );
    for section in &demo.sections {
        for coordinate in section.origin {
            assert_eq!(
                coordinate.rem_euclid(i32::try_from(description::SECTION_SIZE).unwrap()),
                0,
                "origins are section-aligned multiples of 32"
            );
        }
        for raw in section
            .mesh
            .opaque
            .iter()
            .chain(&section.mesh.cutout)
            .chain(&section.mesh.water)
        {
            let quad = renderer::decode_quad(raw.bits());
            assert!(quad.normal <= 7, "reserved quad normal code");
        }
    }
    // A gross-regression guard, not a benchmark: the suite runs tests in
    // parallel on shared cores, and one column also pays the first
    // whole-world hydrology solve on this thread.
    assert!(
        elapsed < Duration::from_secs(10),
        "one-column demo build took {elapsed:?}"
    );
}

fn stub_section(origin: [i32; 3], scale: u32) -> DemoSection {
    DemoSection {
        origin,
        scale,
        mesh: mesher2::SectionMesh::default(),
    }
}

#[test]
fn select_lod_keeps_the_finer_band_on_a_stale_shared_origin() {
    let hydro = Arc::new(hydro::WorldHydro::solve(&[0.0], 1).expect("world hydro"));
    let mut demo = TerrainDemo::empty(0, IVec2::ZERO, 12, hydro);
    demo.fine.insert((0, 0), vec![stub_section([0, 0, 0], 1)]);
    demo.coarse = vec![stub_section([0, 0, 0], 2)];
    // A stale clipped mask that no longer names the base column keeps this
    // LOD1 section active even though its origin is now inside the fine band.
    demo.partial_columns
        .insert((0, 0), BTreeSet::from([(4, 0)]));
    demo.select_lod((0, 0));

    let at_origin: Vec<&DemoSection> = demo
        .sections
        .iter()
        .filter(|section| section.origin == [0, 0, 0])
        .collect();
    assert_eq!(at_origin.len(), 1, "one section per origin");
    assert_eq!(
        at_origin[0].scale, 1,
        "the finer band keeps the shared origin"
    );
}
