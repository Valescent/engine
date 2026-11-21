//! LOD bands: LOD0 near the player, complete and clipped LOD1 in the middle,
//! and complete 4x4 LOD2 squares far out.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::content;
use crate::description;
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
/// `partial` holds clipped `2x2` regions selected from already-covered columns
/// or the ring edge: the ring is `2r+1` columns wide, so the outermost positive
/// base has no `+1` neighbour and cannot be a complete LOD1 region. Each value
/// is the set of columns that region actually draws; the rest of its `2x2`
/// footprint is air.
///
/// A clipped region's key is its section-origin column. It is the aligned base
/// while that base is outside the LOD0 band. A base straddling the LOD0
/// boundary can itself be inside the fine band while the strip it must draw is
/// outside; such a group is re-tiled into `2x2` windows keyed by their smallest
/// drawn column. Every clipped origin is therefore outside the LOD0 band, so
/// the renderer's origin identity stays unique and the finer band keeps its
/// columns.
#[derive(Debug, Default)]
pub(crate) struct BandRegions {
    /// Complete LOD1 `2x2` regions.
    pub(crate) coarse: BTreeSet<(i32, i32)>,
    /// Clipped LOD1 regions keyed by their section-origin column, with the
    /// columns the section draws.
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
/// even radius, and the edge of the fine square where an aligned `2x2` base
/// straddles the LOD0 boundary. Both get a clipped region whose mask names the
/// columns it draws. A clipped region only ever uses its other cells as air;
/// it never overlaps a complete region, so coverage is a partition, not an
/// overlap.
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

    // Group leftover columns by the aligned `2x2` base they were derived in.
    let mut aligned: BTreeMap<(i32, i32), BTreeSet<(i32, i32)>> = BTreeMap::new();
    for column in leftover {
        let base = (
            column.0 - column.0.rem_euclid(2),
            column.1 - column.1.rem_euclid(2),
        );
        aligned.entry(base).or_default().insert(column);
    }

    // Keep the aligned base as the section origin unless that base is itself in
    // the LOD0 band. A base straddling the LOD0 boundary can stay inside the
    // fine square while the strip it must draw lies outside. Using that base as
    // the origin would reuse a LOD0 section origin and the renderer would
    // reject the whole upload, so the group is re-tiled with `2x2` windows
    // anchored at its smallest remaining column. An L-shaped leftover set (the
    // fine square's corner) splits into two anchored windows so every drawn
    // column stays inside its own window. Groups outside the fine band keep
    // their aligned origin, which keeps neighbouring clipped regions tiling and
    // culling against each other.
    let mut partial: BTreeMap<(i32, i32), BTreeSet<(i32, i32)>> = BTreeMap::new();
    for (base, columns) in aligned {
        if ring(base) > TERRAIN_DEMO_FINE_CUTOFF {
            partial.insert(base, columns);
            continue;
        }
        let mut remaining = columns;
        while let Some(&anchor) = remaining.iter().next() {
            let mut drawn = BTreeSet::new();
            for dx in 0..2 {
                for dz in 0..2 {
                    let column = (anchor.0 + dx, anchor.1 + dz);
                    if remaining.remove(&column) {
                        drawn.insert(column);
                    }
                }
            }
            debug_assert!(drawn.contains(&anchor), "the anchor is always drawn");
            let replaced = partial.insert(anchor, drawn);
            debug_assert!(replaced.is_none(), "two clipped groups share an anchor");
        }
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
/// or more of the four cells air, so the region's `2x2` footprint still covers a
/// rim or LOD0-boundary column without drawing over a neighbour that another
/// band already owns. A clipped region's key may be any column, not only an
/// even-aligned base; see [`coarse_bands`].
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

/// The sides of a fine column that face the active coarse band.
///
/// The fine band is the square of columns within [`TERRAIN_DEMO_FINE_CUTOFF`]
/// of `player`. A fine column can therefore only border coarse terrain through
/// a column one ring outside that square: the LOD1 band starts there, and LOD2
/// is never adjacent to LOD0. A visible-ring column outside the cutoff is always
/// claimed by a complete or clipped LOD1 region, so the band membership is the
/// stable rule; [`skirt_fine_section`] additionally requires the neighbour to be
/// resident before it writes geometry.
///
/// Sides come in the fixed `North, South, East, West` order, and the result is
/// empty for a column fully inside LOD0. That keeps the fine band from doubling
/// geometry on its interior edges.
pub(crate) fn fine_skirt_edges(
    column: (i32, i32),
    player: (i32, i32),
    view_radius: i32,
) -> mesher2::SkirtEdges {
    let ring = |cell: (i32, i32)| (cell.0 - player.0).abs().max((cell.1 - player.1).abs());
    let mut edges = mesher2::SkirtEdges::NONE;
    for side in SIDES {
        let neighbour = neighbour_column(column, side);
        let distance = ring(neighbour);
        if distance > TERRAIN_DEMO_FINE_CUTOFF && distance <= view_radius {
            edges = edges.with(side);
        }
    }
    edges
}

/// The four horizontal sides, in the fixed skirt order.
const SIDES: [mesher2::Side; 4] = [
    mesher2::Side::North,
    mesher2::Side::South,
    mesher2::Side::East,
    mesher2::Side::West,
];

/// The column just outside one side of `column`.
fn neighbour_column(column: (i32, i32), side: mesher2::Side) -> (i32, i32) {
    match side {
        mesher2::Side::North => (column.0, column.1 - 1),
        mesher2::Side::South => (column.0, column.1 + 1),
        mesher2::Side::East => (column.0 + 1, column.1),
        mesher2::Side::West => (column.0 - 1, column.1),
    }
}

/// Appends the fine side of a LOD0/coarse stitch, or returns `0`.
///
/// A fine section culls its band-facing wall against the *fine* neighbour
/// column, while a clipped or complete coarse region draws a conservative
/// downsample that can sit a block or more away from that wall's top. Where the
/// coarse surface is lower, the gap between the coarse top and the fine edge is
/// open from the fine side. This appends one downward skirt on each side in
/// `edges` that borders an active coarse column, using the drawn coarse surface
/// as the step:
///
/// - the coarse column's top face is derived with the same conservative vote
///   the LOD1 mesher uses (air, water, and scatter cells do not count);
/// - a flush or higher coarse edge appends nothing;
/// - a lower coarse edge appends exactly the drop, capped at `SKIRT_DEPTH`;
/// - a missing coarse neighbour appends nothing, so the stitch never guesses.
///
/// The padding scratch holds only the coarse surface under the edge, so
/// [`mesher2::add_step_skirts`] reads the coarse height rather than the fine
/// neighbour's. The call is additive, deterministic, and marks the sides it
/// touched; every active selection starts from the unskirted stored mesh, so a
/// repeated selection never doubles a strip. Returns the quad count added.
pub(crate) fn skirt_fine_section(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    materials: &mesher2::MaterialProps,
    pack: &content::ContentPack,
    section: &mut DemoSection,
    column: (i32, i32),
    edges: mesher2::SkirtEdges,
) -> u32 {
    if edges.is_empty() || section.mesh.opaque.is_empty() {
        return 0;
    }
    let blocks_per_section = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
    let Ok(sy) = usize::try_from(section.origin[1].div_euclid(blocks_per_section)) else {
        return 0;
    };
    if sy >= COLUMN_SECTIONS {
        return 0;
    }
    let stone = pack.id("core:stone").unwrap_or(1);
    let width = usize::try_from(description::SECTION_SIZE).unwrap_or(32);
    let mut added = 0;
    for side in SIDES {
        if !edges.contains(side) {
            continue;
        }
        let Some(stack) = columns.get(&neighbour_column(column, side)) else {
            continue;
        };
        let mut blocks = padded::PaddedBlocks::new();
        for along in 0..width {
            let top = coarse_local_top(stack, materials, side, along, sy);
            for y in 0..top {
                let (x, y, z) = padded_cell(side, along, y);
                blocks.set(x, y, z, stone);
            }
        }
        added += mesher2::add_step_skirts(
            &mut section.mesh,
            &blocks,
            materials,
            mesher2::SkirtEdges::NONE.with(side),
            mesher2::MeshLod::Lod0,
        );
    }
    added
}

/// The `2x2` fine columns one coarse cell covers at a section edge.
///
/// A LOD1 cell is two blocks wide on every axis, so the `2x2x2` vote at a
/// boundary always reads two adjacent blocks along the tangent and two along
/// the normal inside the adjacent fine column. Both pairs are block-aligned
/// because every column origin is a multiple of the section size.
fn coarse_block_spans(side: mesher2::Side, along: usize) -> ([usize; 2], [usize; 2]) {
    let tangent = [along & !1, (along & !1) + 1];
    match side {
        mesher2::Side::East => ([0, 1], tangent),
        mesher2::Side::West => ([30, 31], tangent),
        mesher2::Side::South => (tangent, [0, 1]),
        mesher2::Side::North => (tangent, [30, 31]),
    }
}

/// Padded-scratch coordinates of one outside column cell.
fn padded_cell(side: mesher2::Side, along: usize, y: u32) -> (i32, i32, i32) {
    let along = i32::try_from(along).unwrap_or(0);
    let y = i32::try_from(y).unwrap_or(0);
    match side {
        mesher2::Side::North => (along, y, -1),
        mesher2::Side::South => (along, y, 32),
        mesher2::Side::East => (32, y, along),
        mesher2::Side::West => (-1, y, along),
    }
}

/// The coarse surface height under one boundary cell, in section cells.
///
/// Returns the number of cells from the section floor up to (but excluding) the
/// coarse top face, clamped to the section. `0` means the coarse column is empty
/// in this section, so the step is deeper than the section and the caller's
/// `SKIRT_DEPTH` cap applies.
fn coarse_local_top(
    stack: &[section::Section; COLUMN_SECTIONS],
    materials: &mesher2::MaterialProps,
    side: mesher2::Side,
    along: usize,
    sy: usize,
) -> u32 {
    let (xs, zs) = coarse_block_spans(side, along);
    let blocks_per_section = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
    let base = i32::try_from(sy).unwrap_or(0) * blocks_per_section;
    for y in (0..blocks_per_section).rev() {
        let world_y = base + y;
        let section = &stack[usize::try_from(world_y.div_euclid(blocks_per_section)).unwrap_or(0)];
        let solid = xs.iter().any(|&x| {
            zs.iter().any(|&z| {
                let id = section.get(
                    u32::try_from(x).unwrap_or(0),
                    u32::try_from(world_y.rem_euclid(blocks_per_section)).unwrap_or(0),
                    u32::try_from(z).unwrap_or(0),
                );
                let material = materials.material(id);
                id != 0 && !material.water && !material.cross_quad
            })
        });
        if solid {
            let top_face = (world_y & !1) + 2;
            return u32::try_from(top_face - base).unwrap_or(0).min(32);
        }
    }
    0
}

#[cfg(test)]
mod tests;
