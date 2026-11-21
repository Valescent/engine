//! Tests for LOD band selection, clipping, and origin uniqueness.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use glam::IVec2;

use crate::content;
use crate::hydro;
use crate::mesher2;
use crate::section;

use super::super::content::demo_materials;
use super::super::patch::TerrainDemo;
use super::super::persist::{build_terrain_demo, build_terrain_demo_extent};
use super::super::{TERRAIN_DEMO_FINE_MARGIN, TERRAIN_DEMO_FINE_RADIUS};

use super::*;

#[test]
fn lod_uses_coarse_regions_away_from_the_player() {
    let mut demo = build_terrain_demo(0x5EED, IVec2::new(2, -3), 2).expect("the v1 terrain builds");
    assert!(
        !demo.coarse.is_empty(),
        "a radius-2 patch has complete octant regions"
    );
    demo.fine_radius = 0;
    let fine: usize = demo.fine.values().map(Vec::len).sum();
    demo.select_lod((demo.origin_column.x, demo.origin_column.y));
    let near_fine: usize = demo
        .fine
        .iter()
        .filter(|(key, _)| {
            let (x, z) = **key;
            let (px, pz) = (demo.origin_column.x, demo.origin_column.y);
            (x - px).abs().max((z - pz).abs()) <= 1
        })
        .map(|(_, sections)| sections.len())
        .sum();
    assert_eq!(
        demo.sections.len(),
        near_fine,
        "the player's column and its fine ring are full detail"
    );

    demo.select_lod((demo.origin_column.x + 2, demo.origin_column.y + 2));
    assert!(
        demo.sections.len() < fine,
        "a region two columns away is drawn coarse: {} active vs {fine} fine",
        demo.sections.len()
    );

    let coarse: BTreeSet<[i32; 3]> = demo.coarse.iter().map(|section| section.origin).collect();
    let active_fine = demo
        .sections
        .iter()
        .filter(|section| !coarse.contains(&section.origin))
        .count();
    let active_coarse = demo.sections.len() - active_fine;
    assert!(active_coarse > 0, "at least one coarse section is active");
    assert!(active_fine > 0, "the player's region stays fine");

    let mut origins = BTreeSet::new();
    for section in &demo.sections {
        assert!(
            origins.insert(section.origin),
            "fine and coarse sections never overlap"
        );
    }
    assert!(
        demo.coarse
            .iter()
            .any(|section| !section.mesh.skirts.is_empty()),
        "coarse meshes carry skirts for the fine boundary"
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
fn lod2_selects_the_far_band_without_overlapping_lod1() {
    let mut demo = build_terrain_demo(0x5EED, IVec2::new(0, 0), 0).expect("the v1 terrain builds");
    demo.origin_column = IVec2::ZERO;
    demo.fine.clear();
    demo.coarse = vec![
        stub_section([4 * 32, 0, 4 * 32], 2),
        stub_section([8 * 32, 0, 8 * 32], 2),
        stub_section([10 * 32, 0, 10 * 32], 2),
    ];
    demo.far = vec![
        stub_section([8 * 32, 0, 8 * 32], 4),
        stub_section([0, 0, 0], 4),
    ];
    demo.select_lod((0, 0));

    let origins: BTreeSet<[i32; 3]> = demo.sections.iter().map(|s| s.origin).collect();
    assert!(origins.contains(&[4 * 32, 0, 4 * 32]), "middle LOD1 region");
    assert!(
        origins.contains(&[8 * 32, 0, 8 * 32]),
        "far LOD2 region replaces its LOD1 copy"
    );
    assert!(
        !origins.contains(&[10 * 32, 0, 10 * 32]),
        "LOD1 inside an active LOD2 square is suppressed"
    );
    assert!(
        !origins.contains(&[0, 0, 0]),
        "LOD2 touching the fine buffer is suppressed"
    );
    assert_eq!(demo.sections.len(), 2);
    assert_eq!(
        demo.far
            .iter()
            .find(|s| s.origin == [8 * 32, 0, 8 * 32])
            .map(|s| s.scale),
        Some(4)
    );
}

#[test]
fn lod2_regions_are_complete_and_skirted() {
    let mut columns = BTreeMap::new();
    for x in 0..4 {
        for z in 0..4 {
            let stack: [section::Section; COLUMN_SECTIONS] =
                std::array::from_fn(|_| section::Section::uniform(1));
            columns.insert((x, z), Arc::new(stack));
        }
    }
    let regions = BTreeSet::from([(0, 0)]);
    let far = demo_far_regions(
        &columns,
        &demo_materials(),
        &content::v1_default(),
        &regions,
    );
    let sections = far.get(&(0, 0)).expect("the 4x4 region is complete");
    assert_eq!(sections.len(), 3, "one 128-block section per third");
    assert_eq!(
        sections.iter().map(|s| s.origin).collect::<Vec<_>>(),
        vec![[0, 0, 0], [0, 128, 0], [0, 256, 0]]
    );
    for section in sections {
        assert_eq!(section.scale, 4);
        assert!(mesher2::quad_count(&section.mesh) > 0);
        assert!(
            !section.mesh.skirts.is_empty(),
            "LOD2 sections are skirted on every side"
        );
    }
    assert!(
        demo_far_regions(
            &columns,
            &demo_materials(),
            &content::v1_default(),
            &BTreeSet::from([(4, 4)])
        )
        .get(&(4, 4))
        .is_none_or(Vec::is_empty),
        "a region with missing columns is not derived"
    );
}

/// A stub ring plus the coarse bands for one player column.
///
/// Only origins matter here, so a region is one stub section and a fine
/// column is one LOD0 stub. That keeps the coverage test independent of
/// terrain generation and meshing cost.
fn stub_lod_ring_at(radius: i32, player: (i32, i32)) -> TerrainDemo {
    let mut demo = stub_ring_demo(radius, player, player);
    let stack: [section::Section; COLUMN_SECTIONS] =
        std::array::from_fn(|_| section::Section::uniform(0));
    for x in player.0 - radius..=player.0 + radius {
        for z in player.1 - radius..=player.1 + radius {
            demo.columns.insert((x, z), Arc::new(stack.clone()));
        }
    }
    let cutoff = TERRAIN_DEMO_FINE_RADIUS + TERRAIN_DEMO_FINE_MARGIN;
    for &(x, z) in demo.columns.keys() {
        if (x - player.0).abs().max((z - player.1).abs()) <= cutoff {
            demo.fine
                .insert((x, z), vec![stub_section([x * 32, 0, z * 32], 1)]);
        }
    }
    let bands = coarse_bands(&demo.columns, player, radius);
    let mut origins = bands.coarse.clone();
    origins.extend(bands.partial.keys().copied());
    demo.coarse = origins
        .iter()
        .map(|&(x, z)| stub_section([x * 32, 0, z * 32], 2))
        .collect();
    demo.partial_columns = bands.partial;
    demo.far = bands
        .far
        .iter()
        .map(|&(x, z)| stub_section([x * 32, 0, z * 32], 4))
        .collect();
    demo.select_lod(player);
    demo
}

/// Active-section count per column footprint.
fn active_column_coverage(demo: &TerrainDemo) -> BTreeMap<(i32, i32), usize> {
    let mut coverage = BTreeMap::new();
    for section in &demo.sections {
        let scale = i32::try_from(section.scale).expect("scale fits");
        let x = section.origin[0] / 32;
        let z = section.origin[2] / 32;
        if let Some(columns) = demo.partial_columns.get(&(x, z)) {
            for &(cx, cz) in columns {
                *coverage.entry((cx, cz)).or_insert(0) += 1;
            }
            continue;
        }
        for dx in 0..scale {
            for dz in 0..scale {
                *coverage.entry((x + dx, z + dz)).or_insert(0) += 1;
            }
        }
    }
    coverage
}

#[test]
fn the_visible_ring_is_fully_covered_by_the_active_lod_set() {
    for radius in [8, 12, 16] {
        for player in [(0, 0), (0, -1), (-2, 3), (4, -3)] {
            let demo = stub_lod_ring_at(radius, player);
            let coverage = active_column_coverage(&demo);
            for x in player.0 - radius..=player.0 + radius {
                for z in player.1 - radius..=player.1 + radius {
                    assert_eq!(
                        coverage.get(&(x, z)).copied(),
                        Some(1),
                        "radius {radius} player {player:?}: column ({x}, {z}) must be \
                             covered by exactly one LOD"
                    );
                }
            }
            assert_unique_origins(&demo, &format!("radius {radius} player {player:?}"));
        }
    }
}

/// A stub ring of air columns centred on `centre`, with `player` as the
/// column the LOD set is selected for.
fn stub_ring_demo(radius: i32, centre: (i32, i32), player: (i32, i32)) -> TerrainDemo {
    let hydro = Arc::new(hydro::WorldHydro::solve(&[0.0], 1).expect("world hydro"));
    let mut demo = TerrainDemo::empty(0, IVec2::new(centre.0, centre.1), radius, hydro);
    demo.origin_column = IVec2::new(player.0, player.1);
    demo
}

/// Inserts one air column into a demo built by [`stub_ring_demo`].
fn insert_stub_column(demo: &mut TerrainDemo, column: (i32, i32)) {
    let stack: [section::Section; COLUMN_SECTIONS] =
        std::array::from_fn(|_| section::Section::uniform(0));
    demo.columns.insert(column, Arc::new(stack));
}

/// LOD0 stubs for every column inside the fine transition buffer.
fn install_fine_stubs(demo: &mut TerrainDemo, player: (i32, i32)) {
    let cutoff = TERRAIN_DEMO_FINE_RADIUS + TERRAIN_DEMO_FINE_MARGIN;
    let ring = |column: (i32, i32)| (column.0 - player.0).abs().max((column.1 - player.1).abs());
    demo.fine = demo
        .columns
        .keys()
        .copied()
        .filter(|&column| ring(column) <= cutoff)
        .map(|column| (column, fine_stub(column)))
        .collect();
}

fn fine_stub(column: (i32, i32)) -> Vec<DemoSection> {
    (0..COLUMN_SECTIONS)
        .map(|sy| {
            stub_section(
                [
                    column.0 * 32,
                    i32::try_from(sy).unwrap_or(0) * 32,
                    column.1 * 32,
                ],
                1,
            )
        })
        .collect()
}

/// One stub per vertical section of a coarse `2x2` (`scale == 2`) or `4x4`
/// (`scale == 4`) region.
fn band_stubs(origin_column: (i32, i32), scale: u32) -> Vec<DemoSection> {
    let (blocks, count) = if scale == 4 {
        (128, COLUMN_SECTIONS / 4)
    } else {
        (64, COLUMN_SECTIONS / 2)
    };
    (0..count)
        .map(|j| {
            stub_section(
                [
                    origin_column.0 * 32,
                    i32::try_from(j).unwrap_or(0) * blocks,
                    origin_column.1 * 32,
                ],
                scale,
            )
        })
        .collect()
}

/// Builds the three bands for `player`, as a full coarse pass would.
fn install_lod_state(demo: &mut TerrainDemo, player: (i32, i32), radius: i32) {
    install_fine_stubs(demo, player);
    let bands = coarse_bands(&demo.columns, player, radius);
    demo.coarse = bands
        .coarse
        .iter()
        .filter(|base| !bands.partial.contains_key(base))
        .flat_map(|&base| band_stubs(base, 2))
        .collect();
    demo.coarse.extend(
        bands
            .partial
            .keys()
            .flat_map(|&anchor| band_stubs(anchor, 2)),
    );
    demo.partial_columns = bands.partial.clone();
    demo.far = bands
        .far
        .iter()
        .flat_map(|&base| band_stubs(base, 4))
        .collect();
    demo.select_lod(player);
}

/// Asserts the active set has unique origins and passes the renderer's
/// duplicate-origin validation.
fn assert_unique_origins(demo: &TerrainDemo, label: &str) {
    let mut origins = BTreeSet::new();
    for section in &demo.sections {
        assert!(
            origins.insert(section.origin),
            "{label}: duplicate active section origin {:?} (scale {})",
            section.origin,
            section.scale
        );
    }
    let incoming: Vec<([i32; 3], u64)> = demo
        .sections
        .iter()
        .enumerate()
        .map(|(index, section)| (section.origin, u64::try_from(index).unwrap_or(0)))
        .collect();
    crate::renderer::plan_upload(&BTreeMap::new(), &incoming)
        .unwrap_or_else(|error| panic!("{label}: renderer rejected the submission: {error}"));
}

#[test]
fn a_full_fill_never_submits_one_origin_twice() {
    let centres = [(0, 0), (0, -1), (-2, 3), (4, -3)];
    for radius in [8, 12, 16] {
        for &centre in &centres {
            let players = [centre, (centre.0 + 1, centre.1), (centre.0, centre.1 - 2)];
            for player in players {
                let mut demo = stub_ring_demo(radius, centre, player);
                for x in centre.0 - radius..=centre.0 + radius {
                    for z in centre.1 - radius..=centre.1 + radius {
                        insert_stub_column(&mut demo, (x, z));
                    }
                }
                install_lod_state(&mut demo, player, radius);
                assert_unique_origins(
                    &demo,
                    &format!("radius {radius} centre {centre:?} player {player:?}"),
                );
            }
        }
    }
}

#[test]
fn an_incremental_fill_never_submits_one_origin_twice() {
    let player = (0, 0);
    for radius in [8, 12, 16] {
        let mut demo = stub_ring_demo(radius, player, player);
        for layer in 0..=radius {
            for x in -layer..=layer {
                for z in -layer..=layer {
                    if x.abs().max(z.abs()) == layer {
                        insert_stub_column(&mut demo, (x, z));
                    }
                }
            }
            // The coarse job is throttled, so most frames select LOD while
            // the bands are one or two ring layers stale.
            if layer % 2 == 0 || layer == radius {
                install_lod_state(&mut demo, player, radius);
            } else {
                install_fine_stubs(&mut demo, player);
                demo.select_lod(player);
            }
            assert_unique_origins(&demo, &format!("radius {radius} layer {layer}"));
        }
    }
}

#[test]
fn clipped_lod1_regions_cover_only_unowned_columns() {
    let cutoff = TERRAIN_DEMO_FINE_RADIUS + TERRAIN_DEMO_FINE_MARGIN;
    for radius in [8, 12, 16] {
        let demo = stub_lod_ring_at(radius, (0, 0));
        let bands = coarse_bands(&demo.columns, (0, 0), radius);
        let ring = |x: i32, z: i32| x.abs().max(z.abs());

        // Complete coverage that `select_lod` will actually draw.
        let mut complete = BTreeSet::new();
        for &(x, z) in &bands.far {
            assert_eq!(x.rem_euclid(4), 0, "LOD2 base {x} is not 4-aligned");
            assert_eq!(z.rem_euclid(4), 0, "LOD2 base {z} is not 4-aligned");
            for dx in 0..4 {
                for dz in 0..4 {
                    assert!(
                        demo.columns.contains_key(&(x + dx, z + dz)),
                        "LOD2 base ({x}, {z}) is not complete"
                    );
                    complete.insert((x + dx, z + dz));
                }
            }
        }
        for &(x, z) in &bands.coarse {
            let clear = (0..2).all(|dx| (0..2).all(|dz| ring(x + dx, z + dz) > cutoff));
            if clear {
                for dx in 0..2 {
                    for dz in 0..2 {
                        complete.insert((x + dx, z + dz));
                    }
                }
            }
        }

        let mut partial = BTreeSet::new();
        for (&anchor, columns) in &bands.partial {
            assert!(
                !columns.is_empty() && columns.len() <= 4,
                "a clipped region covers one to four columns: ({anchor:?})"
            );
            assert!(
                ring(anchor.0, anchor.1) > cutoff,
                "a clipped section origin must stay outside the LOD0 band ({anchor:?})"
            );
            for &(cx, cz) in columns {
                assert!(
                    cx >= anchor.0 && cx <= anchor.0 + 1,
                    "column ({cx}, {cz}) is outside region ({anchor:?})"
                );
                assert!(
                    cz >= anchor.1 && cz <= anchor.1 + 1,
                    "column ({cx}, {cz}) is outside region ({anchor:?})"
                );
                assert!(
                    ring(cx, cz) > cutoff && ring(cx, cz) <= radius,
                    "partial column ({cx}, {cz}) is not a leftover ring column"
                );
                assert!(
                    !complete.contains(&(cx, cz)),
                    "partial column ({cx}, {cz}) overlaps a complete region"
                );
                assert!(
                    partial.insert((cx, cz)),
                    "column ({cx}, {cz}) is covered by two clipped regions"
                );
            }
        }
    }
}

#[test]
fn clipped_lod1_mesh_is_air_outside_its_mask() {
    let mut columns = BTreeMap::new();
    let stack: [section::Section; COLUMN_SECTIONS] =
        std::array::from_fn(|_| section::Section::uniform(1));
    columns.insert((0, 0), Arc::new(stack));
    let masks = BTreeMap::from([((0, 0), BTreeSet::from([(0, 0)]))]);
    let regions =
        demo_coarse_regions_masked(&columns, &demo_materials(), &content::v1_default(), &masks);
    let sections = regions.get(&(0, 0)).expect("the clipped region meshes");
    assert!(!sections.is_empty());
    for section in sections {
        assert_eq!(section.scale, 2);
        for quad in section
            .mesh
            .opaque
            .iter()
            .chain(&section.mesh.cutout)
            .chain(&section.mesh.water)
        {
            assert!(quad.x() <= 16, "the missing +x column stays air");
            assert!(quad.z() <= 16, "the missing +z column stays air");
        }
    }
}

#[test]
fn coarse_bands_follow_a_smaller_ring() {
    let columns = |radius: i32| {
        let mut columns = BTreeMap::new();
        for x in -radius..=radius {
            for z in -radius..=radius {
                let stack: [section::Section; COLUMN_SECTIONS] =
                    std::array::from_fn(|_| section::Section::uniform(0));
                columns.insert((x, z), Arc::new(stack));
            }
        }
        columns
    };

    // Default 12: complete squares at base 8 and -12 are far.
    let bands12 = coarse_bands(&columns(12), (0, 0), 12);
    for base in [(8, 8), (8, -12), (-12, 8), (-12, -12)] {
        assert!(bands12.far.contains(&base), "missing far base {base:?}");
    }
    assert!(
        !bands12.coarse.is_empty(),
        "the middle band still owns regions"
    );
    // 2r+1 positive-rim bases plus the seven bases that clip the negative
    // edge of the LOD0 square (x = -4 / z = -4 at the default cutoff).
    assert_eq!(
        bands12.partial.len(),
        32,
        "radius 12 rim and LOD0-boundary clipped bases"
    );

    // Radius 8: no 4x4 square is both complete and beyond the LOD1 radius.
    let bands8 = coarse_bands(&columns(8), (0, 0), 8);
    assert!(
        bands8.far.is_empty(),
        "LOD2 has no complete region at radius 8"
    );
    assert!(!bands8.coarse.is_empty(), "the ring falls back to LOD1");
    assert_eq!(
        bands8.partial.len(),
        24,
        "radius 8 rim and LOD0-boundary clipped bases"
    );

    let bands16 = coarse_bands(&columns(16), (0, 0), 16);
    assert_eq!(
        bands16.partial.len(),
        40,
        "radius 16 rim and LOD0-boundary clipped bases"
    );
}

#[test]
fn the_demo_tile_is_lod_selected_at_spawn() {
    let (mut demo, replay) =
        build_terrain_demo_extent(0x5EED, IVec2::new(0, 0), 0, 3, 0, 3, &[], &[], 0)
            .expect("the one-tile demo builds");
    assert_eq!(replay.keys, Vec::<section::SectionKey>::new());
    assert_eq!(
        demo.columns.keys().min(),
        Some(&(0, 0)),
        "the tile is one 128-block generation unit"
    );
    assert_eq!(demo.columns.keys().max(), Some(&(3, 3)));
    let fine: usize = demo.fine.values().map(Vec::len).sum();
    demo.fine_radius = 0;
    demo.select_lod((0, 0));
    assert!(
        demo.sections.len() < fine,
        "the far corner is coarse at spawn: {} active vs {fine} fine",
        demo.sections.len(),
    );
}

/// A column whose lowest section is a flat stone slab topped at `top`.
/// A stone slab whose top block is at `top`.
fn flat_slab(top: i32) -> section::Section {
    let mut slab = section::Section::uniform(0);
    for y in 0..=top {
        for z in 0..32 {
            for x in 0..32 {
                slab.set(x, u32::try_from(y).unwrap_or(0), z, 1);
            }
        }
    }
    slab
}

/// A column whose lowest section is [`flat_slab`] and the rest is air.
fn flat_stack(top: i32) -> Arc<[section::Section; COLUMN_SECTIONS]> {
    let mut stack: [section::Section; COLUMN_SECTIONS] =
        std::array::from_fn(|_| section::Section::uniform(0));
    stack[0] = flat_slab(top);
    Arc::new(stack)
}

/// A LOD0 section for [`flat_slab`], meshed against air on every side.
fn flat_fine_section(top: i32, origin: [i32; 3]) -> DemoSection {
    let slab = flat_slab(top);
    let air = section::Section::uniform(0);
    let owners = [Some(&air); 26];
    let light = crate::padded::PaddedLight::new(15, 0);
    let mesh = mesher2::mesh_from_neighbours(
        &slab,
        &owners,
        &light,
        &demo_materials(),
        mesher2::MeshLod::Lod0,
    );
    DemoSection {
        origin,
        scale: 1,
        mesh,
    }
}

#[test]
fn fine_skirt_edges_only_face_the_coarse_band() {
    let edges = fine_skirt_edges((0, 0), (0, 0), 12);
    assert!(
        edges.is_empty(),
        "a column fully inside LOD0 has no coarse side"
    );

    let east = fine_skirt_edges((3, 0), (0, 0), 12);
    assert!(east.east() && !east.west() && !east.north() && !east.south());

    let north = fine_skirt_edges((0, -3), (0, 0), 12);
    assert!(north.north() && !north.south() && !north.east() && !north.west());

    let corner = fine_skirt_edges((3, 3), (0, 0), 12);
    assert!(corner.east() && corner.south());
    assert!(!corner.west() && !corner.north());

    // The opposite corner flips the two open sides.
    let opposite = fine_skirt_edges((-3, -3), (0, 0), 12);
    assert!(opposite.west() && opposite.north());
    assert!(!opposite.east() && !opposite.south());

    // A short ring has no coarse band at all.
    assert!(fine_skirt_edges((3, 0), (0, 0), 3).is_empty());
}

#[test]
fn a_boundary_fine_section_skirts_only_the_coarse_side() {
    let materials = demo_materials();
    let pack = content::v1_default();
    let fine_top = 9;
    let coarse_top = 8;
    let mut columns = BTreeMap::new();
    columns.insert((4, 0), flat_stack(coarse_top - 1));
    let mut section = flat_fine_section(fine_top, [3 * 32, 0, 0]);
    let before = section.mesh.opaque.len();
    let edges = fine_skirt_edges((3, 0), (0, 0), 12);

    let added = skirt_fine_section(&columns, &materials, &pack, &mut section, (3, 0), edges);

    assert_eq!(added, 2, "the coarse top is two blocks below the fine edge");
    assert_eq!(section.mesh.opaque.len(), before + 2);
    assert!(section.mesh.skirts.east());
    assert!(!section.mesh.skirts.west());
    assert!(!section.mesh.skirts.north());
    assert!(!section.mesh.skirts.south());
    for quad in &section.mesh.opaque[before..] {
        assert_eq!(quad.normal(), 0, "the stitch hangs on the east plane");
    }
}

#[test]
fn a_fine_section_inside_lod0_gets_no_skirts() {
    let materials = demo_materials();
    let pack = content::v1_default();
    let mut columns = BTreeMap::new();
    for cell in [(1, 0), (0, 1), (0, -1), (-1, 0)] {
        columns.insert(cell, flat_stack(20));
    }
    let mut section = flat_fine_section(9, [32, 0, 0]);
    let raw = section.mesh.opaque.clone();
    let edges = fine_skirt_edges((1, 0), (0, 0), 12);
    assert!(edges.is_empty());

    let added = skirt_fine_section(&columns, &materials, &pack, &mut section, (1, 0), edges);

    assert_eq!(added, 0);
    assert!(section.mesh.skirts.is_empty());
    assert_eq!(section.mesh.opaque, raw, "interior geometry is untouched");
}

#[test]
fn skirt_depth_covers_the_actual_coarse_step() {
    let materials = demo_materials();
    let pack = content::v1_default();
    let cases = [
        (9, 12, 0_u32, "coarse above the fine edge"),
        (9, 10, 0, "flush coarse edge"),
        (8, 8, 1, "one-block step"),
        (9, 8, 2, "two-block step"),
        (9, 4, 2, "deeper step capped at SKIRT_DEPTH"),
    ];
    for (fine_top, coarse_top, expected, why) in cases {
        let mut columns = BTreeMap::new();
        columns.insert((4, 0), flat_stack(coarse_top - 1));
        let mut section = flat_fine_section(fine_top, [3 * 32, 0, 0]);
        let before = section.mesh.opaque.len();
        let added = skirt_fine_section(
            &columns,
            &materials,
            &pack,
            &mut section,
            (3, 0),
            mesher2::SkirtEdges::EAST,
        );
        assert_eq!(added, expected, "{why}");
        assert_eq!(
            section.mesh.opaque.len(),
            before + usize::try_from(expected).unwrap()
        );
        let lowest = section.mesh.opaque[before..]
            .iter()
            .map(|quad| quad.y())
            .min();
        if expected > 0 {
            assert_eq!(
                lowest,
                Some(u8::try_from(fine_top).unwrap() - u8::try_from(expected).unwrap()),
                "{why}: the strip starts under the step"
            );
        }
    }
}

#[test]
fn the_demo_stitches_the_lod0_boundary_only() {
    let demo = build_terrain_demo(0x5EED, IVec2::new(0, 0), 4).expect("the v1 terrain builds");
    let sections = |x: i32, z: i32| {
        demo.sections
            .iter()
            .filter(move |section| {
                section.scale == 1 && section.origin[0] == x * 32 && section.origin[2] == z * 32
            })
            .collect::<Vec<_>>()
    };

    let boundary = sections(3, 0);
    assert!(
        !boundary.is_empty(),
        "the boundary column is active at LOD0"
    );
    assert!(
        boundary.iter().any(|section| section.mesh.skirts.east()),
        "the coarse-facing east side is stitched"
    );
    assert!(
        boundary.iter().all(|section| !section.mesh.skirts.west()),
        "the fine-owned west side keeps no skirt"
    );

    let interior = sections(1, 0);
    assert!(
        !interior.is_empty(),
        "the interior column is active at LOD0"
    );
    assert!(
        interior
            .iter()
            .all(|section| section.mesh.skirts.is_empty()),
        "a column fully inside LOD0 has no stitch geometry"
    );
}
