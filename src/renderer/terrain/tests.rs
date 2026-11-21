//! Renderer terrain tests.

use super::*;

/// The CPU mirror of `terrain_draw.wgsl`'s `face_duv`: `(du, dv)`
/// multipliers of the quad extents for one corner slot, in the face's
/// tangent order.
fn face_duv(normal: u8, slot: u32) -> [f32; 2] {
    let mut duv = match slot {
        1 => [0.0, 1.0],
        2 => [1.0, 1.0],
        3 => [1.0, 0.0],
        _ => [0.0, 0.0],
    };
    match normal {
        1 | 4 => duv[0] = 1.0 - duv[0],
        3 => duv[1] = 1.0 - duv[1],
        _ => {}
    }
    duv
}

/// The CPU mirror of `terrain_draw.wgsl`'s `corner_geometry`.
fn corner_position(normal: u8, slot: u32, base: [f32; 3], width: f32, height: f32) -> [f32; 3] {
    let [x, y, z] = base;
    if normal >= 6 {
        // Cross quads: a vertical diagonal through the cell.
        let (start, end) = if normal == 6 {
            ([x, z], [x + 1.0, z + 1.0])
        } else {
            ([x + 1.0, z], [x, z + 1.0])
        };
        return match slot {
            0 => [start[0], y, start[1]],
            1 => [start[0], y + 1.0, start[1]],
            2 => [end[0], y + 1.0, end[1]],
            _ => [end[0], y, end[1]],
        };
    }
    let duv = face_duv(normal, slot);
    let along_u = duv[0] * width;
    let along_v = duv[1] * height;
    match normal {
        0 => [x + 1.0, y + along_v, z + along_u],
        1 => [x, y + along_v, z + along_u],
        2 => [x + along_u, y + 1.0, z + along_v],
        3 => [x + along_u, y, z + along_v],
        4 => [x + along_u, y + along_v, z + 1.0],
        _ => [x + along_u, y + along_v, z],
    }
}

/// The six section-local positions of one quad, after the AO flip.
fn quad_positions(quad: DecodedQuad) -> [[f32; 3]; 6] {
    let flip = quad.normal < 6 && (quad.ao[0] + quad.ao[2]) > (quad.ao[1] + quad.ao[3]);
    let order: [u32; 6] = if flip {
        [1, 2, 3, 1, 3, 0]
    } else {
        [0, 1, 2, 0, 2, 3]
    };
    let base = [f32::from(quad.x), f32::from(quad.y), f32::from(quad.z)];
    let width = f32::from(quad.w1) + 1.0;
    let height = f32::from(quad.h1) + 1.0;
    order.map(|slot| corner_position(quad.normal, slot, base, width, height))
}

#[test]
fn decode_quad_reads_every_field_for_every_normal() {
    for normal in 0..=5 {
        let raw = Quad::encode(3, 7, 11, 9, 17, normal, 0xE4, 13, 5, 777, 9).bits();
        let quad = decode_quad(raw);

        assert_eq!((quad.x, quad.y, quad.z), (3, 7, 11), "normal {normal}");
        assert_eq!((quad.w1, quad.h1), (9, 17), "normal {normal}");
        assert_eq!(quad.normal, normal);
        assert_eq!(quad.ao, [0, 1, 2, 3], "normal {normal}");
        assert_eq!((quad.sky, quad.blk), (13, 5), "normal {normal}");
        assert_eq!(quad.tile, 777, "normal {normal}");
        assert_eq!(quad.tint, 9, "normal {normal}");
    }
}

#[test]
#[allow(clippy::float_cmp)]
fn plus_y_one_block_quad_has_the_expected_six_corners() {
    let quad = decode_quad(Quad::encode(2, 3, 4, 0, 0, 2, 0xE4, 15, 0, 2, 0).bits());

    assert_eq!(
        quad_positions(quad),
        [
            [2.0, 4.0, 4.0],
            [2.0, 4.0, 5.0],
            [3.0, 4.0, 5.0],
            [2.0, 4.0, 4.0],
            [3.0, 4.0, 5.0],
            [3.0, 4.0, 4.0],
        ]
    );
}

#[test]
#[allow(clippy::float_cmp)]
fn ao_anisotropy_flips_the_diagonal() {
    // Corner 0 and corner 2 are fully open; the other pair is fully dark.
    let quad = decode_quad(Quad::encode(0, 0, 0, 0, 0, 2, 0x33, 15, 0, 0, 0).bits());
    assert_eq!(quad.ao, [3, 0, 3, 0]);

    let positions = quad_positions(quad);
    assert_eq!(
        positions,
        [
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ]
    );
}

#[test]
#[allow(clippy::float_cmp)]
fn cross_quad_decodes_with_zero_extents() {
    let quad = decode_quad(Quad::encode(5, 6, 7, 0, 0, 6, 0, 4, 2, 12, 3).bits());

    assert_eq!(quad.normal, 6);
    assert_eq!((quad.w1, quad.h1), (0, 0));
    assert_eq!((quad.x, quad.y, quad.z), (5, 6, 7));
    assert_eq!((quad.sky, quad.blk), (4, 2));

    // A cross quad still covers one vertical cell, so its corners span 1x1.
    let positions = quad_positions(quad);
    assert_eq!(positions[0], [5.0, 6.0, 7.0]);
    assert_eq!(positions[2], [6.0, 7.0, 8.0]);
}

#[test]
fn water_sections_draw_far_to_near_from_the_camera() {
    let camera = IVec3::ZERO;
    let mut sections = vec![
        ([0, 0, 0], 1),
        ([64, 0, 0], 1),
        ([128, 0, 0], 1),
        ([-64, 0, 0], 1),
    ];

    sort_water_order(&mut sections, camera);

    assert_eq!(
        sections
            .iter()
            .map(|&(origin, _)| origin)
            .collect::<Vec<_>>(),
        vec![[128, 0, 0], [64, 0, 0], [-64, 0, 0], [0, 0, 0]],
        "the farthest section centre draws first"
    );
}

#[test]
fn equal_water_distances_break_by_origin() {
    // At scale 1 the centres are (48, 16, 16) and (-16, 16, 16); both are
    // 48 blocks from the camera on the x axis, so the distance ties.
    let camera = IVec3::ZERO;
    let mut sections = vec![([32, 0, 0], 1), ([-64, 0, 0], 1)];

    sort_water_order(&mut sections, camera);

    assert_eq!(
        sections
            .iter()
            .map(|&(origin, _)| origin)
            .collect::<Vec<_>>(),
        vec![[-64, 0, 0], [32, 0, 0]]
    );
}

fn revisions(entries: &[([i32; 3], u64)]) -> BTreeMap<[i32; 3], u64> {
    entries.iter().copied().collect()
}

#[test]
fn plan_upload_adds_a_new_origin() {
    let current = revisions(&[([0, 0, 0], 1)]);
    let incoming = [([0, 0, 0], 1), ([32, 0, 0], 7)];

    let plan = plan_upload(&current, &incoming).expect("origins are unique");

    assert_eq!(plan.uploads, vec![([32, 0, 0], 7)]);
    assert_eq!(plan.removals, Vec::<[i32; 3]>::new());
}

#[test]
fn plan_upload_rewrites_a_changed_revision() {
    let current = revisions(&[([0, 0, 0], 1), ([32, 0, 0], 4)]);
    let incoming = [([0, 0, 0], 1), ([32, 0, 0], 5)];

    let plan = plan_upload(&current, &incoming).expect("origins are unique");

    assert_eq!(plan.uploads, vec![([32, 0, 0], 5)]);
    assert_eq!(plan.removals, Vec::<[i32; 3]>::new());
}

#[test]
fn plan_upload_skips_unchanged_revisions() {
    let current = revisions(&[([0, 0, 0], 1), ([32, 0, 0], 4)]);
    let incoming = [([0, 0, 0], 1), ([32, 0, 0], 4)];

    let plan = plan_upload(&current, &incoming).expect("origins are unique");

    assert_eq!(plan.uploads, Vec::<([i32; 3], u64)>::new());
    assert_eq!(plan.removals, Vec::<[i32; 3]>::new());
}

#[test]
fn plan_upload_drops_a_removed_origin() {
    let current = revisions(&[([0, 0, 0], 1), ([32, 0, 0], 4)]);
    let incoming = [([0, 0, 0], 1)];

    let plan = plan_upload(&current, &incoming).expect("origins are unique");

    assert_eq!(plan.uploads, Vec::<([i32; 3], u64)>::new());
    assert_eq!(plan.removals, vec![[32, 0, 0]]);
}

#[test]
fn plan_upload_rejects_duplicate_origins() {
    let current = BTreeMap::new();
    let incoming = [([1, 2, 3], 1), ([1, 2, 3], 2)];

    let error = plan_upload(&current, &incoming).expect_err("duplicates are rejected");

    assert_eq!(error.origin, [1, 2, 3]);
}

#[test]
fn section_revision_tracks_content_and_length() {
    let quad = Quad::encode(0, 0, 0, 0, 0, 2, 0, 15, 0, 1, 0);
    let darker = Quad::encode(0, 0, 0, 0, 0, 2, 0, 14, 0, 1, 0);
    let one = [quad];
    let same = [quad];
    let two = [quad, quad];
    let changed = [darker];

    let section = TerrainSection {
        origin: [0; 3],
        scale: 1,
        opaque: &one,
        cutout: &[],
        water: &[],
    };
    let identical = TerrainSection {
        origin: [0; 3],
        scale: 1,
        opaque: &same,
        cutout: &[],
        water: &[],
    };
    let longer = TerrainSection {
        origin: [0; 3],
        scale: 1,
        opaque: &two,
        cutout: &[],
        water: &[],
    };
    let edited = TerrainSection {
        origin: [0; 3],
        scale: 1,
        opaque: &changed,
        cutout: &[],
        water: &[],
    };

    assert_eq!(section_revision(&section), section_revision(&identical));
    assert_ne!(section_revision(&section), section_revision(&longer));
    assert_ne!(section_revision(&section), section_revision(&edited));
}
