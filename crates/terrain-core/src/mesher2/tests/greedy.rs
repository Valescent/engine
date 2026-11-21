//! Greedy meshing: faces, merging, AO, water, cutout, and cross quads.

use super::*;
use crate::description::SECTION_SIZE;
use crate::padded::{LOCAL_MAX, PaddedBlocks};
use crate::section::{BlockId, Section};

#[test]
fn single_block_emits_six_faces() {
    let section = section_with(&[([5, 5, 5], 1)]);
    let blocks = padded(&section);
    let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);

    assert_eq!(result.opaque.len(), 6);
    assert_eq!(result.cutout.len(), 0);
    assert_eq!(result.water.len(), 0);
}

#[test]
fn faces_take_the_texture_of_their_own_direction() {
    let mut props = MaterialProps::new();
    props.set(
        BlockId::from(1_u16),
        Material {
            texture_top: 20,
            texture_side: 21,
            texture_bottom: 22,
            ..Material::UNKNOWN
        },
    );
    let section = section_with(&[([5, 5, 5], BlockId::from(1_u16))]);
    let blocks = padded(&section);
    let result = mesh(&section, &blocks, &lit(), &props, MeshLod::Lod0);

    let expected = |normal: u8| match normal {
        NORMAL_POS_Y => 20,
        NORMAL_NEG_Y => 22,
        _ => 21,
    };
    for quad in &result.opaque {
        assert_eq!(
            quad.tile(),
            expected(quad.normal()),
            "normal {} must use its own face texture",
            quad.normal()
        );
    }
    assert_eq!(result.opaque.len(), 6);
}

#[test]
fn cross_quads_use_the_side_texture() {
    let mut props = MaterialProps::new();
    props.set(
        BlockId::from(9_u16),
        Material {
            cross_quad: true,
            texture_top: 20,
            texture_side: 21,
            texture_bottom: 22,
            ..Material::UNKNOWN
        },
    );
    let section = section_with(&[([5, 5, 5], BlockId::from(9_u16))]);
    let blocks = padded(&section);
    let result = mesh(&section, &blocks, &lit(), &props, MeshLod::Lod0);

    assert_eq!(result.cutout.len(), 2);
    for quad in &result.cutout {
        assert_eq!(quad.tile(), 21);
    }
}

#[test]
fn flat_ground_merges_into_one_quad() {
    let section = slab();
    let blocks = padded(&section);
    let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);

    assert_eq!(count_normal(&result.opaque, NORMAL_POS_Y), 1);
    let top = result
        .opaque
        .iter()
        .find(|quad| quad.normal() == NORMAL_POS_Y)
        .expect("the slab has a top face");
    assert_eq!((top.x(), top.y(), top.z()), (0, 0, 0));
    assert_eq!((top.w1(), top.h1()), (31, 31));
}

#[test]
fn light_difference_breaks_merging() {
    let section = slab();
    let blocks = padded(&section);
    let mut light = lit();
    light.set(0, 1, 0, 7, 0);
    let result = mesh(&section, &blocks, &light, &props(), MeshLod::Lod0);

    // A single dark cell in a uniform plane is a rectangular hole, so the
    // rest of the plane cannot be one rectangle: greedy produces the hole
    // plus two rectangles. The merge still breaks, which is the property
    // M1.1 asks for.
    assert_eq!(count_normal(&result.opaque, NORMAL_POS_Y), 3);
    let skies: Vec<u8> = result
        .opaque
        .iter()
        .filter(|quad| quad.normal() == NORMAL_POS_Y)
        .map(|quad| quad.sky())
        .collect();
    assert!(skies.contains(&7));
    assert!(skies.contains(&15));
}

#[test]
fn ao_difference_breaks_merging() {
    let section = slab();
    let mut blocks = padded(&section);
    for x in -1..=LOCAL_MAX {
        blocks.set(x, 1, -1, 1);
    }
    let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);

    let tops: Vec<&Quad> = result
        .opaque
        .iter()
        .filter(|quad| quad.normal() == NORMAL_POS_Y)
        .collect();
    assert_eq!(
        tops.len(),
        2,
        "the dark edge row must not merge with the rest"
    );
    assert_ne!(tops[0].ao(), tops[1].ao(), "the wall darkens one corner");
}

#[test]
fn corner_ao_reads_the_diagonal_neighbour() {
    let stone = 1;
    let section = section_with(&[([0, 31, 0], stone)]);
    let air = Section::uniform(0);
    let corner = section_with(&[([31, 0, 31], stone)]);
    let mut owners: [Option<&Section>; 26] = [Some(&air); 26];
    owners[23] = Some(&corner);

    let blocks = PaddedBlocks::from_sections(&section, &owners);
    let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
    let top = result
        .opaque
        .iter()
        .find(|quad| quad.normal() == NORMAL_POS_Y)
        .expect("the block has a top face");

    // Corner 0 of a `+Y` face is at local `(-1, 32, -1)`, inside the
    // `(-X, +Y, -Z)` corner section. One occluded diagonal is AO 2.
    assert_eq!(top.ao() & 0x03, 2, "the corner section darkens the corner");
    assert_eq!(top.ao(), 254, "the three other corners stay open");
}

#[test]
fn six_face_neighbours_match_the_padded_path_without_diagonals() {
    // A slab whose AO samples never touch a diagonal section: every
    // face-neighbour result must be byte-identical to the 26-neighbour
    // result with explicit air edges and corners.
    let section = slab();
    let air = Section::uniform(0);
    let faces = [Some(&air); 6];
    let material = props();
    let light = lit();

    let six = mesh_from_face_neighbours(&section, &faces, &light, &material, MeshLod::Lod0);
    let padded_all = mesh(
        &section,
        &padded(&section),
        &light,
        &material,
        MeshLod::Lod0,
    );
    assert_eq!(
        raw(&six),
        raw(&padded_all),
        "the six-neighbour path is unchanged where diagonals cannot matter"
    );
}

#[test]
fn diagonal_neighbour_darkens_a_corner_but_a_face_neighbour_does_not() {
    let stone = 1;
    let section = section_with(&[([0, 31, 0], stone)]);
    let air = Section::uniform(0);
    let material = props();
    let light = lit();

    // With no diagonal data the corner is fully open.
    let faces = [Some(&air); 6];
    let open = mesh_from_face_neighbours(&section, &faces, &light, &material, MeshLod::Lod0);
    let open_top = top_quad(&open, (0, 31, 0));
    assert_eq!(
        corner(open_top.ao(), 0),
        3,
        "a corner with no diagonal block is open"
    );

    // A block sharing the `-Z` face of the source sits below the `+Y` AO
    // sample plane, so it culls a side face but cannot darken corner 0.
    let face_neighbour = section_with(&[([0, 31, 31], stone)]);
    let mut owners: [Option<&Section>; 26] = [Some(&air); 26];
    owners[5] = Some(&face_neighbour);
    let side_probe = mesh_from_neighbours(&section, &owners, &light, &material, MeshLod::Lod0);
    let probe_top = top_quad(&side_probe, (0, 31, 0));
    assert_eq!(
        corner(probe_top.ao(), 0),
        3,
        "a face-adjacent block does not darken the diagonal corner"
    );
    assert_eq!(
        side_probe.opaque.len(),
        5,
        "the face-adjacent block culls the shared side face"
    );

    // One opaque block in the `(-X, +Y, -Z)` corner section darkens only
    // corner 0: the diagonal is AO 2.
    let diagonal = section_with(&[([31, 0, 31], stone)]);
    let mut owners: [Option<&Section>; 26] = [Some(&air); 26];
    owners[23] = Some(&diagonal);
    let dark = mesh_from_neighbours(&section, &owners, &light, &material, MeshLod::Lod0);
    let dark_top = top_quad(&dark, (0, 31, 0));
    assert_eq!(corner(dark_top.ao(), 0), 2, "one diagonal is AO 2");
    for index in 1..4 {
        assert_eq!(
            corner(dark_top.ao(), index),
            3,
            "the other corners stay open"
        );
    }
}

#[test]
fn only_opaque_full_cubes_occlude_ao() {
    let stone = 1;
    let section = section_with(&[([5, 5, 5], stone)]);
    let base = padded(&section);
    let material = props();
    let light = lit();

    for (block, why) in [(7, "water"), (5, "cutout leaves"), (3, "cross quads")] {
        let mut blocks = base.clone();
        // Corner 0 of the `+Y` face samples this cell.
        blocks.set(4, 6, 4, block);
        let result = mesh(&section, &blocks, &light, &material, MeshLod::Lod0);
        let top = top_quad(&result, (5, 5, 5));
        assert_eq!(corner(top.ao(), 0), 3, "{why} must not occlude a corner");
    }

    let mut blocks = base;
    blocks.set(4, 6, 4, stone);
    let result = mesh(&section, &blocks, &light, &material, MeshLod::Lod0);
    let top = top_quad(&result, (5, 5, 5));
    assert_eq!(corner(top.ao(), 0), 2, "a full opaque cube is AO 2");
}

#[test]
fn empty_sections_mesh_to_nothing() {
    let section = Section::uniform(0);
    let air = Section::uniform(0);
    let faces = [Some(&air); 6];
    let material = props();
    let light = lit();

    let from_faces = mesh_from_face_neighbours(&section, &faces, &light, &material, MeshLod::Lod0);
    let owners: [Option<&Section>; 26] = [Some(&air); 26];
    let from_all = mesh_from_neighbours(&section, &owners, &light, &material, MeshLod::Lod0);
    assert_eq!(quad_count(&from_faces), 0);
    assert_eq!(quad_count(&from_all), 0);
}

#[test]
fn water_emits_only_against_air() {
    let water = 7;
    let stone = 1;

    let section = section_with(&[([0, 0, 0], water), ([0, 1, 0], stone), ([1, 0, 0], stone)]);
    let blocks = padded(&section);
    let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
    assert!(
        !result
            .water
            .iter()
            .any(|quad| quad.normal() == NORMAL_POS_X || quad.normal() == NORMAL_POS_Y),
        "water must not emit against stone"
    );

    let pair = section_with(&[([0, 0, 0], water), ([0, 0, 1], water)]);
    let pair_blocks = padded(&pair);
    let pair_mesh = mesh(&pair, &pair_blocks, &lit(), &props(), MeshLod::Lod0);
    assert!(
        !pair_mesh.water.iter().any(|quad| {
            (quad.normal() == NORMAL_POS_Z && quad.x() == 0 && quad.y() == 0 && quad.z() == 0)
                || (quad.normal() == NORMAL_NEG_Z
                    && quad.x() == 0
                    && quad.y() == 0
                    && quad.z() == 1)
        }),
        "water/water emits no face on the shared plane"
    );

    let mut ocean = Section::uniform(0);
    for x in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            ocean.set(x, 0, z, water);
        }
    }
    let mut ocean_blocks = padded(&ocean);
    for x in -1..=LOCAL_MAX {
        for z in -1..=LOCAL_MAX {
            ocean_blocks.set(x, -1, z, water);
        }
    }
    for z in -1..=LOCAL_MAX {
        ocean_blocks.set(-1, 0, z, water);
        ocean_blocks.set(LOCAL_MAX, 0, z, water);
    }
    for x in -1..=LOCAL_MAX {
        ocean_blocks.set(x, 0, -1, water);
        ocean_blocks.set(x, 0, LOCAL_MAX, water);
    }
    let ocean_mesh = mesh(&ocean, &ocean_blocks, &lit(), &props(), MeshLod::Lod0);
    assert_eq!(ocean_mesh.water.len(), 1, "a full ocean top is one quad");
    let top = ocean_mesh.water[0];
    assert_eq!(top.normal(), NORMAL_POS_Y);
    assert_eq!((top.w1(), top.h1()), (31, 31));
}

#[test]
fn cutout_is_never_merged() {
    let leaves = 5;
    let section = section_with(&[
        ([0, 0, 0], leaves),
        ([1, 0, 0], leaves),
        ([2, 0, 0], leaves),
        ([3, 0, 0], leaves),
    ]);
    let blocks = padded(&section);
    let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);

    assert_eq!(result.opaque.len(), 0);
    assert_eq!(count_normal(&result.cutout, NORMAL_POS_Y), 4);
    let mut tops: Vec<Quad> = result
        .cutout
        .iter()
        .copied()
        .filter(|quad| quad.normal() == NORMAL_POS_Y)
        .collect();
    assert_eq!(tops.len(), 4, "each leaf keeps its own record");
    tops.sort_by_key(|quad| quad.bits());
    tops.dedup();
    assert_eq!(tops.len(), 4, "records differ by origin");
    assert!(
        result
            .cutout
            .iter()
            .all(|quad| quad.w1() == 0 && quad.h1() == 0),
        "cutout faces are never merged"
    );
}

#[test]
fn cross_quads_are_two_quads_and_never_occlude() {
    let grass = 3;
    let stone = 1;
    let section = section_with(&[([0, 0, 0], grass), ([1, 0, 0], stone)]);
    let blocks = padded(&section);
    let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);

    assert_eq!(result.cutout.len(), 2, "one cross quad per diagonal");
    assert!(
        result
            .cutout
            .iter()
            .any(|quad| quad.normal() == NORMAL_CROSS_A)
    );
    assert!(
        result
            .cutout
            .iter()
            .any(|quad| quad.normal() == NORMAL_CROSS_B)
    );
    assert!(
        !result
            .opaque
            .iter()
            .any(|quad| quad.x() == 0 && quad.y() == 0 && quad.z() == 0),
        "grass emits no opaque record"
    );
    assert_eq!(
        count_normal(&result.opaque, NORMAL_NEG_X),
        1,
        "grass does not cull the neighbour's face"
    );
}

#[test]
fn no_face_between_two_opaque_blocks() {
    let stone = 1;
    let section = section_with(&[([0, 0, 0], stone), ([1, 0, 0], stone)]);
    let blocks = padded(&section);
    let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);

    // A 2x1x1 box has six coplanar outer faces, so greedy merging produces
    // one quad per box face rather than the naive ten per-block faces. The
    // shared face is still culled, which is the property this test pins.
    assert_eq!(result.opaque.len(), 6, "one merged quad per box face");
    assert!(
        !result
            .opaque
            .iter()
            .any(|quad| quad.normal() == NORMAL_POS_X && quad.x() == 0),
        "the +X face of the left block is inside the pair"
    );
    assert!(
        !result
            .opaque
            .iter()
            .any(|quad| quad.normal() == NORMAL_NEG_X && quad.x() == 1),
        "the -X face of the right block is inside the pair"
    );
}
