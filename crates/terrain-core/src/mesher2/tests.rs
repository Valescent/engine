//! Tests for every mesher range and skirt path.

use super::*;
use crate::description::{SECTION_SIZE, SECTION_VOLUME};
use crate::lod::SKIRT_DEPTH;
use crate::padded::{LOCAL_MAX, LOCAL_SIZE, PaddedBlocks, PaddedLight};
use crate::revisions::Face as ColumnFace;
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

#[test]
fn record_layout_is_64_bits() {
    let zero = Quad::encode(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
    assert_eq!(zero.bits(), 0);
    assert_eq!(
        (
            zero.x(),
            zero.y(),
            zero.z(),
            zero.w1(),
            zero.h1(),
            zero.normal(),
            zero.ao(),
            zero.sky(),
            zero.blk(),
            zero.tile(),
            zero.tint(),
        ),
        (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
    );

    let max = Quad::encode(31, 31, 31, 31, 31, 7, 255, 15, 15, u16::MAX, 15);
    assert_eq!(
        max.bits(),
        u64::MAX,
        "every bit must belong to a field; the encoder masks nothing away"
    );
    assert_eq!(
        (
            max.x(),
            max.y(),
            max.z(),
            max.w1(),
            max.h1(),
            max.normal(),
            max.ao(),
            max.sky(),
            max.blk(),
            max.tile(),
            max.tint(),
        ),
        (31, 31, 31, 31, 31, 7, 255, 15, 15, u16::MAX, 15)
    );
}

#[test]
fn coordinates_round_trip_over_the_whole_section() {
    for x in 0..32_u8 {
        for y in 0..32_u8 {
            for z in 0..32_u8 {
                let quad = Quad::encode(x, y, z, 0, 0, 0, 0, 0, 0, 0, 0);
                assert_eq!((quad.x(), quad.y(), quad.z()), (x, y, z));
            }
        }
    }
}

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
fn output_order_is_deterministic() {
    let stone = 1;
    let leaves = 5;

    let mut data: Box<[BlockId; SECTION_VOLUME]> = vec![0_u16; SECTION_VOLUME]
        .into_boxed_slice()
        .try_into()
        .expect("the scratch array matches the section volume");
    for y in 0..32_usize {
        for z in 0..32_usize {
            for x in 0..32_usize {
                let block = match (x * 7 + z * 13 + y) % 11 {
                    0 | 1 => 0,
                    2 => leaves,
                    _ => stone,
                };
                data[crate::section::section_index(
                    u32::try_from(x).expect("fits"),
                    u32::try_from(y).expect("fits"),
                    u32::try_from(z).expect("fits"),
                )] = block;
            }
        }
    }

    let direct = Section::from_blocks(&data);
    let mut incremental = Section::uniform(0);
    for (index, block) in data.iter().enumerate().rev() {
        incremental.set_index(index, *block);
    }
    assert_eq!(direct, incremental);

    let direct_pad = padded(&direct);
    let mut incremental_pad = PaddedBlocks::new();
    incremental_pad.fill_from(&incremental);
    let material = props();
    let light = lit();

    let first = mesh(&direct, &direct_pad, &light, &material, MeshLod::Lod0);
    let second = mesh(&direct, &direct_pad, &light, &material, MeshLod::Lod0);
    let third = mesh(
        &incremental,
        &incremental_pad,
        &light,
        &material,
        MeshLod::Lod0,
    );
    assert_eq!(raw(&first), raw(&second));
    assert_eq!(raw(&first), raw(&third));
}

#[test]
fn coarse_skips_ao_and_block_light() {
    let stone = 1;
    let mut floor = PaddedBlocks::new();
    for x in 0..LOCAL_SIZE {
        for z in 0..LOCAL_SIZE {
            floor.set(x, 0, z, stone);
        }
    }
    let mut roofed = floor.clone();
    for x in 0..LOCAL_SIZE {
        for z in 0..LOCAL_SIZE {
            roofed.set(x, 10, z, stone);
        }
    }

    let open = mesh_coarse(&floor, &props(), MeshLod::Lod1, 1);
    let open_tops: Vec<&Quad> = open
        .opaque
        .iter()
        .filter(|quad| quad.normal() == NORMAL_POS_Y && quad.y() == 0)
        .collect();
    assert_eq!(open_tops.len(), 1);
    assert_eq!(open_tops[0].sky(), 15, "open ceiling is full sky");

    let shaded = mesh_coarse(&roofed, &props(), MeshLod::Lod1, 1);
    let shaded_tops: Vec<&Quad> = shaded
        .opaque
        .iter()
        .filter(|quad| quad.normal() == NORMAL_POS_Y && quad.y() == 0)
        .collect();
    assert_eq!(shaded_tops.len(), 1, "the uniform sky still merges");
    assert_eq!(shaded_tops[0].sky(), 6, "15 - (10 - 1)");
    assert!(shaded.opaque.iter().all(|quad| quad.ao() == 0));
    assert!(shaded.opaque.iter().all(|quad| quad.blk() == 0));
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

#[test]
fn skirt_extends_the_boundary_downward() {
    let section = cap();
    let blocks = padded(&section);
    let mut mesh = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
    let before = mesh.opaque.len();
    let top = mesh
        .opaque
        .iter()
        .find(|quad| quad.normal() == NORMAL_POS_Y)
        .copied()
        .expect("the cap has a top face");
    assert_eq!(top.y(), 31, "the cap sits on the top cell layer");

    let added = add_skirts(&mut mesh, SkirtEdges::ALL, MeshLod::Lod0);

    let depth = u32::try_from(SKIRT_DEPTH).expect("SKIRT_DEPTH is positive");
    assert_eq!(added, 8, "four sides at SKIRT_DEPTH cells each");
    assert_eq!(mesh.opaque.len(), before + 8);
    let skirts = added_skirts(&mesh, before);
    assert_eq!(skirts.len(), 8);

    // One quad per cell layer, top layer first, hanging under the cap.
    let expected: Vec<u8> = (1..=depth)
        .map(|layer| top.y() - u8::try_from(layer).expect("a layer fits a cell index"))
        .collect();
    assert_eq!(expected, vec![30, 29]);

    for (side, normal) in [
        (Side::North, NORMAL_NEG_Z),
        (Side::South, NORMAL_POS_Z),
        (Side::East, NORMAL_POS_X),
        (Side::West, NORMAL_NEG_X),
    ] {
        let wall: Vec<Quad> = skirts
            .iter()
            .copied()
            .filter(|quad| quad.normal() == normal)
            .collect();
        assert_eq!(wall.len(), 2, "{side:?} is SKIRT_DEPTH cells deep");
        let ys: Vec<u8> = wall.iter().map(|quad| quad.y()).collect();
        assert_eq!(ys, expected, "{side:?} hangs straight down");
        for quad in wall {
            assert_eq!(quad.w1(), 31, "{side:?} is one run across the edge");
            assert_eq!(quad.h1(), 0, "{side:?} quads are one cell tall");
        }
    }

    // World extents: the north wall lies on the z = 0 plane, spans all 32
    // cells of x, and reaches two blocks below the boundary face.
    let north = skirts
        .iter()
        .find(|quad| quad.normal() == NORMAL_NEG_Z)
        .copied()
        .expect("the north edge has a skirt");
    assert_eq!((north.x(), north.z()), (0, 0));
    assert_eq!(u32::from(north.w1()) + 1, 32, "the full x width in blocks");
    let lowest = skirts
        .iter()
        .filter(|quad| quad.normal() == NORMAL_NEG_Z)
        .map(|quad| quad.y())
        .min()
        .expect("the north edge has a skirt");
    let expected_blocks = skirt_blocks(MeshLod::Lod0);
    assert_eq!(
        u32::from(top.y()) - u32::from(lowest),
        expected_blocks,
        "the strip extends the boundary face down by SKIRT_DEPTH blocks"
    );
    assert_eq!(expected_blocks, 2);

    let south = skirts
        .iter()
        .find(|quad| quad.normal() == NORMAL_POS_Z)
        .copied()
        .expect("the south edge has a skirt");
    assert_eq!((south.x(), south.z()), (0, 31));
    let east = skirts
        .iter()
        .find(|quad| quad.normal() == NORMAL_POS_X)
        .copied()
        .expect("the east edge has a skirt");
    assert_eq!((east.x(), east.z()), (31, 0));
    assert_eq!(u32::from(east.w1()) + 1, 32, "the full z width in blocks");
    let west = skirts
        .iter()
        .find(|quad| quad.normal() == NORMAL_NEG_X)
        .copied()
        .expect("the west edge has a skirt");
    assert_eq!((west.x(), west.z()), (0, 0));
}

#[test]
fn skirt_depth_scales_with_lod() {
    let section = cap();
    let blocks = padded(&section);
    let material = props();
    let light = lit();

    let mut fine = mesh(&section, &blocks, &light, &material, MeshLod::Lod0);
    let fine_before = fine.opaque.len();
    assert_eq!(add_skirts(&mut fine, SkirtEdges::ALL, MeshLod::Lod0), 8);
    let mut coarse = mesh(&section, &blocks, &light, &material, MeshLod::Lod1);
    let coarse_before = coarse.opaque.len();
    assert_eq!(add_skirts(&mut coarse, SkirtEdges::ALL, MeshLod::Lod1), 8);

    let wall = |built: &SectionMesh, before: usize| -> Vec<u8> {
        added_skirts(built, before)
            .iter()
            .filter(|quad| quad.normal() == NORMAL_NEG_Z)
            .map(|quad| quad.y())
            .collect()
    };
    let fine_wall = wall(&fine, fine_before);
    let coarse_wall = wall(&coarse, coarse_before);
    assert_eq!(fine_wall.len(), 2, "one quad per skirt cell");
    assert_eq!(
        coarse_wall, fine_wall,
        "the same cells carry the skirt at every level"
    );

    // Decode the world-space height from the `y` field: one coarse cell is
    // `cells_per_block` blocks tall, so LOD1's strip is twice as tall.
    let cell_span = |wall: &[u8]| -> u8 {
        let low = wall.iter().copied().min().expect("a wall has cells");
        let high = wall.iter().copied().max().expect("a wall has cells") + 1;
        high - low
    };
    assert_eq!(cell_span(&fine_wall), 2, "two cells of stone");
    assert_eq!(cell_span(&coarse_wall), 2);
    let fine_height = u32::from(cell_span(&fine_wall)) * MeshLod::Lod0.cells_per_block();
    let coarse_height = u32::from(cell_span(&coarse_wall)) * MeshLod::Lod1.cells_per_block();
    assert_eq!(fine_height, skirt_blocks(MeshLod::Lod0));
    assert_eq!(coarse_height, skirt_blocks(MeshLod::Lod1));
    assert_eq!(fine_height, 2);
    assert_eq!(coarse_height, 4, "LOD1 cells are two blocks tall");
    assert_eq!(coarse_height, 2 * fine_height, "twice as tall in blocks");
}

#[test]
fn skirt_closes_a_one_block_step_at_the_edge() {
    // A single exposed block on the east boundary, one cell above the
    // section floor: the strip has exactly one cell of room below it.
    let section = section_with(&[([31, 1, 10], 1)]);
    let blocks = padded(&section);
    let mut mesh = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
    let top = top_quad(&mesh, (31, 1, 10));
    assert_eq!(top.y(), 1, "the step top is one cell above the floor");

    let before = mesh.opaque.len();
    assert_eq!(
        add_skirts(&mut mesh, SkirtEdges::EAST, MeshLod::Lod0),
        1,
        "a one-cell step emits one skirt quad, not SKIRT_DEPTH of them"
    );
    let skirts = added_skirts(&mesh, before);
    assert_eq!(skirts.len(), 1);
    let skirt = skirts[0];
    assert_eq!(
        (skirt.x(), skirt.y(), skirt.z()),
        (31, 0, 10),
        "the strip starts directly under the boundary top"
    );
    assert_eq!(skirt.normal(), NORMAL_POS_X, "the east wall faces +X");
    assert_eq!((skirt.w1(), skirt.h1()), (0, 0), "one cell wide, one tall");
    assert_eq!(
        u32::from(top.y()) - u32::from(skirt.y()),
        1,
        "the strip height equals the step height"
    );
}

#[test]
fn step_skirt_fills_a_one_block_drop() {
    let stone = 1;
    let section = section_with(&[([31, 4, 10], stone), ([31, 5, 10], stone)]);
    let plus_x = section_with(&[([0, 4, 10], stone)]);
    let air = Section::uniform(0);
    let mut owners: [Option<&Section>; 26] = [Some(&air); 26];
    owners[0] = Some(&plus_x);
    let blocks = PaddedBlocks::from_sections(&section, &owners);
    let material = props();
    let light = lit();
    let mut mesh = mesh(&section, &blocks, &light, &material, MeshLod::Lod0);
    let top = top_quad(&mesh, (31, 5, 10));
    assert_eq!(top.y(), 5);

    let before = mesh.opaque.len();
    assert_eq!(
        add_step_skirts(
            &mut mesh,
            &blocks,
            &material,
            SkirtEdges::EAST,
            MeshLod::Lod0
        ),
        1,
        "a one-cell drop needs exactly one skirt cell"
    );
    let skirts = added_skirts(&mesh, before);
    assert_eq!(skirts.len(), 1);
    let skirt = skirts[0];
    assert_eq!(
        (skirt.x(), skirt.y(), skirt.z()),
        (31, 4, 10),
        "the strip fills the cell between the top and the lower column"
    );
    assert_eq!(skirt.normal(), NORMAL_POS_X);
    assert_eq!(
        u32::from(top.y()) - u32::from(skirt.y()),
        1,
        "the strip height equals the drop"
    );
}

#[test]
fn step_skirt_is_empty_on_a_flush_edge() {
    let stone = 1;
    let section = section_with(&[([31, 5, 10], stone)]);
    let air = Section::uniform(0);
    let material = props();
    let light = lit();

    for (height, why) in [(5_u32, "flush"), (6_u32, "neighbour above")] {
        let neighbour = section_with(&[([0, height, 10], stone)]);
        let mut owners: [Option<&Section>; 26] = [Some(&air); 26];
        owners[0] = Some(&neighbour);
        let blocks = PaddedBlocks::from_sections(&section, &owners);
        let mut mesh = mesh(&section, &blocks, &light, &material, MeshLod::Lod0);
        let before = mesh.opaque.len();
        assert_eq!(
            add_step_skirts(
                &mut mesh,
                &blocks,
                &material,
                SkirtEdges::EAST,
                MeshLod::Lod0
            ),
            0,
            "{why} edge has no drop to fill"
        );
        assert_eq!(mesh.opaque.len(), before, "{why} edge appends nothing");
    }
}

#[test]
fn step_skirts_never_appear_on_interior_edges() {
    let stone = 1;

    let interior = section_with(&[([10, 10, 10], stone)]);
    let blocks = padded(&interior);
    let material = props();
    let mut middle = mesh(&interior, &blocks, &lit(), &material, MeshLod::Lod0);
    let before = middle.opaque.len();
    assert_eq!(
        add_step_skirts(
            &mut middle,
            &blocks,
            &material,
            SkirtEdges::ALL,
            MeshLod::Lod0
        ),
        0,
        "an interior column has no boundary face"
    );
    assert_eq!(middle.opaque.len(), before);

    // A west-edge column asked for an east skirt emits nothing.
    let west = section_with(&[([0, 10, 15], stone)]);
    let blocks = padded(&west);
    let mut east_only = mesh(&west, &blocks, &lit(), &material, MeshLod::Lod0);
    let before = east_only.opaque.len();
    assert_eq!(
        add_step_skirts(
            &mut east_only,
            &blocks,
            &material,
            SkirtEdges::NONE.with(Side::East),
            MeshLod::Lod0
        ),
        0
    );
    assert_eq!(east_only.opaque.len(), before);
}

#[test]
fn step_skirts_are_deterministic_and_append_once() {
    let stone = 1;
    let section = section_with(&[([31, 4, 10], stone), ([31, 5, 10], stone)]);
    let plus_x = section_with(&[([0, 4, 10], stone)]);
    let air = Section::uniform(0);
    let mut owners: [Option<&Section>; 26] = [Some(&air); 26];
    owners[0] = Some(&plus_x);
    let blocks = PaddedBlocks::from_sections(&section, &owners);
    let material = props();
    let light = lit();

    let build = || {
        let mut mesh = mesh(&section, &blocks, &light, &material, MeshLod::Lod0);
        assert_eq!(
            add_step_skirts(
                &mut mesh,
                &blocks,
                &material,
                SkirtEdges::ALL,
                MeshLod::Lod0
            ),
            1
        );
        mesh
    };
    let first = build();
    let second = build();
    assert_eq!(raw(&first), raw(&second), "same input, same bytes");

    let mut mesh = build();
    let once = raw(&mesh);
    assert_eq!(
        add_step_skirts(
            &mut mesh,
            &blocks,
            &material,
            SkirtEdges::ALL,
            MeshLod::Lod0
        ),
        0
    );
    assert_eq!(raw(&mesh), once, "a skirt set is never appended twice");
}

#[test]
fn flat_boundary_has_a_zero_height_skirt() {
    // No exposed top face at the edge: an empty section adds nothing.
    let mut empty = SectionMesh::default();
    assert_eq!(add_skirts(&mut empty, SkirtEdges::ALL, MeshLod::Lod0), 0);
    assert_eq!(empty.opaque.len(), 0);
    assert!(empty.skirts.is_empty());

    // A boundary surface on the section floor has no cell below it inside
    // this section, so the skirt clamps to zero height. This is the
    // documented flat-edge case: zero quads, not a strip into the section
    // below.
    let section = slab();
    let blocks = padded(&section);
    let mut flat = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
    let before = flat.opaque.len();
    assert_eq!(
        add_skirts(&mut flat, SkirtEdges::ALL, MeshLod::Lod0),
        0,
        "a floor-level boundary has zero skirt height"
    );
    assert_eq!(flat.opaque.len(), before, "no record is appended");
}

#[test]
fn interior_edges_never_get_a_skirt() {
    // A block in the middle of the section has no face on any boundary
    // plane, so no side can hang a skirt from it.
    let interior = section_with(&[([10, 10, 10], 1)]);
    let blocks = padded(&interior);
    let mut middle = mesh(&interior, &blocks, &lit(), &props(), MeshLod::Lod0);
    let before = middle.opaque.len();
    assert_eq!(add_skirts(&mut middle, SkirtEdges::ALL, MeshLod::Lod0), 0);
    assert_eq!(middle.opaque.len(), before);

    // A west-edge block asked for an east skirt emits nothing: the
    // requested edge has no boundary face.
    let west = section_with(&[([0, 10, 15], 1)]);
    let blocks = padded(&west);
    let mut east_only = mesh(&west, &blocks, &lit(), &props(), MeshLod::Lod0);
    let before = east_only.opaque.len();
    assert_eq!(
        add_skirts(
            &mut east_only,
            SkirtEdges::NONE.with(Side::East),
            MeshLod::Lod0
        ),
        0
    );
    assert_eq!(east_only.opaque.len(), before);

    // Selecting the edge that does own the face emits there and nowhere
    // else, which is the interior-edge invariant.
    let mut west_only = mesh(&west, &blocks, &lit(), &props(), MeshLod::Lod0);
    let before = west_only.opaque.len();
    assert_eq!(
        add_skirts(
            &mut west_only,
            SkirtEdges::NONE.with(Side::West),
            MeshLod::Lod0
        ),
        2
    );
    let skirts = added_skirts(&west_only, before);
    assert_eq!(skirts.len(), 2);
    assert!(
        skirts.iter().all(|quad| quad.normal() == NORMAL_NEG_X),
        "only the west wall is emitted"
    );
}

#[test]
fn no_skirt_without_a_boundary_face() {
    // An empty mesh has nothing to hang a skirt from.
    let mut empty = SectionMesh::default();
    assert_eq!(add_skirts(&mut empty, SkirtEdges::ALL, MeshLod::Lod0), 0);
    assert_eq!(empty.opaque.len(), 0);
    assert!(empty.skirts.is_empty());

    // A block in the middle of the section has six faces, none of them on a
    // boundary plane.
    let stone = section_with(&[([10, 10, 10], 1)]);
    let blocks = padded(&stone);
    let mut interior = mesh(&stone, &blocks, &lit(), &props(), MeshLod::Lod0);
    let before = interior.opaque.len();
    assert_eq!(before, 6);
    assert_eq!(add_skirts(&mut interior, SkirtEdges::ALL, MeshLod::Lod0), 0);
    assert_eq!(interior.opaque.len(), before);

    // A section buried in solid rock emits no face at all.
    let solid = Section::uniform(1);
    let buried = PaddedBlocks::from_sections(&solid, &[Some(&solid); 26]);
    let mut buried_mesh = mesh(&solid, &buried, &lit(), &props(), MeshLod::Lod0);
    assert_eq!(quad_count(&buried_mesh), 0);
    assert_eq!(
        add_skirts(&mut buried_mesh, SkirtEdges::ALL, MeshLod::Lod0),
        0
    );
    assert_eq!(buried_mesh.opaque.len(), 0);
}

#[test]
fn skirts_are_deterministic() {
    let first = skirted_cap();
    let second = skirted_cap();
    assert_eq!(raw(&first), raw(&second), "same input, same bytes");

    // The marker makes a second apply a no-op instead of a doubled strip.
    let mut mesh = skirted_cap();
    let once = raw(&mesh);
    assert_eq!(add_skirts(&mut mesh, SkirtEdges::ALL, MeshLod::Lod0), 0);
    assert_eq!(raw(&mesh), once, "a skirt set is never appended twice");
}

#[test]
fn skirt_ao_never_darkens() {
    let section = cap();
    let blocks = padded(&section);
    let mut mesh = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
    let before = mesh.opaque.len();
    assert_eq!(add_skirts(&mut mesh, SkirtEdges::ALL, MeshLod::Lod0), 8);
    let skirts = added_skirts(&mesh, before);

    assert_eq!(skirts.len(), 8);
    for quad in skirts {
        assert_eq!(quad.ao(), 0xFF, "every corner is fully open");
        for corner in 0_u32..4 {
            assert_eq!(
                (quad.ao() >> (2 * corner)) & 0b11,
                3,
                "corner {corner} of a skirt never darkens the seam"
            );
        }
    }
}

#[test]
fn one_side_only_touches_that_side() {
    let edges = SkirtEdges::NONE.with(Side::East);
    assert!(edges.east());
    assert!(!edges.north());
    assert!(!edges.south());
    assert!(!edges.west());
    assert!(SkirtEdges::ALL.north());
    assert!(SkirtEdges::ALL.south());
    assert!(SkirtEdges::ALL.east());
    assert!(SkirtEdges::ALL.west());
    assert!(SkirtEdges::NONE.is_empty());
    assert_eq!(
        SkirtEdges::NONE.with(Side::East).without(Side::East),
        SkirtEdges::NONE
    );

    // `Side` keeps the `revisions::Face` order and meaning.
    for (index, face) in ColumnFace::ALL.into_iter().enumerate() {
        assert_eq!(Side::from(face), Side::ALL[index]);
    }

    let section = cap();
    let blocks = padded(&section);
    let mut mesh = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
    let before = mesh.opaque.len();
    assert_eq!(add_skirts(&mut mesh, edges, MeshLod::Lod0), 2);
    let skirts = added_skirts(&mesh, before);

    assert_eq!(skirts.len(), 2);
    for quad in skirts {
        assert_eq!(quad.normal(), NORMAL_POS_X, "only the east wall");
        assert_eq!(quad.x(), 31, "the east edge plane");
        assert_eq!(quad.z(), 0, "the run starts at the north end");
        assert_eq!(quad.w1(), 31, "the run covers the whole east edge");
    }
}

/// Hashes one generated section's three ranges so a refactor cannot change
/// the bytes the mesher emits for a fixed seed and column.
#[test]
fn generated_section_mesh_is_hash_stable() {
    use crate::hash::sha256;
    use crate::pipeline::Terrain;
    use std::fmt::Write as _;

    fn digest(quads: &[Quad]) -> String {
        let mut bytes = Vec::with_capacity(quads.len() * 8);
        for quad in quads {
            bytes.extend_from_slice(&quad.bits().to_le_bytes());
        }
        let mut out = String::new();
        for byte in sha256(&bytes) {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    let pack = crate::content::v1_default();
    let mut materials = MaterialProps::new();
    for record in pack.records() {
        let id = pack.id(&record.name).expect("every pack record has an id");
        materials.set(
            id,
            Material {
                solid: record.collision,
                opaque: record.opacity >= crate::content::LIGHT_LEVELS,
                water: record.liquid,
                cross_quad: record.render_shape == crate::content::RenderShape::CrossQuad,
                texture_top: 0,
                texture_side: 0,
                texture_bottom: 0,
                tint: 0,
                emissive: record.emission > 0,
            },
        );
    }

    let terrain = Terrain::new(2).expect("the fixture terrain builds");
    let section = terrain
        .section(7, 4, 1)
        .expect("the fixture section generates");
    let air = Section::uniform(0);
    let owners: [Option<&Section>; 26] = [Some(&air); 26];
    let blocks = PaddedBlocks::from_sections(&section, &owners);
    let result = mesh(&section, &blocks, &lit(), &materials, MeshLod::Lod0);

    assert_eq!(
        digest(&result.opaque),
        "5a2e86e7b1e39241efbdcbee2c2cc0b135109c6a90f5efc1acd4fca4f07357fe"
    );
    assert_eq!(
        digest(&result.cutout),
        "92fc8acf2625cd0a4da055029edf034ac5e7ceeec903a55890f2e791f511a962"
    );
    assert_eq!(
        digest(&result.water),
        "ddf55caf61f2c48bf8fc16807d55617512b94da54f3becc7527ad7eb9e32c21f"
    );
}
