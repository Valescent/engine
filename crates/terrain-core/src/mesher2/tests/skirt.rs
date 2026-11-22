//! Skirt and step-skirt tests.

use super::*;
use crate::lod::SKIRT_DEPTH;
use crate::padded::PaddedBlocks;
use crate::revisions::Face as ColumnFace;
use crate::section::Section;

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
