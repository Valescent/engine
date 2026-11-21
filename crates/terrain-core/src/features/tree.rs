//! Trees: data templates, the jittered grid, spacing, and placement (`H3`).
//!
//! # Templates
//!
//! A tree is a template over a closed primitive set (`H3.2`):
//! `Part::Trunk`, `Part::Branch`, `Part::Canopy` with a
//! `CanopyShape`, and `Part::RootFlare`. Species are data in `SPECIES`;
//! the per-tree variant comes from `hash64(seed, "tree:<species>", id)`, so a
//! tree is identical wherever it is applied.
//!
//! Leaves are emitted only inside the canopy volume *and* within
//! `LEAF_MAX_DIST` of a stem, checked against the template's own parts. There
//! is no world scan and no floating leaf.
//!
//! # Grid and spacing
//!
//! Discovery uses a jittered grid with `TREE_CELL` spacing keyed on the cell
//! and the vertical region (the "group"). Density only changes acceptance; it
//! never moves the grid (`H3.3`).
//!
//! Spacing is resolved at application from the gathered candidates: a candidate
//! loses to any higher-priority candidate within `MIN_DIST` in the `3 x 3`
//! cells, with ties to the lower cell index and then the lower id. Spacing runs
//! before the 2D height filters.
//!
//! # Ownership
//!
//! A tree candidate is discovered in every vertical region, because discovery
//! cannot know the surface height. Exactly one of those copies is live: the one
//! whose region contains the tree's base (`top + 1`). That check runs before
//! spacing, so dead copies never compete for space.
//!
//! # Water
//!
//! Trees never touch water (`H3.5`). A candidate is rejected when its centre is
//! underwater or a river/lake bed, when any column within `TREE_WATER_MARGIN`
//! carries water, or when any column that close carries a bank. The trunk base is
//! sunk to the lowest of the centre and its four edge neighbours and rejected
//! below `SEA + 1`. At write time a water cell is skipped, never replaced: logs
//! accept air or an existing leaf, leaves accept air only, and logs run first so
//! a log outranks a leaf on overlap.
//!
//! # Generation only
//!
//! Trees are generated once and never simulated (`H3.6`). This module exposes no
//! sapling, growth tick, leaf decay, or per-tree runtime state: the only entry
//! points are the generation-time `discover`, `place`, and the apply pass
//! driven from `crate::features::apply`. After generation a tree is ordinary
//! log and leaf blocks; later placement is an ordinary edit through the edit
//! path, not growth.

use crate::description::{SEA, WORLD_HEIGHT};
use crate::hash::{domain, hash64};

use crate::features::{
    COLUMN_BANK, COLUMN_RIVER, Candidate, ColumnField, FeatureKind, FeatureSpec, Mask, REGION_SIZE,
    Sink, region_contains, within,
};

/// Grid cell edge in blocks (`H3.3`).
pub(crate) const TREE_CELL: i32 = 6;

/// Minimum trunk spacing in blocks (`H3.3`).
pub(crate) const MIN_DIST: i32 = 4;

/// Largest slope drop a tree may stand on (`H3.4`).
pub(crate) const TREE_MAX_DROP: i32 = 2;

/// Columns a tree must stay away from water and banks (`H3.5`).
pub(crate) const TREE_WATER_MARGIN: i32 = 2;

/// Largest distance from a leaf to the nearest stem (`H3.2`).
pub(crate) const LEAF_MAX_DIST: i32 = 4;

/// Largest declared tree half-extent (`H3.2`).
pub(crate) const TREE_MAX_EXTENT: i32 = 12;

/// Closed set of canopy volumes (`H3.2`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum CanopyShape {
    /// An ellipsoid sitting on the trunk.
    Ellipsoid,
    /// A cone of shrinking discs, for conifers.
    Cone,
    /// Two or three stacked discs, for birches.
    LayeredDisc,
}

/// Closed set of tree primitives (`H3.2`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum Part {
    /// The vertical stem.
    Trunk,
    /// One short diagonal arm.
    Branch,
    /// A canopy volume of the given shape.
    Canopy(CanopyShape),
    /// Blocks flaring out at the base.
    RootFlare,
}

/// Which pass a placement belongs to. Logs are written before leaves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Layer {
    /// Written through [`Mask::AirOrLeaf`].
    Log,
    /// Written through [`Mask::AirOnly`].
    Leaf,
}

/// One block of a tree, relative to the trunk base.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Placement {
    /// Offset from the trunk base; `[0, 0, 0]` is the lowest trunk block.
    pub offset: [i32; 3],
    /// Log or leaf.
    pub layer: Layer,
}

/// Biome bits a species may grow in.
const BIOME_PLAINS: u8 = 2;

/// Forest biome bit.
const BIOME_FOREST: u8 = 3;

/// Alpine biome bit.
const BIOME_ALPINE: u8 = 5;

/// One species record (`H3.1`: data, not a constant count).
#[derive(Clone, Copy, Debug)]
struct Species {
    /// Stable name.
    name: &'static str,
    /// Hash domain of the per-tree variant.
    domain: &'static str,
    /// The primitives this species uses.
    parts: &'static [Part],
    /// Shortest trunk.
    min_height: u32,
    /// Extra trunk heights above [`Self::min_height`].
    height_span: u32,
    /// Smallest canopy radius.
    min_radius: u32,
    /// Extra canopy radii above [`Self::min_radius`].
    radius_span: u32,
    /// Bit set of biomes the species tolerates.
    biomes: u8,
}

const _: () = assert!(TREE_CELL > MIN_DIST && TREE_MAX_EXTENT <= REGION_SIZE);

/// The v1 species table.
const SPECIES: [Species; 3] = [
    Species {
        name: "oak",
        domain: "tree:oak",
        parts: &[
            Part::Trunk,
            Part::Branch,
            Part::Canopy(CanopyShape::Ellipsoid),
            Part::RootFlare,
        ],
        min_height: 4,
        height_span: 3,
        min_radius: 2,
        radius_span: 2,
        biomes: (1 << BIOME_PLAINS) | (1 << BIOME_FOREST),
    },
    Species {
        name: "pine",
        domain: "tree:pine",
        parts: &[
            Part::Trunk,
            Part::Branch,
            Part::Canopy(CanopyShape::Cone),
            Part::RootFlare,
        ],
        min_height: 6,
        height_span: 3,
        min_radius: 2,
        radius_span: 1,
        biomes: (1 << BIOME_FOREST) | (1 << BIOME_ALPINE),
    },
    Species {
        name: "birch",
        domain: "tree:birch",
        parts: &[
            Part::Trunk,
            Part::Canopy(CanopyShape::LayeredDisc),
            Part::RootFlare,
        ],
        min_height: 5,
        height_span: 2,
        min_radius: 2,
        radius_span: 1,
        biomes: (1 << BIOME_PLAINS) | (1 << BIOME_FOREST),
    },
];

/// Acceptance density per biome, in permille (`H3.3`).
///
/// Indexed by the v1 biome ids: ocean, beach, plains, forest, desert, alpine.
/// Density changes acceptance only; the grid is fixed.
pub(crate) const DENSITY: [u32; 6] = [0, 0, 400, 800, 40, 250];

/// Acceptance density of one biome, or zero outside the table.
#[must_use]
pub(crate) fn density(biome: u8) -> u32 {
    DENSITY.get(usize::from(biome)).copied().unwrap_or_default()
}

/// The species index of a candidate.
#[must_use]
pub(crate) fn species_of(candidate: &Candidate) -> usize {
    species_of_id(candidate.id)
}

/// Stable name of a species index, for content packs and debug views.
#[must_use]
pub(crate) fn species_name(index: usize) -> Option<&'static str> {
    SPECIES.get(index).map(|entry| entry.name)
}

/// Species of a candidate id, seed-free so application can recompute it.
fn species_of_id(id: u64) -> usize {
    let count = u64::try_from(SPECIES.len()).expect("the species count fits u64");
    usize::try_from(hash64(0, domain("tree:species"), &[id.cast_signed()]) % count)
        .expect("a species index fits usize")
}

/// Whether a species may grow in a biome (`H3.4`).
#[must_use]
pub(crate) fn allowed(species: usize, biome: u8) -> bool {
    if biome >= 8 {
        return false;
    }
    SPECIES
        .get(species)
        .is_some_and(|entry| entry.biomes & (1 << biome) != 0)
}

/// Whether density accepts this candidate in this biome (`H3.3`).
#[must_use]
pub(crate) fn accepts(seed: u64, candidate: &Candidate, biome: u8) -> bool {
    let roll = hash64(seed, domain("tree:accept"), &[candidate.id.cast_signed()]) % 1000;
    roll < u64::from(density(biome))
}

/// Priority of a candidate within its grid neighbourhood.
///
/// Derived from the id, which already folds the cell and the region, so
/// application needs no seed and the comparison is total.
#[must_use]
pub(crate) fn priority(candidate: &Candidate) -> u64 {
    let cell = cell_of(candidate.origin);
    hash64(
        candidate.id,
        domain("tree:priority"),
        &[i64::from(cell[0]), i64::from(cell[1])],
    )
}

/// Whether `left` outranks `right` (`H3.3`).
///
/// Higher priority wins; equal priority goes to the lower cell index, then to
/// the lower id so the relation is a total order.
#[must_use]
pub(crate) fn beats(left: &Candidate, right: &Candidate) -> bool {
    let left_priority = priority(left);
    let right_priority = priority(right);
    if left_priority != right_priority {
        return left_priority > right_priority;
    }
    let left_cell = cell_of(left.origin);
    let right_cell = cell_of(right.origin);
    if left_cell != right_cell {
        return left_cell < right_cell;
    }
    left.id < right.id
}

/// Grid cell of a column.
fn cell_of(point: [i32; 3]) -> [i32; 2] {
    [
        point[0].div_euclid(TREE_CELL),
        point[2].div_euclid(TREE_CELL),
    ]
}

/// Whether two candidates are within [`MIN_DIST`] on the column plane.
fn within_min_dist(left: &Candidate, right: &Candidate) -> bool {
    let dx = left.origin[0] - right.origin[0];
    let dz = left.origin[2] - right.origin[2];
    dx * dx + dz * dz <= MIN_DIST * MIN_DIST
}

/// Discovers the grid candidates of one region (`H3.3`).
pub(crate) fn discover(region: [i32; 3], seed: u64, spec: &FeatureSpec, out: &mut Vec<Candidate>) {
    if spec.kind != FeatureKind::Tree || SPECIES.is_empty() {
        return;
    }
    let origin = crate::features::region_origin(region);
    let first_x = origin[0].div_euclid(TREE_CELL);
    let last_x = (origin[0] + REGION_SIZE - 1).div_euclid(TREE_CELL);
    let first_z = origin[2].div_euclid(TREE_CELL);
    let last_z = (origin[2] + REGION_SIZE - 1).div_euclid(TREE_CELL);
    let domains = [
        domain(SPECIES[0].domain),
        domain(SPECIES[1].domain),
        domain(SPECIES[2].domain),
    ];
    let jitter_span = TREE_CELL - 2;

    for cell_z in first_z..=last_z {
        for cell_x in first_x..=last_x {
            let key = [i64::from(cell_x), i64::from(cell_z), i64::from(region[1])];
            let jitter = hash64(seed, domain("tree:cell"), &key);
            let x = cell_x * TREE_CELL + 1 + within(jitter, jitter_span);
            let z = cell_z * TREE_CELL + 1 + within(jitter >> 16, jitter_span);
            if x.div_euclid(REGION_SIZE) != region[0] || z.div_euclid(REGION_SIZE) != region[2] {
                continue;
            }
            let y = origin[1] + within(jitter >> 32, REGION_SIZE);
            let id = hash64(
                0,
                domain("tree"),
                &[
                    i64::from(region[0]),
                    i64::from(region[1]),
                    i64::from(region[2]),
                    i64::from(cell_x),
                    i64::from(cell_z),
                ],
            );
            let species = species_of_id(id);
            let variant = hash64(seed, domains[species], &[id.cast_signed()]);
            out.push(Candidate {
                id,
                region,
                kind: FeatureKind::Tree,
                origin: [x, y, z],
                variant,
            });
        }
    }
}

/// Writes every live tree candidate of one section.
pub(crate) fn apply(sink: &mut Sink<'_, '_>, ordered: &[&Candidate]) {
    let live: Vec<Live<'_>> = ordered
        .iter()
        .copied()
        .filter(|candidate| candidate.kind == FeatureKind::Tree)
        .filter_map(|candidate| {
            let top = sink
                .ctx()
                .columns
                .top(candidate.origin[0], candidate.origin[2])?;
            let base = trunk_base(sink.ctx().columns, candidate.origin[0], candidate.origin[2])?;
            let foot = [candidate.origin[0], base, candidate.origin[2]];
            region_contains(candidate.region, foot).then_some(Live {
                candidate,
                top,
                base,
            })
        })
        .collect();
    if live.is_empty() {
        return;
    }

    let mut cells: Vec<(i32, i32, usize)> = live
        .iter()
        .enumerate()
        .map(|(slot, entry)| {
            let cell = cell_of(entry.candidate.origin);
            (cell[0], cell[1], slot)
        })
        .collect();
    cells.sort_unstable();

    for slot in 0..live.len() {
        if dominated(&live, &cells, slot) {
            sink.skip_candidate();
            continue;
        }
        let entry = live[slot];
        if !accepts(sink.ctx().seed, entry.candidate, biome_of(sink, &entry)) {
            sink.skip_candidate();
            continue;
        }
        if !filters(sink, &entry) {
            sink.skip_candidate();
            continue;
        }
        write_tree(sink, &entry);
    }
}

/// One candidate that owns a column in this section.
#[derive(Clone, Copy)]
struct Live<'a> {
    /// The candidate itself.
    candidate: &'a Candidate,
    /// Top solid block of its own column.
    top: i32,
    /// Sunk trunk base: one above the lowest top in its footprint.
    base: i32,
}

/// The trunk base of one column (`H3.5`).
///
/// The centre and its four edge neighbours are compared, and the base is one
/// above the **lowest** top, so a tree on a step is sunk to the low side rather
/// than floating over it. A missing neighbour cannot lower the base. `None` is
/// returned only when the centre column itself is outside the footprint.
#[must_use]
pub(crate) fn trunk_base(columns: ColumnField<'_>, x: i32, z: i32) -> Option<i32> {
    let mut lowest = columns.top(x, z)?;
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        if let Some(top) = columns.top(x + dx, z + dz) {
            lowest = lowest.min(top);
        }
    }
    Some(lowest + 1)
}

/// Whether a column carries water (`H3.5`).
///
/// A column is water when its top is below the sea, or when the hydrology tile
/// marked it as a river or lake. Those are the two facts the footprint already
/// carries; no second tile read is made.
fn water_column(columns: ColumnField<'_>, x: i32, z: i32) -> bool {
    let flags = columns.flags(x, z).unwrap_or_default();
    flags & COLUMN_RIVER != 0 || columns.top(x, z).is_some_and(|top| top < SEA)
}

/// Whether any column within `radius` of `(x, z)` carries water.
fn near_water(columns: ColumnField<'_>, x: i32, z: i32, radius: i32) -> bool {
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            if water_column(columns, x + dx, z + dz) {
                return true;
            }
        }
    }
    false
}

/// Whether any column within `radius` of `(x, z)` carries a bank bit.
fn near_bank(columns: ColumnField<'_>, x: i32, z: i32, radius: i32) -> bool {
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let flags = columns.flags(x + dx, z + dz).unwrap_or_default();
            if flags & COLUMN_BANK != 0 {
                return true;
            }
        }
    }
    false
}

/// Biome of the candidate's column, or an out-of-table id.
fn biome_of(sink: &Sink<'_, '_>, entry: &Live<'_>) -> u8 {
    sink.ctx()
        .columns
        .biome(entry.candidate.origin[0], entry.candidate.origin[2])
        .unwrap_or(u8::MAX)
}

/// Whether any higher-priority candidate wins this column (`H3.3`).
fn dominated(live: &[Live<'_>], cells: &[(i32, i32, usize)], slot: usize) -> bool {
    let candidate = live[slot].candidate;
    let cell = cell_of(candidate.origin);
    for dz in -1..=1 {
        for dx in -1..=1 {
            let key = (cell[0] + dx, cell[1] + dz);
            let low = cells.partition_point(|entry| (entry.0, entry.1) < key);
            let high = cells.partition_point(|entry| (entry.0, entry.1) <= key);
            for entry in &cells[low..high] {
                if entry.2 == slot {
                    continue;
                }
                let other = live[entry.2].candidate;
                if within_min_dist(candidate, other) && beats(other, candidate) {
                    return true;
                }
            }
        }
    }
    false
}

/// The 2D filters of `H3.4` and the water rules of `H3.5`, after spacing.
fn filters(sink: &Sink<'_, '_>, entry: &Live<'_>) -> bool {
    let candidate = entry.candidate;
    let x = candidate.origin[0];
    let z = candidate.origin[2];
    let columns = sink.ctx().columns;
    let Some(slope) = columns.slope(x, z) else {
        return false;
    };
    if slope > TREE_MAX_DROP {
        return false;
    }
    // Not underwater, not a bed, not near a bank, and clear of every column
    // with a non-empty water range (`H3.5`).
    if entry.top < SEA
        || near_water(columns, x, z, TREE_WATER_MARGIN)
        || near_bank(columns, x, z, TREE_WATER_MARGIN)
    {
        return false;
    }
    // The sunk base must stay above the sea, so no trunk stands in water.
    if entry.base < SEA + 1 {
        return false;
    }
    if !allowed(species_of(candidate), biome_of(sink, entry)) {
        return false;
    }
    let tree_top = entry.base + top_offset(candidate);
    let world_top = i32::try_from(WORLD_HEIGHT).expect("the world height fits i32") - 2;
    if tree_top > world_top {
        return false;
    }
    !roofed(sink, entry, tree_top)
}

/// Whether the trunk column is closed above its surface (`H3.4`).
///
/// The scan starts at the own column's top, not the sunk base: a sunk base may
/// lie inside lower ground, and that ground must not count as a roof.
fn roofed(sink: &Sink<'_, '_>, entry: &Live<'_>, tree_top: i32) -> bool {
    let palette = sink.ctx().palette;
    for y in entry.top + 1..=tree_top {
        let position = [entry.candidate.origin[0], y, entry.candidate.origin[2]];
        if let Some(id) = sink.block(position)
            && id != palette.air
            && id != palette.log
            && id != palette.leaf
        {
            return true;
        }
    }
    false
}

/// The tallest offset in a candidate's template.
fn top_offset(candidate: &Candidate) -> i32 {
    place(candidate)
        .iter()
        .map(|placement| placement.offset[1])
        .max()
        .unwrap_or_default()
}

/// Emits a template and writes it, logs before leaves (`H2.1`).
fn write_tree(sink: &mut Sink<'_, '_>, entry: &Live<'_>) {
    let placements = place(entry.candidate);
    let (low, high) = placement_bounds(entry, &placements);
    if !sink.intersects(low, high) {
        return;
    }
    let palette = sink.ctx().palette;
    for placement in &placements {
        if placement.layer != Layer::Log {
            continue;
        }
        sink.write(
            world_position(entry, placement.offset),
            Mask::AirOrLeaf,
            palette.log,
        );
    }
    for placement in &placements {
        if placement.layer != Layer::Leaf {
            continue;
        }
        sink.write(
            world_position(entry, placement.offset),
            Mask::AirOnly,
            palette.leaf,
        );
    }
}

/// World position of one placement.
fn world_position(entry: &Live<'_>, offset: [i32; 3]) -> [i32; 3] {
    [
        entry.candidate.origin[0] + offset[0],
        entry.base + offset[1],
        entry.candidate.origin[2] + offset[2],
    ]
}

/// World-space bounding box of an emitted template.
fn placement_bounds(entry: &Live<'_>, placements: &[Placement]) -> ([i32; 3], [i32; 3]) {
    let mut low = [i32::MAX; 3];
    let mut high = [i32::MIN; 3];
    for placement in placements {
        let position = world_position(entry, placement.offset);
        for axis in 0..3 {
            low[axis] = low[axis].min(position[axis]);
            high[axis] = high[axis].max(position[axis]);
        }
    }
    (low, high)
}

/// Emits the template of one candidate as offsets from its trunk base (`H3.2`).
#[must_use]
pub(crate) fn place(candidate: &Candidate) -> Vec<Placement> {
    let species = species_of(candidate);
    let Some(template) = SPECIES.get(species) else {
        return Vec::new();
    };
    let height = template.min_height
        + u32::try_from((candidate.variant >> 4) % u64::from(template.height_span))
            .expect("a height jitter fits u32");
    let radius = template.min_radius
        + u32::try_from((candidate.variant >> 12) % u64::from(template.radius_span))
            .expect("a radius jitter fits u32");
    let height = i32::try_from(height).expect("a tree height fits i32");
    let radius = i32::try_from(radius).expect("a tree radius fits i32");

    let mut stems = Vec::new();
    for part in template.parts {
        match part {
            Part::Trunk => {
                for dy in 0..height {
                    stems.push([0, dy, 0]);
                }
            }
            Part::Branch => stems.extend(branches(candidate.variant, height)),
            Part::RootFlare => stems.extend(root_flare()),
            Part::Canopy(_) => {}
        }
    }

    let mut leaves = Vec::new();
    for part in template.parts {
        if let Part::Canopy(shape) = part {
            leaves.extend(canopy(*shape, candidate.variant, height, radius));
        }
    }

    let limit = LEAF_MAX_DIST * LEAF_MAX_DIST;
    leaves.retain(|leaf| {
        stems.iter().any(|stem| {
            let dx = stem[0] - leaf[0];
            let dy = stem[1] - leaf[1];
            let dz = stem[2] - leaf[2];
            dx * dx + dy * dy + dz * dz <= limit
        })
    });
    leaves.retain(|leaf| !stems.contains(leaf));
    leaves.sort_unstable();
    leaves.dedup();

    let mut out = Vec::with_capacity(stems.len() + leaves.len());
    for offset in stems {
        out.push(Placement {
            offset,
            layer: Layer::Log,
        });
    }
    for offset in leaves {
        out.push(Placement {
            offset,
            layer: Layer::Leaf,
        });
    }
    out
}

/// Two short diagonal arms.
fn branches(variant: u64, height: i32) -> Vec<[i32; 3]> {
    let mut out = Vec::new();
    for arm in 0..2_u32 {
        let word = hash64(variant, domain("tree:branch"), &[i64::from(arm)]);
        let dir_x = if (word >> 1) & 1 == 0 { -1 } else { 1 };
        let dir_z = if (word >> 2) & 1 == 0 { -1 } else { 1 };
        let base = height / 2 + i32::try_from(arm).expect("a branch index fits i32");
        for step in 1..=2_i32 {
            out.push([dir_x * step, base + step, dir_z * step]);
        }
    }
    out
}

/// Four blocks flaring out at the base.
fn root_flare() -> Vec<[i32; 3]> {
    vec![[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]]
}

/// The leaves of one canopy volume.
fn canopy(shape: CanopyShape, variant: u64, height: i32, radius: i32) -> Vec<[i32; 3]> {
    match shape {
        CanopyShape::Ellipsoid => ellipsoid_canopy(height, radius),
        CanopyShape::Cone => cone_canopy(height, radius),
        CanopyShape::LayeredDisc => disc_canopy(variant, height, radius),
    }
}

/// An ellipsoid centred one block above the trunk top.
fn ellipsoid_canopy(height: i32, radius: i32) -> Vec<[i32; 3]> {
    let ry = (radius - 1).max(1);
    let limit = radius * radius * ry * ry;
    let mut out = Vec::new();
    for dy in -ry..=ry {
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let value = dx * dx * ry * ry + dy * dy * radius * radius + dz * dz * ry * ry;
                if value <= limit {
                    out.push([dx, height + dy, dz]);
                }
            }
        }
    }
    out
}

/// A cone of shrinking discs, for conifers.
fn cone_canopy(height: i32, radius: i32) -> Vec<[i32; 3]> {
    let base = height / 2;
    let layers = radius + 2;
    let mut out = Vec::new();
    for step in 0..=layers {
        let r = radius * (layers - step) / layers;
        for dz in -r..=r {
            for dx in -r..=r {
                if dx * dx + dz * dz <= r * r {
                    out.push([dx, base + step, dz]);
                }
            }
        }
    }
    out
}

/// Two wide discs and a narrow one on top, for birches.
fn disc_canopy(variant: u64, height: i32, radius: i32) -> Vec<[i32; 3]> {
    let mut out = Vec::new();
    for layer in 0..3_i32 {
        let wide = layer < 2;
        let r = if wide { radius } else { (radius - 1).max(0) };
        let wobble = within(hash64(variant, domain("tree:disc"), &[i64::from(layer)]), 2);
        let y = height + layer - wobble;
        for dz in -r..=r {
            for dx in -r..=r {
                if dx * dx + dz * dz <= r * r {
                    out.push([dx, y, dz]);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: u64, variant: u64, x: i32, z: i32) -> Candidate {
        Candidate {
            id,
            region: [0, 0, 0],
            kind: FeatureKind::Tree,
            origin: [x, 0, z],
            variant,
        }
    }

    #[test]
    fn every_species_has_a_stem_and_leaves() {
        for id in 0..64_u64 {
            let tree = candidate(id, hash64(1, domain("tree:oak"), &[id.cast_signed()]), 0, 0);
            let placements = place(&tree);
            assert!(
                placements.iter().any(|p| p.layer == Layer::Log),
                "id {id} has no stem"
            );
            assert!(
                placements.iter().any(|p| p.layer == Layer::Leaf),
                "id {id} has no leaves"
            );
        }
    }

    #[test]
    fn logs_come_before_leaves() {
        for id in 0..32_u64 {
            let tree = candidate(id, id.wrapping_mul(0x9e37), 0, 0);
            let placements = place(&tree);
            let first_leaf = placements.iter().position(|p| p.layer == Layer::Leaf);
            let last_log = placements.iter().rposition(|p| p.layer == Layer::Log);
            if let (Some(first_leaf), Some(last_log)) = (first_leaf, last_log) {
                assert!(last_log < first_leaf, "id {id} interleaves logs and leaves");
            }
        }
    }

    #[test]
    fn spacing_is_a_total_order() {
        let a = candidate(1, 1, 0, 0);
        let b = candidate(2, 2, 3, 0);
        let c = candidate(3, 3, 3, 0);
        assert_ne!(beats(&a, &b), beats(&b, &a));
        assert!(!beats(&a, &a));
        assert!(!beats(&b, &b));
        assert_ne!(beats(&c, &b), beats(&b, &c));
    }

    #[test]
    fn trees_are_generation_only() {
        // `H3.6`: a tree is generated once from its candidate and never
        // simulated. `place` reads no clock, seed, or world, so repeated calls
        // agree exactly, and the closed layer set has no sapling or growth
        // stage: only logs and leaves are ever emitted.
        for id in 0..32_u64 {
            let tree = candidate(id, hash64(7, domain("tree:oak"), &[id.cast_signed()]), 0, 0);
            let first = place(&tree);
            let second = place(&tree);
            assert_eq!(first, second, "id {id} changed between placements");
            for placement in &first {
                assert!(
                    matches!(placement.layer, Layer::Log | Layer::Leaf),
                    "id {id} emitted a non-generation layer"
                );
            }
        }
    }

    #[test]
    fn the_variant_is_the_species_domain_hash() {
        let id = 42_u64;
        let species = species_of_id(id);
        let variant = hash64(7, domain(SPECIES[species].domain), &[id.cast_signed()]);
        let tree = candidate(id, variant, 0, 0);
        assert_eq!(species_of(&tree), species);
    }
}
