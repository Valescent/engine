//! The incremental light and mesh rebuild: one block change relights the 3x3
//! columns it can reach and remeshes the sections its 26-neighbour window
//! touches.
//!
//! The patch is shared read-only with the rebuild, which returns the changed
//! light and section patches for the caller to install.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use glam::IVec3;

use crate::description;
use crate::light2;
use crate::mesher2;
use crate::terrain_section as section;

use super::{COLUMN_SECTIONS, demo_light_table, demo_section};

/// One block change fed to an incremental rebuild.
pub(crate) struct DemoEdit {
    pub(crate) cell: IVec3,
}

/// One section in a rebuild result; `mesh: None` removes it.
pub(crate) struct DemoPatch {
    pub(crate) origin: [i32; 3],
    pub(crate) mesh: Option<mesher2::SectionMesh>,
}

/// One finished background rebuild of the patch (`L3.3`).
pub(crate) struct DemoRebuild {
    pub(crate) light: BTreeMap<(i32, i32), light2::ColumnLight>,
    pub(crate) patches: BTreeMap<(i32, i32), Vec<DemoPatch>>,
    /// Columns the edits touched, for the coarse-region refresh.
    pub(crate) columns: BTreeSet<(i32, i32)>,
    pub(crate) light_time: std::time::Duration,
    pub(crate) mesh_time: std::time::Duration,
}

/// Repairs light and remeshes only what an edit can reach.
///
/// Light changes stay within `LIGHT_LEVELS` laterally, so only the 3x3 columns
/// around an edit are relit; within a column the edit's vertical span is
/// repaired by [`light2::ColumnLight::relight_column`]. A section is remeshed
/// when its light changed or when the edit touched its geometry.
#[allow(clippy::too_many_lines)]
pub(crate) fn demo_rebuild(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    previous_light: &BTreeMap<(i32, i32), light2::ColumnLight>,
    materials: &mesher2::MaterialProps,
    edits: &[DemoEdit],
) -> DemoRebuild {
    let size = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
    let mut light = previous_light.clone();
    let mut affected_columns = BTreeSet::new();
    let mut queued = BTreeSet::new();
    for edit in edits {
        let (cx, cz) = (edit.cell.x.div_euclid(size), edit.cell.z.div_euclid(size));
        for dz in -1..=1_i32 {
            for dx in -1..=1_i32 {
                affected_columns.insert((cx + dx, cz + dz));
            }
        }
        // Faces and AO read the 26 neighbours of the changed cell, so only the
        // sections containing that block neighbourhood can change geometry.
        // An interior block costs one section, not a 3x3x3 window.
        for dy in -1..=1_i32 {
            for dz in -1..=1_i32 {
                for dx in -1..=1_i32 {
                    let x = edit.cell.x + dx;
                    let y = edit.cell.y + dy;
                    let z = edit.cell.z + dz;
                    queued.insert((x.div_euclid(size), y.div_euclid(size), z.div_euclid(size)));
                }
            }
        }
    }

    let started = std::time::Instant::now();
    let table = demo_light_table();
    for &(sx, sz) in &affected_columns {
        let Some(stack) = columns.get(&(sx, sz)) else {
            continue;
        };
        let mut borders: [Option<&[section::Section; COLUMN_SECTIONS]>; 8] = [None; 8];
        let mut slot = 0;
        for dz in -1..=1_i32 {
            for dx in -1..=1_i32 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                borders[slot] = columns.get(&(sx + dx, sz + dz)).map(|stack| &**stack);
                slot += 1;
            }
        }
        let view = light2::ColumnView::new(stack, borders);

        let mut owns_edit = false;
        let mut low = i32::MAX;
        let mut high = i32::MIN;
        for edit in edits {
            let (ex, ez) = (edit.cell.x.div_euclid(size), edit.cell.z.div_euclid(size));
            if (ex, ez) == (sx, sz) {
                owns_edit = true;
            }
            low = low.min(edit.cell.y);
            high = high.max(edit.cell.y);
        }
        if high < low {
            continue;
        }
        let (y_lo, y_hi) = if owns_edit {
            // A changed block moves the sky seed for every cell below it.
            let section_top = high - high.rem_euclid(size);
            (0, section_top + size - 1 + light2::RELIGHT_RADIUS)
        } else {
            // A neighbour keeps its sky seed; only lateral light moves.
            (low - light2::RELIGHT_RADIUS, high + light2::RELIGHT_RADIUS)
        };
        if let Some(entry) = light.get_mut(&(sx, sz)) {
            entry.relight_span_view(&view, &table, y_lo, y_hi);
        }
    }
    let light_time = started.elapsed();

    for &(sx, sz) in &affected_columns {
        let (Some(before), Some(after)) = (previous_light.get(&(sx, sz)), light.get(&(sx, sz)))
        else {
            continue;
        };
        for sy in 0..COLUMN_SECTIONS {
            if before.section_differs(after, sy) {
                queued.insert((sx, i32::try_from(sy).unwrap_or(0), sz));
            }
        }
    }

    let mesh_started = std::time::Instant::now();
    let mut patches: BTreeMap<(i32, i32), Vec<DemoPatch>> = BTreeMap::new();
    for &(sx, sy, sz) in &queued {
        let Ok(sy) = usize::try_from(sy) else {
            continue;
        };
        if sy >= COLUMN_SECTIONS || !columns.contains_key(&(sx, sz)) {
            continue;
        }
        let origin = [sx * size, i32::try_from(sy).unwrap_or(0) * size, sz * size];
        let mesh = demo_section(columns, &light, materials, sx, sz, sy).map(|section| section.mesh);
        patches
            .entry((sx, sz))
            .or_default()
            .push(DemoPatch { origin, mesh });
    }
    for list in patches.values_mut() {
        list.sort_by_key(|patch| patch.origin);
    }
    DemoRebuild {
        light,
        patches,
        columns: affected_columns,
        light_time,
        mesh_time: mesh_started.elapsed(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Instant;

    use glam::{IVec2, IVec3};

    use crate::content;
    use crate::mesher2;
    use crate::padded;
    use crate::pipeline;
    use crate::terrain_section as section;

    use super::super::content::demo_materials;
    use super::super::lod::demo_coarse;
    use super::super::patch::{
        TerrainDemo, demo_column_light, demo_fine_all, demo_section_light, worker_count,
    };
    use super::super::persist::{build_terrain_demo, demo_columns_extent};

    use super::*;

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
}
