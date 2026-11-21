//! LOD bands: LOD0 near the player, complete and clipped LOD1 in the middle,
//! and complete 4x4 LOD2 squares far out.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::content;
use crate::lod;
use crate::mesher2;
use crate::padded;
use crate::section;

use super::{COLUMN_SECTIONS, DemoSection, TERRAIN_DEMO_FINE_CUTOFF, TERRAIN_DEMO_LOD1_RADIUS};

/// Derives and meshes every complete `2x2x2` coarse group (`N1.4`, `N3.2`).
#[cfg(test)]
pub(crate) fn demo_coarse(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    materials: &mesher2::MaterialProps,
    pack: &content::ContentPack,
) -> Vec<DemoSection> {
    let mut regions = BTreeSet::new();
    for &(x, z) in columns.keys() {
        if x.rem_euclid(2) == 0 && z.rem_euclid(2) == 0 {
            regions.insert((x, z));
        }
    }
    let mut out: Vec<DemoSection> = demo_coarse_regions(columns, materials, pack, &regions)
        .into_values()
        .flatten()
        .collect();
    out.sort_by_key(|section| section.origin);
    out
}

/// The LOD1 and LOD2 region bases selected for one player column.
///
/// `partial` holds aligned `2x2` bases clipped by an already-covered column or
/// by the ring edge: the ring is `2r+1` columns wide, so the outermost positive
/// base has no `+1` neighbour and cannot be a complete LOD1 region. Each value
/// is the set of columns that region actually draws; the rest of its aligned
/// footprint is air.
#[derive(Debug, Default)]
pub(crate) struct BandRegions {
    /// Complete LOD1 `2x2` regions.
    pub(crate) coarse: BTreeSet<(i32, i32)>,
    /// Clipped LOD1 `2x2` regions and the columns they cover.
    pub(crate) partial: BTreeMap<(i32, i32), BTreeSet<(i32, i32)>>,
    /// Complete LOD2 `4x4` regions.
    pub(crate) far: BTreeSet<(i32, i32)>,
}

/// The LOD1 and LOD2 region bases for a player column.
///
/// LOD2 owns a `4x4` column square only when all 16 columns are resident and
/// every one of them is beyond [`TERRAIN_DEMO_LOD1_RADIUS`]. LOD1 regions in
/// that square are dropped, so a later selection can never draw both levels
/// over the same columns.
///
/// Every column in the visible ring (`ring <= view_radius`) is covered by
/// LOD0, a complete LOD1 region, a clipped LOD1 region, or LOD2. Two kinds of
/// column fall through the complete bands: the positive edge of a ring with an
/// even radius, and the negative edge of the fine square where an aligned
/// `2x2` base straddles the LOD0 boundary. Both get a clipped `2x2` base whose
/// mask names the columns it draws. A clipped base only ever uses its other
/// columns as air; it never overlaps a complete region, so coverage is a
/// partition, not an overlap.
pub(crate) fn coarse_bands(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    player: (i32, i32),
    view_radius: i32,
) -> BandRegions {
    let ring = |column: (i32, i32)| (column.0 - player.0).abs().max((column.1 - player.1).abs());
    let mut far = BTreeSet::new();
    for &(x, z) in columns.keys() {
        if x.rem_euclid(4) != 0 || z.rem_euclid(4) != 0 {
            continue;
        }
        let complete = (0..4).all(|dx| (0..4).all(|dz| columns.contains_key(&(x + dx, z + dz))));
        let clear =
            (0..4).all(|dx| (0..4).all(|dz| ring((x + dx, z + dz)) > TERRAIN_DEMO_LOD1_RADIUS));
        if complete && clear {
            far.insert((x, z));
        }
    }

    let mut coarse = BTreeSet::new();
    for &(x, z) in columns.keys() {
        if x.rem_euclid(2) != 0 || z.rem_euclid(2) != 0 {
            continue;
        }
        if far.contains(&(x.div_euclid(4) * 4, z.div_euclid(4) * 4)) {
            continue;
        }
        if columns.contains_key(&(x + 1, z))
            && columns.contains_key(&(x, z + 1))
            && columns.contains_key(&(x + 1, z + 1))
        {
            coarse.insert((x, z));
        }
    }

    // Footprints of the bands that `select_lod` will actually activate. A
    // complete LOD1 base that touches LOD0 is suppressed, so its outside
    // columns stay leftover and must be covered by a clipped region.
    let mut covered = BTreeSet::new();
    for &(x, z) in &far {
        for dx in 0..4 {
            for dz in 0..4 {
                covered.insert((x + dx, z + dz));
            }
        }
    }
    for &(x, z) in &coarse {
        let clear =
            (0..2).all(|dx| (0..2).all(|dz| ring((x + dx, z + dz)) > TERRAIN_DEMO_FINE_CUTOFF));
        if clear {
            for dx in 0..2 {
                for dz in 0..2 {
                    covered.insert((x + dx, z + dz));
                }
            }
        }
    }

    let mut leftover = BTreeSet::new();
    for x in player.0 - view_radius..=player.0 + view_radius {
        for z in player.1 - view_radius..=player.1 + view_radius {
            let column = (x, z);
            if ring(column) > TERRAIN_DEMO_FINE_CUTOFF
                && !covered.contains(&column)
                && columns.contains_key(&column)
            {
                leftover.insert(column);
            }
        }
    }

    let mut partial: BTreeMap<(i32, i32), BTreeSet<(i32, i32)>> = BTreeMap::new();
    for column in leftover {
        let base = (
            column.0 - column.0.rem_euclid(2),
            column.1 - column.1.rem_euclid(2),
        );
        partial.entry(base).or_default().insert(column);
    }

    BandRegions {
        coarse,
        partial,
        far,
    }
}

/// A built coarse band and the masks of its clipped regions.
pub(crate) struct LodSets {
    /// Complete LOD1 sections for the middle band.
    pub(crate) coarse: Vec<DemoSection>,
    /// Clipped LOD1 sections, rebuilt only when their masks change.
    pub(crate) partial_sections: Vec<DemoSection>,
    /// LOD2 sections for the far band.
    pub(crate) far: Vec<DemoSection>,
    /// Columns drawn by each clipped LOD1 base.
    pub(crate) partial: BTreeMap<(i32, i32), BTreeSet<(i32, i32)>>,
}

/// Builds the LOD1 and LOD2 section sets for `player`.
///
/// `include_partial` controls the costly clipped-region pass. The live ring
/// keeps its clipped sections and only rebuilds them when [`coarse_bands`]
/// reports a different mask; a full rebuild (startup, repair) always asks for
/// them.
pub(crate) fn demo_lod_sets(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    materials: &mesher2::MaterialProps,
    pack: &content::ContentPack,
    player: (i32, i32),
    view_radius: i32,
    include_partial: bool,
) -> LodSets {
    let bands = coarse_bands(columns, player, view_radius);
    let mut masks: BTreeMap<(i32, i32), BTreeSet<(i32, i32)>> = BTreeMap::new();
    for &(x, z) in &bands.coarse {
        if bands.partial.contains_key(&(x, z)) {
            continue;
        }
        masks.insert(
            (x, z),
            BTreeSet::from([(x, z), (x + 1, z), (x, z + 1), (x + 1, z + 1)]),
        );
    }
    let mut coarse: Vec<DemoSection> = demo_coarse_regions_masked(columns, materials, pack, &masks)
        .into_values()
        .flatten()
        .collect();
    let (partial_sections, partial) = if include_partial {
        let sections: Vec<DemoSection> =
            demo_coarse_regions_masked(columns, materials, pack, &bands.partial)
                .into_values()
                .flatten()
                .collect();
        (sections, bands.partial)
    } else {
        (Vec::new(), BTreeMap::new())
    };
    let mut far: Vec<DemoSection> = demo_far_regions(columns, materials, pack, &bands.far)
        .into_values()
        .flatten()
        .collect();
    coarse.sort_by_key(|section| section.origin);
    far.sort_by_key(|section| section.origin);
    LodSets {
        coarse,
        partial_sections,
        far,
        partial,
    }
}

/// Meshes the coarse sections of `regions`.
///
/// A coarse region is the 2x2 column group whose base is even; its sections are
/// `64` blocks tall, so six cover the column. Only regions with all four columns
/// produce sections. Every region in `regions` is rebuilt; the surrounding ring
/// is derived too, so a coarse mesh culls against a real neighbour when present
/// and falls back to air at the ring edge.
#[cfg(test)]
pub(crate) fn demo_coarse_regions(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    materials: &mesher2::MaterialProps,
    pack: &content::ContentPack,
    regions: &BTreeSet<(i32, i32)>,
) -> BTreeMap<(i32, i32), Vec<DemoSection>> {
    let masks: BTreeMap<(i32, i32), BTreeSet<(i32, i32)>> = regions
        .iter()
        .map(|&(x, z)| {
            (
                (x, z),
                BTreeSet::from([(x, z), (x + 1, z), (x, z + 1), (x + 1, z + 1)]),
            )
        })
        .collect();
    demo_coarse_regions_masked(columns, materials, pack, &masks)
}

/// Meshes `2x2` regions, treating every column outside its mask as air.
///
/// A mask names the columns a region actually draws. A clipped mask leaves one
/// or more of the four cells air, so the aligned `2x2` footprint still covers a
/// rim or LOD0-boundary column without drawing over a neighbour that another
/// band already owns.
pub(crate) fn demo_coarse_regions_masked(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    materials: &mesher2::MaterialProps,
    pack: &content::ContentPack,
    masks: &BTreeMap<(i32, i32), BTreeSet<(i32, i32)>>,
) -> BTreeMap<(i32, i32), Vec<DemoSection>> {
    if masks.is_empty() {
        return BTreeMap::new();
    }
    let water = pack.id("core:water").unwrap_or(0);
    let scatter: Vec<section::BlockId> = pack
        .records()
        .iter()
        .filter(|record| record.render_shape == content::RenderShape::CrossQuad)
        .filter_map(|record| pack.id(&record.name))
        .collect();
    let air = section::Section::uniform(0);

    let mut expanded = BTreeSet::new();
    for &(x, z) in masks.keys() {
        for dx in -1..=1_i32 {
            for dz in -1..=1_i32 {
                expanded.insert((x + 2 * dx, z + 2 * dz));
            }
        }
    }

    let mut derived: BTreeMap<(i32, i32, i32), section::Section> = BTreeMap::new();
    for &(x, z) in &expanded {
        let Some(mask) = masks.get(&(x, z)) else {
            // A neighbour outside the requested set is derived whole when it
            // is complete, only so the requested mesh culls against real data.
            if (0..2).all(|dx| (0..2).all(|dz| columns.contains_key(&(x + dx, z + dz)))) {
                for j in 0..(COLUMN_SECTIONS / 2) {
                    let octants: [&section::Section; 8] = std::array::from_fn(|slot| {
                        let dx = i32::try_from(slot & 1).unwrap_or(0);
                        let dz = i32::try_from((slot >> 1) & 1).unwrap_or(0);
                        let dy = (slot >> 2) & 1;
                        &columns[&(x + dx, z + dz)][2 * j + dy]
                    });
                    if let Ok(coarse) = lod::downsample(&octants, water, &scatter) {
                        derived.insert((x, i32::try_from(j).unwrap_or(0), z), coarse);
                    }
                }
            }
            continue;
        };
        if !mask.iter().all(|column| columns.contains_key(column)) {
            continue;
        }
        for j in 0..(COLUMN_SECTIONS / 2) {
            let octants: [&section::Section; 8] = std::array::from_fn(|slot| {
                let dx = i32::try_from(slot & 1).unwrap_or(0);
                let dz = i32::try_from((slot >> 1) & 1).unwrap_or(0);
                let dy = (slot >> 2) & 1;
                let column = (x + dx, z + dz);
                if mask.contains(&column) {
                    columns
                        .get(&column)
                        .map_or(&air, |stack| &stack[2 * j + dy])
                } else {
                    &air
                }
            });
            if let Ok(coarse) = lod::downsample(&octants, water, &scatter) {
                derived.insert((x, i32::try_from(j).unwrap_or(0), z), coarse);
            }
        }
    }

    let mut out: BTreeMap<(i32, i32), Vec<DemoSection>> = BTreeMap::new();
    for &(x, z) in masks.keys() {
        let mut sections = Vec::new();
        for j in 0..(COLUMN_SECTIONS / 2) {
            let j = i32::try_from(j).unwrap_or(0);
            let Some(coarse) = derived.get(&(x, j, z)) else {
                continue;
            };
            // A missing coarse neighbour is drawn as air, so the coarse region's
            // boundary wall is emitted instead of culled against a section that is
            // not on screen. Skirts still continue the top face downward.
            let neighbours: [Option<&section::Section>; 26] =
                padded::NEIGHBOUR_ORDER.map(|(dx, dy, dz)| {
                    derived
                        .get(&(x + dx * 2, j + dy, z + dz * 2))
                        .or(Some(&air))
                });
            let blocks = padded::PaddedBlocks::from_sections(coarse, &neighbours);
            let mut mesh = mesher2::mesh_coarse(&blocks, materials, mesher2::MeshLod::Lod1, 0);
            mesher2::add_skirts(&mut mesh, mesher2::SkirtEdges::ALL, mesher2::MeshLod::Lod1);
            if mesher2::quad_count(&mesh) == 0 {
                continue;
            }
            sections.push(DemoSection {
                origin: [x * 32, j * 64, z * 32],
                scale: 2,
                mesh,
            });
        }
        sections.sort_by_key(|section| section.origin);
        out.insert((x, z), sections);
    }
    out
}

/// Derives and meshes the LOD2 sections of `regions` (`N1.4`, `N3.2`).
///
/// A LOD2 region is the `4x4` column square whose base is 4-aligned, and one
/// section covers `128` blocks, so three cover the column. Every section is
/// derived with [`lod::downsample4`] from 64 fine sections and meshed with
/// `mesh_coarse` at [`mesher2::MeshLod::Lod2`] (four blocks per cell), with a
/// skirt of two LOD2 cells (`8` blocks) on every side. As with LOD1, the
/// surrounding ring is derived too so a region culls against a real neighbour
/// when present and falls back to air at the ring edge.
#[allow(clippy::too_many_lines)]
pub(crate) fn demo_far_regions(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    materials: &mesher2::MaterialProps,
    pack: &content::ContentPack,
    regions: &BTreeSet<(i32, i32)>,
) -> BTreeMap<(i32, i32), Vec<DemoSection>> {
    if regions.is_empty() {
        return BTreeMap::new();
    }
    let water = pack.id("core:water").unwrap_or(0);
    let scatter: Vec<section::BlockId> = pack
        .records()
        .iter()
        .filter(|record| record.render_shape == content::RenderShape::CrossQuad)
        .filter_map(|record| pack.id(&record.name))
        .collect();

    let mut expanded = BTreeSet::new();
    for &(x, z) in regions {
        for dx in -1..=1_i32 {
            for dz in -1..=1_i32 {
                expanded.insert((x + 4 * dx, z + 4 * dz));
            }
        }
    }

    let mut derived: BTreeMap<(i32, i32, i32), section::Section> = BTreeMap::new();
    for &(x, z) in &expanded {
        let complete = (0..4).all(|dx| (0..4).all(|dz| columns.contains_key(&(x + dx, z + dz))));
        if !complete {
            continue;
        }
        for j in 0..(COLUMN_SECTIONS / 4) {
            let sections: [&section::Section; 64] = std::array::from_fn(|slot| {
                let dx = i32::try_from(slot & 3).unwrap_or(0);
                let dz = i32::try_from((slot >> 2) & 3).unwrap_or(0);
                let dy = (slot >> 4) & 3;
                &columns[&(x + dx, z + dz)][4 * j + dy]
            });
            if let Ok(coarse) = lod::downsample4(&sections, water, &scatter) {
                derived.insert((x, i32::try_from(j).unwrap_or(0), z), coarse);
            }
        }
    }

    let air = section::Section::uniform(0);
    let mut out: BTreeMap<(i32, i32), Vec<DemoSection>> = BTreeMap::new();
    for &(x, z) in regions {
        let mut sections = Vec::new();
        for j in 0..(COLUMN_SECTIONS / 4) {
            let j = i32::try_from(j).unwrap_or(0);
            let Some(coarse) = derived.get(&(x, j, z)) else {
                continue;
            };
            let neighbours: [Option<&section::Section>; 26] =
                padded::NEIGHBOUR_ORDER.map(|(dx, dy, dz)| {
                    derived
                        .get(&(x + dx * 4, j + dy, z + dz * 4))
                        .or(Some(&air))
                });
            let blocks = padded::PaddedBlocks::from_sections(coarse, &neighbours);
            let mut mesh = mesher2::mesh_coarse(&blocks, materials, mesher2::MeshLod::Lod2, 0);
            mesher2::add_skirts(&mut mesh, mesher2::SkirtEdges::ALL, mesher2::MeshLod::Lod2);
            if mesher2::quad_count(&mesh) == 0 {
                continue;
            }
            sections.push(DemoSection {
                origin: [x * 32, j * 128, z * 32],
                scale: 4,
                mesh,
            });
        }
        sections.sort_by_key(|section| section.origin);
        out.insert((x, z), sections);
    }
    out
}

#[cfg(test)]
mod tests {
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
}
