//! The v1 terrain demo runtime: resident columns, LOD bands, the column ring,
//! edits, and the save/replay wiring.
//!
//! Nothing in this tree names `winit` or `wgpu`; the app translates the demo's
//! state into renderer submissions.

mod edit;
mod lod;
mod patch;
mod persist;
mod stream;

pub(crate) use edit::{
    DemoEdit, DemoPatch, DemoRebuild, demo_rebuild, queue_demo_edit, section_key,
};
pub(crate) use lod::coarse_bands;
#[cfg(test)]
pub(crate) use lod::{demo_coarse, demo_coarse_regions_masked, demo_far_regions};
#[cfg(test)]
pub(crate) use patch::demo_section_light;
pub(crate) use patch::{
    DemoSection, PartialBuild, TerrainDemo, demo_fine_column, demo_light_table, demo_section,
    worker_count,
};
pub(crate) use persist::{RepairPrompt, replay_saved_sections};
#[cfg(test)]
pub(crate) use persist::{
    build_terrain_demo, build_terrain_demo_extent, build_terrain_demo_saved, demo_columns_extent,
};
pub(crate) use stream::{
    BuildJob, COLUMN_NEIGHBOURS, CoarseBuilt, CoarseJob, GenerateJob, PoolJob, PoolResult,
    RingStream, build_margin, build_window_ready, coarse_snapshot_intact, neighbourhood_intact,
    snapshot_around,
};
#[cfg(test)]
pub(crate) use stream::{ColumnMap, build_column, relight_border};

pub(crate) const TERRAIN_DEMO_ENV: &str = "TERRAIN_DEMO";
/// Directory for the v1 terrain demo's saved world (`A16`).
pub(crate) const TERRAIN_SAVE_DIR: &str = "world/terrain-demo";
/// Seed for the stress scene, which has no generated world seed.
pub(crate) const TERRAIN_DEMO_STRESS_SEED: u64 = 0x7E44_5EED;
/// Fine LOD radius in columns. Columns beyond this ring are drawn as LOD1.
const TERRAIN_DEMO_FINE_RADIUS: i32 = 2;
/// Fine meshes are kept one ring beyond the fine radius, so a column change
/// does not leave a gap while the new fine mesh builds.
const TERRAIN_DEMO_FINE_MARGIN: i32 = 1;
/// Chebyshev radius drawn at LOD0: the fine radius plus its transition buffer.
const TERRAIN_DEMO_FINE_CUTOFF: i32 = TERRAIN_DEMO_FINE_RADIUS + TERRAIN_DEMO_FINE_MARGIN;
/// Outer radius of the LOD1 band in columns. A complete LOD2 region is a
/// `4x4` column square entirely beyond this radius.
///
/// The radius is one short of the first 4-aligned LOD2 base (`8`), so every
/// complete LOD2 region inside the ring is clear of the fine transition. At the
/// spec cap (`render_distance_chunks = 16`) LOD2 covers the outer half of the
/// 33-column ring (columns `8..15` and `-16..-9`); at the default 12 the `8..11`
/// band still forms complete far squares; at 8 there is no complete far square
/// and the middle band falls back to LOD1.
const TERRAIN_DEMO_LOD1_RADIUS: i32 = 7;
/// Minimum interval between coarse LOD uploads, in milliseconds.
pub(crate) const TERRAIN_DEMO_UPLOAD_MS: u64 = 100;
/// Minimum interval between whole-ring coarse rebuilds, in milliseconds. LOD2
/// is rebuilt by the same job, so it shares this throttle.
pub(crate) const TERRAIN_DEMO_COARSE_MS: u64 = 250;

/// Sections per column as a `usize`, for array types (`K1.5`).
pub(crate) const COLUMN_SECTIONS: usize = crate::description::SECTIONS_PER_COLUMN as usize;

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::content;
    use crate::description;
    use crate::editlog;
    use crate::hydro;
    use crate::light2;
    use crate::mesher2;
    use crate::padded;
    use crate::pipeline;
    use crate::player::BlockSource;
    use crate::raycast::raycast_where;
    use crate::renderer;
    use crate::section;
    use crate::world_save;
    use glam::{IVec2, IVec3, Vec3};

    use super::patch::{demo_column_light, demo_fine_all, demo_materials};

    use super::*;
    #[test]
    fn demo_materials_map_each_face_from_the_pack() {
        let pack = content::v1_default();
        let props = demo_materials();
        let tiles = |name: &str| {
            let material = props.material(pack.id(name).expect("the pack holds the block"));
            (
                material.texture_top,
                material.texture_side,
                material.texture_bottom,
            )
        };

        assert_eq!(tiles("core:grass"), (2, 3, 1));
        assert_eq!(tiles("core:wood"), (5, 6, 5));
        assert_eq!(tiles("core:stone"), (0, 0, 0));
        assert_eq!(tiles("core:sand"), (10, 10, 10));
    }

    #[test]
    fn mesher_reads_an_identity_light_field() {
        // Decisive split: if a full-sky PaddedLight still yields sky 0, the
        // mesher indexes light wrongly; if it yields 15, the light field is bad.
        let demo = build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
        let props = demo_materials();
        let light = padded::PaddedLight::new(15, 0);
        let mut seen = 0;
        for (&(sx, sz), stack) in &demo.columns {
            for (sy, section) in stack.iter().enumerate() {
                let sy_i32 = i32::try_from(sy).unwrap_or(0);
                let neighbours: [Option<&section::Section>; 26] =
                    padded::NEIGHBOUR_ORDER.map(|(dx, dy, dz)| {
                        let ny = i32::try_from(sy).ok()? + dy;
                        demo.columns
                            .get(&(sx + dx, sz + dz))?
                            .get(usize::try_from(ny).ok()?)
                    });
                let mesh = mesher2::mesh_from_neighbours(
                    section,
                    &neighbours,
                    &light,
                    &props,
                    mesher2::MeshLod::Lod0,
                );
                for quad in &mesh.opaque {
                    assert_eq!(quad.sky(), 15, "identity light must read as sky 15");
                    seen += 1;
                }
                let _ = sy_i32;
            }
        }
        assert!(seen > 0, "the fixture emits quads");
    }

    #[test]
    fn section_light_uses_the_neighbour_on_the_border() {
        let demo = build_terrain_demo(0x5EED, IVec2::new(2, -3), 1).expect("the v1 terrain builds");
        let lit = demo_column_light(&demo.columns);
        let mut checked = false;
        for &(sx, sz) in demo.columns.keys() {
            let Some(neighbour) = lit.get(&(sx + 1, sz)) else {
                continue;
            };
            let sy = 4_usize;
            let padded = demo_section_light(&lit, sx, sz, sy);
            let base = i32::try_from(sy).unwrap_or(0) * 32;
            for y in 0..32 {
                for z in 0..32 {
                    // Padded column 32 is the first cell of the +x neighbour.
                    assert_eq!(
                        padded.get(32, y, z).0,
                        neighbour.sky(0, base + y, z),
                        "border sky must come from the neighbour, not the mirror"
                    );
                }
            }
            checked = true;
            break;
        }
        assert!(checked, "the fixture has a column with a +x neighbour");
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn every_exposed_face_is_covered_once() {
        // `M1.1`: a greedy mesh must cover each exposed face exactly once. A miss
        // is a pinhole (sky through a wall); a double is coplanar z-fighting. The
        // grid is per face plane, because two walls at different `x` share a
        // `(y, z)`.
        const FACES: [(i32, i32, i32); 6] = [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ];
        let demo = build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
        let props = demo_materials();
        let mut holes = 0_u32;
        let mut doubles = 0_u32;
        let mut samples: Vec<String> = Vec::new();
        for (&(sx, sz), stack) in &demo.columns {
            for (sy, _) in stack.iter().enumerate() {
                let sy_i32 = i32::try_from(sy).unwrap_or(0);
                let mut exposed = vec![false; 6 * 32 * 1024];
                for y in 0..32 {
                    for z in 0..32 {
                        for x in 0..32 {
                            let cell = IVec3::new(sx * 32 + x, sy_i32 * 32 + y, sz * 32 + z);
                            let id = demo.block_at(cell).unwrap_or(0);
                            if !props.material(id).opaque {
                                continue;
                            }
                            for (face, (dx, dy, dz)) in FACES.iter().enumerate() {
                                if face == 3 && cell.y == 0 {
                                    continue;
                                }
                                let neighbour = cell + IVec3::new(*dx, *dy, *dz);
                                let column =
                                    (neighbour.x.div_euclid(32), neighbour.z.div_euclid(32));
                                if !demo.columns.contains_key(&column) {
                                    continue;
                                }
                                if props.material(demo.block_at(neighbour).unwrap_or(0)).opaque {
                                    continue;
                                }
                                let (plane, index) = match face {
                                    0 | 1 => (x, z + y * 32),
                                    2 | 3 => (y, x + z * 32),
                                    _ => (z, x + y * 32),
                                };
                                let plane = usize::try_from(plane).unwrap_or(0);
                                let index = usize::try_from(index).unwrap_or(0);
                                exposed[(face * 32 + plane) * 1024 + index] = true;
                            }
                        }
                    }
                }
                let Some(section) = demo
                    .sections
                    .iter()
                    .find(|section| section.origin == [sx * 32, sy_i32 * 32, sz * 32])
                else {
                    let count = exposed.iter().filter(|seen| **seen).count();
                    if count > 0 {
                        eprintln!("missing section ({sx},{sy},{sz}) with {count} exposed");
                    }
                    holes += u32::try_from(count).unwrap_or(0);
                    continue;
                };
                let mut covered = vec![0_u16; 6 * 32 * 1024];
                for quad in &section.mesh.opaque {
                    let face = usize::from(quad.normal());
                    if face >= 6 {
                        continue;
                    }
                    let (ow, oh) = (u32::from(quad.w1()) + 1, u32::from(quad.h1()) + 1);
                    let (plane, ou, ov) = match face {
                        0 | 1 => (
                            u32::from(quad.x()),
                            u32::from(quad.z()),
                            u32::from(quad.y()),
                        ),
                        2 | 3 => (
                            u32::from(quad.y()),
                            u32::from(quad.x()),
                            u32::from(quad.z()),
                        ),
                        _ => (
                            u32::from(quad.z()),
                            u32::from(quad.x()),
                            u32::from(quad.y()),
                        ),
                    };
                    for v in ov..ov + oh {
                        for u in ou..ou + ow {
                            let index = usize::try_from(u + v * 32).unwrap_or(0);
                            if index < 1024 {
                                let plane_index = (face * 32 + usize::try_from(plane).unwrap_or(0))
                                    * 1024
                                    + index;
                                covered[plane_index] += 1;
                            }
                        }
                    }
                }
                for face in 0..6 {
                    for plane in 0..32 {
                        for index in 0..1024 {
                            let slot = (face * 32 + plane) * 1024 + index;
                            let want = exposed[slot];
                            let got = covered[slot];
                            if want && got == 0 {
                                holes += 1;
                                if samples.len() < 12 {
                                    samples.push(format!(
                                        "hole face {face} section ({sx},{sy},{sz}) plane {plane} index {index}"
                                    ));
                                }
                            } else if !want && got > 0 {
                                doubles += 1;
                                if samples.len() < 12 {
                                    samples.push(format!(
                                        "stray face {face} section ({sx},{sy},{sz}) plane {plane} index {index}"
                                    ));
                                }
                            } else if want && got > 1 {
                                doubles += 1;
                                if samples.len() < 12 {
                                    samples.push(format!(
                                        "overlap x{got} face {face} section ({sx},{sy},{sz}) plane {plane} index {index}"
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
        for sample in &samples {
            eprintln!("{sample}");
        }
        assert_eq!(holes, 0, "exposed faces with no quad");
        assert_eq!(doubles, 0, "faces covered more than once or never exposed");
    }
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
    fn lod_uses_coarse_regions_away_from_the_player() {
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(2, -3), 2).expect("the v1 terrain builds");
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
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(0, 0), 0).expect("the v1 terrain builds");
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
    fn stub_lod_ring(radius: i32) -> TerrainDemo {
        let hydro = Arc::new(hydro::WorldHydro::solve(&[0.0], 1).expect("world hydro"));
        let mut demo = TerrainDemo::empty(0, IVec2::ZERO, radius, hydro);
        let stack: [section::Section; COLUMN_SECTIONS] =
            std::array::from_fn(|_| section::Section::uniform(0));
        for x in -radius..=radius {
            for z in -radius..=radius {
                demo.columns.insert((x, z), Arc::new(stack.clone()));
            }
        }
        let cutoff = TERRAIN_DEMO_FINE_RADIUS + TERRAIN_DEMO_FINE_MARGIN;
        for &(x, z) in demo.columns.keys() {
            if x.abs().max(z.abs()) <= cutoff {
                demo.fine
                    .insert((x, z), vec![stub_section([x * 32, 0, z * 32], 1)]);
            }
        }
        let bands = coarse_bands(&demo.columns, (0, 0), radius);
        let mut bases = bands.coarse.clone();
        bases.extend(bands.partial.keys().copied());
        demo.coarse = bases
            .iter()
            .map(|&(x, z)| stub_section([x * 32, 0, z * 32], 2))
            .collect();
        demo.partial_columns = bands.partial;
        demo.far = bands
            .far
            .iter()
            .map(|&(x, z)| stub_section([x * 32, 0, z * 32], 4))
            .collect();
        demo.select_lod((0, 0));
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
            let demo = stub_lod_ring(radius);
            let coverage = active_column_coverage(&demo);
            for x in -radius..=radius {
                for z in -radius..=radius {
                    assert_eq!(
                        coverage.get(&(x, z)).copied(),
                        Some(1),
                        "radius {radius}: column ({x}, {z}) must be covered by exactly one LOD"
                    );
                }
            }
            let mut origins = BTreeSet::new();
            for section in &demo.sections {
                assert!(
                    origins.insert(section.origin),
                    "radius {radius}: duplicate active section origin {:?}",
                    section.origin
                );
            }
        }
    }
    #[test]
    fn clipped_lod1_regions_cover_only_unowned_columns() {
        let cutoff = TERRAIN_DEMO_FINE_RADIUS + TERRAIN_DEMO_FINE_MARGIN;
        for radius in [8, 12, 16] {
            let demo = stub_lod_ring(radius);
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
            for (&(x, z), columns) in &bands.partial {
                assert!(
                    columns.len() < 4,
                    "a clipped base must leave at least one cell air: ({x}, {z})"
                );
                assert!(
                    !bands.far.contains(&(x, z)),
                    "LOD2 must not be used for a partial region ({x}, {z})"
                );
                for &(cx, cz) in columns {
                    assert_eq!(
                        cx - cx.rem_euclid(2),
                        x,
                        "column ({cx}, {cz}) is outside base ({x}, {z})"
                    );
                    assert_eq!(cz - cz.rem_euclid(2), z);
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
                        "column ({cx}, {cz}) is covered by two clipped bases"
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
    #[test]
    fn saved_columns_replay_into_the_ring() {
        let mut saved = Vec::new();
        let pack = content::v1_default();
        let mut section = section::Section::uniform(0);
        let key = section::SectionKey {
            column: [0, 0],
            section: 0,
        };
        section.set(1, 1, 1, pack.id("core:stone").expect("stone"));
        saved.push(section::SavedSection {
            key,
            edited: true,
            baseline_hash: section::baseline_hash(&section::Section::uniform(0)),
            section,
            unknown_names: Vec::new(),
        });
        let replayed = replay_saved_sections(saved, &[], 0, &pack, None, None);
        assert_eq!(replayed.sections, 1);
        assert_eq!(replayed.revision, 0);
        assert!(!replayed.read_only);
        assert_eq!(
            replayed.saved[0].section.get(1, 1, 1),
            pack.id("core:stone").expect("stone")
        );
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn a_journal_frame_ahead_of_the_section_store_regenerates_it() {
        let seed = 0x5EED;
        let pack = content::v1_default();
        let terrain = pipeline::Terrain::new(seed).expect("the description compiles");
        let section = terrain.section(0, 0, 0).expect("the section generates");
        let mut cell = None;
        for y in 0..32 {
            for z in 0..32 {
                for x in 0..32 {
                    if section.get(x, y, z) != 0 {
                        cell = Some((x, y, z));
                        break;
                    }
                }
                if cell.is_some() {
                    break;
                }
            }
            if cell.is_some() {
                break;
            }
        }
        let (x, y, z) = cell.expect("the spawn section has a solid block");
        let old = pack
            .name(section.get(x, y, z))
            .expect("the block has a name");
        let record = editlog::Edit::record(
            [
                i32::try_from(x).unwrap_or(0),
                i32::try_from(y).unwrap_or(0),
                i32::try_from(z).unwrap_or(0),
            ],
            old,
            "core:air",
            1,
            0,
            0,
            0,
        )
        .expect("the edit records");
        let batch = editlog::EditBatch::new(1, 0, vec![record]);
        let replayed = replay_saved_sections(
            Vec::new(),
            std::slice::from_ref(&batch),
            0,
            &pack,
            None,
            Some(&terrain),
        );
        assert_eq!(replayed.applied, 1);
        assert_eq!(replayed.revision, 1);
        assert_eq!(replayed.sections, 1, "the missing section was regenerated");
        assert_eq!(replayed.saved[0].section.get(x, y, z), 0);
    }
    #[test]
    fn a_new_column_relights_and_remeshes_its_neighbour_border() {
        let stone: Arc<[section::Section; COLUMN_SECTIONS]> =
            Arc::new(std::array::from_fn(|_| section::Section::uniform(1)));
        let air: Arc<[section::Section; COLUMN_SECTIONS]> =
            Arc::new(std::array::from_fn(|_| section::Section::uniform(0)));
        let mut columns = ColumnMap::new();
        columns.insert((0, 0), stone);
        columns.insert((1, 0), air);
        let built = build_column(BuildJob {
            centre: (1, 0),
            full: true,
            detailed: true,
            margin: 2,
            columns,
            previous_light: BTreeMap::new(),
            fine_columns: BTreeSet::from([(0, 0)]),
            materials: Arc::new(demo_materials()),
            light_table: Arc::new(demo_light_table()),
        });

        assert!(
            built.light.contains_key(&(0, 0)),
            "the present neighbour is relit"
        );
        assert!(
            built
                .patches
                .get(&(0, 0))
                .is_some_and(|patches| !patches.is_empty()),
            "the neighbour's border sections are remeshed"
        );
        assert!(
            built.replace.contains_key(&(1, 0)),
            "the new column is meshed at LOD0"
        );
    }

    #[test]
    fn narrow_border_relight_matches_a_full_relight() {
        let seed = 0x5EED;
        let hydro = pipeline::Terrain::new(seed)
            .expect("the v1 description compiles")
            .solve_world_hydro()
            .expect("the hydrology solves");
        let columns: ColumnMap = demo_columns_extent(seed, -1, 1, -1, 1, &hydro)
            .expect("the tile generates")
            .into_iter()
            .map(|(key, stack)| (key, Arc::new(stack)))
            .collect();
        let table = demo_light_table();
        let materials = demo_materials();
        let centre = (0, 0);

        // Before the centre is generated, every neighbour sees its own edge
        // mirrored across that border. That is the state a build job starts
        // from, and the one the narrow repair must match.
        let mut previous = BTreeMap::new();
        for (&key, stack) in &columns {
            if key == centre {
                continue;
            }
            let mut borders: [Option<&[section::Section; COLUMN_SECTIONS]>; 8] = [None; 8];
            for (slot, (dx, dz)) in COLUMN_NEIGHBOURS.into_iter().enumerate() {
                let border = (key.0 + dx, key.1 + dz);
                if border == centre {
                    continue;
                }
                borders[slot] = columns.get(&border).map(|stack| &**stack);
            }
            let blocks = light2::ColumnBlocks::new((**stack).clone(), &borders);
            previous.insert(key, light2::compute_column(&blocks, &table));
        }

        let mut narrow = previous.clone();
        let mut full = previous.clone();
        let mut centre_borders: [Option<&[section::Section; COLUMN_SECTIONS]>; 8] = [None; 8];
        for (slot, (dx, dz)) in COLUMN_NEIGHBOURS.into_iter().enumerate() {
            centre_borders[slot] = columns
                .get(&(centre.0 + dx, centre.1 + dz))
                .map(|stack| &**stack);
        }
        let centre_blocks =
            light2::ColumnBlocks::new((**columns.get(&centre).unwrap()).clone(), &centre_borders);
        let centre_light = light2::compute_column(&centre_blocks, &table);
        narrow.insert(centre, centre_light.clone());
        full.insert(centre, centre_light);

        for (dx, dz) in COLUMN_NEIGHBOURS {
            let key = (centre.0 + dx, centre.1 + dz);
            narrow.insert(
                key,
                relight_border(&previous[&key], &columns, key, centre, &table),
            );
            let mut borders: [Option<&[section::Section; COLUMN_SECTIONS]>; 8] = [None; 8];
            for (slot, (nx, nz)) in COLUMN_NEIGHBOURS.into_iter().enumerate() {
                borders[slot] = columns.get(&(key.0 + nx, key.1 + nz)).map(|stack| &**stack);
            }
            let blocks =
                light2::ColumnBlocks::new((**columns.get(&key).unwrap()).clone(), &borders);
            full.insert(key, light2::compute_column(&blocks, &table));
        }

        for (dx, dz) in COLUMN_NEIGHBOURS {
            let key = (centre.0 + dx, centre.1 + dz);
            assert_eq!(
                narrow[&key], full[&key],
                "neighbour {dx},{dz} matches a full relight"
            );
            for sy in 0..COLUMN_SECTIONS {
                let narrow_mesh = demo_section(&columns, &narrow, &materials, key.0, key.1, sy)
                    .map(|section| section.mesh);
                let full_mesh = demo_section(&columns, &full, &materials, key.0, key.1, sy)
                    .map(|section| section.mesh);
                assert_eq!(
                    narrow_mesh, full_mesh,
                    "neighbour {dx},{dz} section {sy} mesh matches"
                );
            }
        }
    }

    #[test]
    fn a_build_waits_for_its_in_ring_dependency_window() {
        let air: Arc<[section::Section; COLUMN_SECTIONS]> =
            Arc::new(std::array::from_fn(|_| section::Section::uniform(0)));
        let mut columns = ColumnMap::new();
        for dx in -2..=2 {
            for dz in -2..=2 {
                columns.insert((dx, dz), Arc::clone(&air));
            }
        }
        assert!(build_window_ready(&columns, (0, 0), 2, (0, 0), 2));

        columns.remove(&(2, 0));
        assert!(
            !build_window_ready(&columns, (0, 0), 2, (0, 0), 2),
            "a missing in-ring column defers the build"
        );
        assert!(
            build_window_ready(&columns, (0, 0), 2, (0, 0), 1),
            "outside the visible ring the absence is the permanent edge"
        );
        assert!(
            build_window_ready(&columns, (0, 0), 1, (0, 0), 2),
            "a mesh-only upgrade does not read the outer ring"
        );
    }

    #[test]
    fn a_mesh_only_build_ignores_the_outer_ring() {
        let air: Arc<[section::Section; COLUMN_SECTIONS]> =
            Arc::new(std::array::from_fn(|_| section::Section::uniform(0)));
        let mut snapshot = ColumnMap::new();
        for dx in -1..=1 {
            for dz in -1..=1 {
                snapshot.insert((dx, dz), Arc::clone(&air));
            }
        }
        let mut current = snapshot.clone();
        current.insert((2, 0), Arc::clone(&air));

        assert!(neighbourhood_intact(&current, &snapshot, (0, 0), 1));
        assert!(!neighbourhood_intact(&current, &snapshot, (0, 0), 2));
    }

    #[test]
    fn a_complete_window_build_matches_a_full_rebuild() {
        let seed = 0x5EED;
        let hydro = pipeline::Terrain::new(seed)
            .expect("the v1 description compiles")
            .solve_world_hydro()
            .expect("the hydrology solves");
        let columns: ColumnMap = demo_columns_extent(seed, -2, 2, -2, 2, &hydro)
            .expect("the tile generates")
            .into_iter()
            .map(|(key, stack)| (key, Arc::new(stack)))
            .collect();
        let centre = (0, 0);
        assert!(
            build_window_ready(&columns, centre, 2, centre, 2),
            "the accepted fixture has its whole dependency window"
        );

        let materials = Arc::new(demo_materials());
        let light_table = Arc::new(demo_light_table());
        let fine_columns = COLUMN_NEIGHBOURS
            .into_iter()
            .map(|(dx, dz)| (centre.0 + dx, centre.1 + dz))
            .collect();
        let built = build_column(BuildJob {
            centre,
            full: true,
            detailed: true,
            margin: 2,
            columns: columns.clone(),
            previous_light: BTreeMap::new(),
            fine_columns,
            materials: Arc::clone(&materials),
            light_table,
        });
        let full_light = demo_column_light(&columns);

        for (dx, dz) in std::iter::once((0, 0)).chain(COLUMN_NEIGHBOURS) {
            let key = (centre.0 + dx, centre.1 + dz);
            assert_eq!(
                built.light[&key], full_light[&key],
                "column {dx},{dz} light matches a full rebuild"
            );
        }

        let expected = demo_fine_column(&columns, &full_light, &materials, centre.0, centre.1);
        let actual = &built.replace[&centre];
        assert_eq!(
            actual.len(),
            expected.len(),
            "the centre mesh has the same sections"
        );
        for (actual, expected) in actual.iter().zip(&expected) {
            assert_eq!(actual.origin, expected.origin);
            assert_eq!(actual.scale, expected.scale);
            assert_eq!(
                actual.mesh, expected.mesh,
                "the accepted centre mesh matches a full remesh"
            );
        }
    }
    #[test]
    fn demo_build_phase_split() {
        let started = Instant::now();
        let terrain = pipeline::Terrain::new(0x5EED).expect("the v1 description compiles");
        let terrain_ms = started.elapsed().as_secs_f64() * 1000.0;

        let tiled = Instant::now();
        terrain.tile(0, 0).expect("the tile builds");
        let tile_ms = tiled.elapsed().as_secs_f64() * 1000.0;

        let generated = Instant::now();
        let hydro = pipeline::Terrain::new(0x5EED)
            .expect("the v1 description compiles")
            .solve_world_hydro()
            .expect("the world hydrology solves");
        let columns: BTreeMap<_, _> = demo_columns_extent(0x5EED, 0, 3, 0, 3, &hydro)
            .expect("the tile generates")
            .into_iter()
            .map(|(key, stack)| (key, Arc::new(stack)))
            .collect();
        let generate_ms = generated.elapsed().as_secs_f64() * 1000.0;

        let lit = Instant::now();
        let light = demo_column_light(&columns);
        let light_ms = lit.elapsed().as_secs_f64() * 1000.0;

        let meshed = Instant::now();
        let materials = demo_materials();
        let fine = demo_fine_all(&columns, &light, &materials);
        let sections: usize = fine.values().map(Vec::len).sum();
        let quads: usize = fine
            .values()
            .flatten()
            .map(|section| mesher2::quad_count(&section.mesh))
            .sum();
        let mesh_ms = meshed.elapsed().as_secs_f64() * 1000.0;

        let coarse_started = Instant::now();
        let coarse = demo_coarse(&columns, &materials, &content::v1_default());
        let coarse_ms = coarse_started.elapsed().as_secs_f64() * 1000.0;

        eprintln!(
            "demo build: terrain={terrain_ms:.0}ms tile={tile_ms:.0}ms generate={generate_ms:.0}ms light={light_ms:.0}ms mesh={mesh_ms:.0}ms coarse={coarse_ms:.0}ms | {sections} fine, {} coarse, {quads} quads",
            coarse.len()
        );
        assert!(quads > 0);
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn demo_rebuild_cost_split() {
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(2, -3), 1).expect("the v1 terrain builds");
        let cell = IVec3::new(
            demo.spawn.x.floor() as i32,
            demo.spawn.y.round() as i32 - 1,
            demo.spawn.z.floor() as i32,
        );
        let block = demo.air;
        assert!(demo.set_block(cell, block));
        let rebuild = demo_rebuild(
            &demo.columns,
            &demo.light,
            &demo.materials,
            &[DemoEdit { cell }],
        );
        eprintln!(
            "rebuild: workers={} light={:.1}ms mesh={:.1}ms patches={} ({} sections at startup)",
            worker_count(),
            rebuild.light_time.as_secs_f64() * 1000.0,
            rebuild.mesh_time.as_secs_f64() * 1000.0,
            rebuild.patches.len(),
            demo.sections.len()
        );
        assert!(!rebuild.patches.is_empty());
        assert!(
            rebuild.patches.len() <= 8,
            "an interior edit touches only its section and adjacent borders, not a 3x3x3 window: {} patches",
            rebuild.patches.len()
        );
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn incremental_rebuild_matches_a_full_rebuild() {
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(2, -3), 1).expect("the v1 terrain builds");
        let center = IVec3::new(
            demo.spawn.x.floor() as i32,
            demo.spawn.y.round() as i32 - 1,
            demo.spawn.z.floor() as i32,
        );
        let above = center + IVec3::Y * 3;
        let border_x = demo.origin_column.x * 32 + 31;
        let border_z = demo.spawn.z.floor() as i32;
        let border = (0..=demo.spawn.y.round() as i32)
            .rev()
            .map(|y| IVec3::new(border_x, y, border_z))
            .find(|cell| demo.is_solid(*cell))
            .expect("the column edge has ground");
        let stone = content::v1_default()
            .id("core:stone")
            .expect("the v1 pack has stone");

        let edits = [
            (center, demo.air),
            (above, stone),
            (above, demo.air),
            (border, demo.air),
        ];
        for (index, (cell, block)) in edits.into_iter().enumerate() {
            assert!(
                demo.set_block(cell, block),
                "edit {index} changes the patch"
            );
            assert_incremental_matches_full(&mut demo, &[DemoEdit { cell }], index);
        }
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn coalesced_edits_match_a_full_rebuild() {
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(2, -3), 1).expect("the v1 terrain builds");
        let center = IVec3::new(
            demo.spawn.x.floor() as i32,
            demo.spawn.y.round() as i32 - 1,
            demo.spawn.z.floor() as i32,
        );
        let neighbour_x = center.x + 32;
        let neighbour = (0..=center.y)
            .rev()
            .map(|y| IVec3::new(neighbour_x, y, center.z))
            .find(|cell| demo.is_solid(*cell))
            .expect("the next column has ground");
        let air = demo.air;
        assert!(demo.set_block(center, air));
        assert!(demo.set_block(neighbour, air));
        let edits = [DemoEdit { cell: center }, DemoEdit { cell: neighbour }];
        assert_incremental_matches_full(&mut demo, &edits, 0);
    }

    /// Rebuilds one batch and checks it against a full relight and remesh.
    fn assert_incremental_matches_full(demo: &mut TerrainDemo, edits: &[DemoEdit], label: usize) {
        let rebuild = demo_rebuild(&demo.columns, &demo.light, &demo.materials, edits);

        let expected_light = demo_column_light(&demo.columns);
        assert_eq!(
            rebuild.light, expected_light,
            "edit {label}: the incremental repair matches a full relight"
        );

        demo.apply_rebuild(rebuild);
        let expected: BTreeMap<[i32; 3], mesher2::SectionMesh> =
            demo_fine_all(&demo.columns, &expected_light, &demo.materials)
                .into_values()
                .flatten()
                .map(|section| (section.origin, section.mesh))
                .collect();
        let actual: BTreeMap<[i32; 3], mesher2::SectionMesh> = demo
            .sections
            .iter()
            .map(|section| (section.origin, section.mesh.clone()))
            .collect();
        assert_eq!(
            actual, expected,
            "edit {label}: the incremental mesh matches a full remesh"
        );
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn a_newer_journal_frame_is_replayed_over_the_saved_section() {
        let demo = build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
        let cell = IVec3::new(
            demo.spawn.x.floor() as i32,
            demo.spawn.y.round() as i32 - 1,
            demo.spawn.z.floor() as i32,
        );
        let key = section_key(cell);
        let old = demo.block_at(cell).expect("the spawn block exists");
        let old_name = demo
            .pack
            .name(old)
            .expect("the block has a name")
            .to_owned();
        let new_name = demo.pack.name(demo.air).expect("air has a name").to_owned();
        let record =
            editlog::Edit::record([cell.x, cell.y, cell.z], old_name, new_name, 1, 0, 0, 0)
                .expect("the change is recorded");
        let batch = editlog::EditBatch::new(1, 0, vec![record]);

        let (loaded, replay) = build_terrain_demo_saved(
            demo.seed,
            demo.origin_column,
            0,
            &[],
            std::slice::from_ref(&batch),
            0,
        )
        .expect("the replayed patch builds");
        assert_eq!(loaded.block_at(cell), Some(demo.air));
        assert_eq!(replay.keys, vec![key]);
        assert_eq!(replay.revision, 1);
        assert_eq!(loaded.revision, 1);
        assert_ne!(
            loaded.baselines.get(&key).copied().unwrap_or([0; 32]),
            [0; 32],
            "the replay derives the generated baseline"
        );

        let (covered, replay) =
            build_terrain_demo_saved(demo.seed, demo.origin_column, 0, &[], &[batch], 1)
                .expect("the covered patch builds");
        assert_eq!(covered.block_at(cell), Some(old));
        assert_eq!(replay.keys.len(), 0);
        assert_eq!(replay.revision, 1);
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn saved_sections_override_generation_when_the_baseline_matches() {
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
        let cell = IVec3::new(
            demo.spawn.x.floor() as i32,
            demo.spawn.y.round() as i32 - 1,
            demo.spawn.z.floor() as i32,
        );
        let key = section_key(cell);
        let baseline = demo.baseline_for(key);
        let original = demo.block_at(cell).expect("the spawn block exists");
        let air = demo.air;
        assert!(demo.set_block(cell, air));
        let section =
            demo.columns[&(key.column[0], key.column[1])][usize::from(key.section)].clone();
        let saved = section::SavedSection {
            key,
            edited: true,
            baseline_hash: baseline,
            section,
            unknown_names: Vec::new(),
        };

        let (loaded, replay) =
            build_terrain_demo_saved(demo.seed, demo.origin_column, 0, &[saved], &[], 0)
                .expect("the saved patch builds");
        assert_eq!(loaded.block_at(cell), Some(air));
        assert_eq!(replay.keys.len(), 0);
        assert_ne!(original, air, "the generated block was solid");
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn a_saved_section_with_another_baseline_is_ignored() {
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
        let cell = IVec3::new(
            demo.spawn.x.floor() as i32,
            demo.spawn.y.round() as i32 - 1,
            demo.spawn.z.floor() as i32,
        );
        let key = section_key(cell);
        let original = demo.block_at(cell).expect("the spawn block exists");
        let air = demo.air;
        assert!(demo.set_block(cell, air));
        let section =
            demo.columns[&(key.column[0], key.column[1])][usize::from(key.section)].clone();
        let saved = section::SavedSection {
            key,
            edited: true,
            baseline_hash: [0; 32],
            section,
            unknown_names: Vec::new(),
        };

        let (loaded, replay) =
            build_terrain_demo_saved(demo.seed, demo.origin_column, 0, &[saved], &[], 0)
                .expect("the saved patch builds");
        assert_eq!(loaded.block_at(cell), Some(original));
        assert_eq!(replay.keys.len(), 0);
    }
    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn an_edit_queues_a_journal_batch_and_a_section_save() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "valescent-demo-save-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        {
            let mut demo =
                build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
            let world = world_save::WorldSave::open_or_create(&directory, demo.seed, "Demo test")
                .expect("the world opens");
            let save = world_save::SaveWorker::start(world, 8).expect("the worker starts");

            let cell = IVec3::new(
                demo.spawn.x.floor() as i32,
                demo.spawn.y.round() as i32 - 1,
                demo.spawn.z.floor() as i32,
            );
            let key = section_key(cell);
            let baseline = demo.baseline_for(key);
            let old = demo.block_at(cell).expect("the spawn block exists");
            let old_name = demo
                .pack
                .name(old)
                .expect("the block has a name")
                .to_owned();
            let new_name = demo.pack.name(demo.air).expect("air has a name").to_owned();
            assert!(demo.set_block(cell, demo.air));
            queue_demo_edit(&save, &mut demo, cell, key, baseline, old_name, new_name);
            let revision = demo.revision;
            save.shutdown().expect("the worker stops");

            let reopened =
                world_save::WorldSave::open(&directory, demo.seed).expect("the world reopens");
            assert_eq!(reopened.header.revision, revision);
            assert!(
                reopened.recovered().is_empty(),
                "the section save covers and trims the batch"
            );
            let sections = reopened.read_sections().expect("the sections read");
            assert_eq!(sections.len(), 1);
            assert_eq!(sections[0].key, key);
            let size = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
            assert_eq!(
                sections[0].section.get(
                    u32::try_from(cell.x.rem_euclid(size)).unwrap_or(0),
                    u32::try_from(cell.y.rem_euclid(size)).unwrap_or(0),
                    u32::try_from(cell.z.rem_euclid(size)).unwrap_or(0),
                ),
                demo.air
            );
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn demo_edit_breaks_a_block_and_relights() {
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
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
}
