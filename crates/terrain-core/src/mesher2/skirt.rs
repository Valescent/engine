//! Boundary skirts (`N3.2`).

use crate::description::SECTION_SIZE;
use crate::lod::SKIRT_DEPTH;
use crate::padded::{LOCAL_MAX, LOCAL_MIN, PaddedBlocks};
use crate::revisions::Face as ColumnFace;

use super::PLANE_WIDTH;
use super::lod::MeshLod;
use super::material::MaterialProps;
use super::record::{
    NORMAL_NEG_X, NORMAL_NEG_Z, NORMAL_POS_X, NORMAL_POS_Y, NORMAL_POS_Z, Quad, SectionMesh,
};

// ---------------------------------------------------------------------------
// Skirts (`N3.2`)
// ---------------------------------------------------------------------------

/// Fully open AO: every corner at `3`, so a skirt never darkens a seam.
const AO_OPEN: u8 = 0xFF;

/// One horizontal side of a section, in [`Face`](crate::revisions::Face) order.
///
/// The compass meaning follows the hydro flow graph: north is `z - 1`, east is
/// `x + 1`, south is `z + 1`, west is `x - 1`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Side {
    /// The `z = 0` edge, facing `-Z`.
    North,
    /// The `z = 31` edge, facing `+Z`.
    South,
    /// The `x = 31` edge, facing `+X`.
    East,
    /// The `x = 0` edge, facing `-X`.
    West,
}

impl Side {
    /// Every side, in the fixed order skirts are appended.
    pub const ALL: [Self; crate::revisions::FACE_COUNT] =
        [Self::North, Self::South, Self::East, Self::West];

    /// The set bit of this side in [`SkirtEdges`].
    const fn bit(self) -> u8 {
        match self {
            Self::North => 0b0001,
            Self::South => 0b0010,
            Self::East => 0b0100,
            Self::West => 0b1000,
        }
    }

    /// The outward normal code of this side's wall.
    const fn normal(self) -> u8 {
        match self {
            Self::North => NORMAL_NEG_Z,
            Self::South => NORMAL_POS_Z,
            Self::East => NORMAL_POS_X,
            Self::West => NORMAL_NEG_X,
        }
    }

    /// The cell plane inside the section that the wall lies on.
    fn plane(self) -> u8 {
        match self {
            Self::North | Self::West => 0,
            Self::South | Self::East => {
                u8::try_from(SECTION_SIZE - 1).expect("a section is 32 cells wide")
            }
        }
    }
}

impl From<ColumnFace> for Side {
    fn from(face: ColumnFace) -> Self {
        match face {
            ColumnFace::North => Self::North,
            ColumnFace::South => Self::South,
            ColumnFace::East => Self::East,
            ColumnFace::West => Self::West,
        }
    }
}

/// Which sides of a section carry a downward skirt (`N3.2`).
///
/// The bits follow [`Side`] order, so a mask is also a four-slot side set.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SkirtEdges(u8);

impl SkirtEdges {
    /// No side: no skirts.
    pub const NONE: Self = Self(0);
    /// The north edge only.
    pub const NORTH: Self = Self(0b0001);
    /// The south edge only.
    pub const SOUTH: Self = Self(0b0010);
    /// The east edge only.
    pub const EAST: Self = Self(0b0100);
    /// The west edge only.
    pub const WEST: Self = Self(0b1000);
    /// Every edge.
    pub const ALL: Self = Self(0b1111);

    /// Whether the north edge is selected.
    pub const fn north(self) -> bool {
        self.0 & 0b0001 != 0
    }

    /// Whether the south edge is selected.
    pub const fn south(self) -> bool {
        self.0 & 0b0010 != 0
    }

    /// Whether the east edge is selected.
    pub const fn east(self) -> bool {
        self.0 & 0b0100 != 0
    }

    /// Whether the west edge is selected.
    pub const fn west(self) -> bool {
        self.0 & 0b1000 != 0
    }

    /// Adds one side.
    #[must_use]
    pub const fn with(self, side: Side) -> Self {
        Self(self.0 | side.bit())
    }

    /// Removes one side.
    #[must_use]
    pub const fn without(self, side: Side) -> Self {
        Self(self.0 & !side.bit())
    }

    /// Whether `side` is selected.
    pub const fn contains(self, side: Side) -> bool {
        self.0 & side.bit() != 0
    }

    /// Whether no side is selected.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// World-block height of a skirt at `lod`: [`SKIRT_DEPTH`] cells.
pub const fn skirt_blocks(lod: MeshLod) -> u32 {
    SKIRT_DEPTH.unsigned_abs() * lod.cells_per_block()
}

/// One boundary surface cell: its height and the fields a skirt copies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SkirtCell {
    /// Block origin `y` of the boundary top face.
    y: u8,
    /// Sky nibble of the face's air-side cell.
    sky: u8,
    /// Block-light nibble of the face's air-side cell.
    blk: u8,
    /// Texture layer, copied from the boundary face.
    tile: u16,
    /// Biome tint, copied from the boundary face.
    tint: u8,
}

/// Appends the downward skirts of `edges` to the opaque range (`N3.2`).
///
/// A coarse section draws only where its covered fine sections are absent, so
/// its lower-resolution edges can sit over a hole. A skirt continues the exposed
/// boundary wall downward by [`SKIRT_DEPTH`] cells, which is [`skirt_blocks`]
/// world blocks, and hides the gap without stitching.
///
/// For each selected side the lowest exposed top face of every boundary column
/// supplies one strip: the `+Y` quads whose rectangle touches that side's plane.
/// Columns with an equal face merge into one run, and a run emits one quad per
/// cell layer. Sides come in the fixed order `North, South, East, West`, then
/// runs ascend in `z`/`x`. Records go to the opaque range, copy the boundary
/// face's `tile` and `tint`, take its air-side `sky`/`blk`, and always use fully
/// open AO.
///
/// A boundary face within [`SKIRT_DEPTH`] cells of the section floor gets fewer
/// layers, because world geometry below `y = 0` belongs to the section below.
/// An empty mesh adds nothing.
///
/// This function is additive and deterministic, and a skirt set is never
/// appended twice: [`SectionMesh::skirts`] remembers the sides already done, so
/// a repeated call appends nothing and returns `0`. The marker belongs to one
/// mesh, so a rebuilt section needs a fresh [`SectionMesh`].
///
/// The return value is the number of quads added.
pub fn add_skirts(mesh: &mut SectionMesh, edges: SkirtEdges, lod: MeshLod) -> u32 {
    if edges.is_empty() || mesh.opaque.is_empty() {
        return 0;
    }
    let depth = SKIRT_DEPTH.unsigned_abs();
    debug_assert_eq!(
        skirt_blocks(lod),
        depth * lod.cells_per_block(),
        "a skirt spans SKIRT_DEPTH cells, so its block height scales with the level"
    );
    let mut strip = Vec::new();
    for side in Side::ALL {
        if edges.contains(side) && !mesh.skirts.contains(side) {
            push_side_skirts(&mut strip, mesh, side, depth);
        }
    }
    let added = u32::try_from(strip.len()).expect("a skirt batch fits u32");
    for side in Side::ALL {
        if edges.contains(side) {
            mesh.skirts = mesh.skirts.with(side);
        }
    }
    mesh.opaque.append(&mut strip);
    added
}

/// Appends only the skirt cells needed to reach a lower adjacent column.
///
/// This is the drop-aware sibling of [`add_skirts`]. [`add_skirts`] always
/// continues every exposed boundary top face downward; this variant reads the
/// coarse padded scratch and fills only the vertical step to the column just
/// outside `edges`:
///
/// - a flush edge (the adjacent coarse column reaches or passes the boundary
///   top) appends nothing;
/// - a one-cell drop appends one cell;
/// - a larger drop or an open neighbour appends at most [`SKIRT_DEPTH`] cells.
///
/// Cells below the section floor are never emitted. Records, edge order, the
/// per-edge normal, fully open AO (`0xFF`, so a skirt never darkens), `sky`/`blk`
/// from the boundary face, and [`SectionMesh::skirts`] all match [`add_skirts`];
/// a repeated call returns `0`. The return value is the number of quads added.
pub fn add_step_skirts(
    mesh: &mut SectionMesh,
    blocks: &PaddedBlocks,
    props: &MaterialProps,
    edges: SkirtEdges,
    lod: MeshLod,
) -> u32 {
    if edges.is_empty() || mesh.opaque.is_empty() {
        return 0;
    }
    let depth = SKIRT_DEPTH.unsigned_abs();
    debug_assert_eq!(
        skirt_blocks(lod),
        depth * lod.cells_per_block(),
        "a skirt spans SKIRT_DEPTH cells, so its block height scales with the level"
    );
    let mut strip = Vec::new();
    for side in Side::ALL {
        if edges.contains(side) && !mesh.skirts.contains(side) {
            push_side_step_skirts(&mut strip, mesh, blocks, props, side, depth);
        }
    }
    let added = u32::try_from(strip.len()).expect("a skirt batch fits u32");
    for side in Side::ALL {
        if edges.contains(side) {
            mesh.skirts = mesh.skirts.with(side);
        }
    }
    mesh.opaque.append(&mut strip);
    added
}

/// Appends one side's merged drop-aware runs, top cell first.
fn push_side_step_skirts(
    out: &mut Vec<Quad>,
    mesh: &SectionMesh,
    blocks: &PaddedBlocks,
    props: &MaterialProps,
    side: Side,
    depth: u32,
) {
    let cells = boundary_cells(mesh, side);
    let mut column = 0;
    while column < PLANE_WIDTH {
        let Some(cell) = cells[column] else {
            column += 1;
            continue;
        };
        let layers = step_layers(blocks, props, side, column, cell.y, depth);
        if layers == 0 {
            column += 1;
            continue;
        }
        let mut run = 1;
        while column + run < PLANE_WIDTH {
            let Some(next) = cells[column + run] else {
                break;
            };
            let next_layers = step_layers(blocks, props, side, column + run, next.y, depth);
            if next != cell || next_layers != layers {
                break;
            }
            run += 1;
        }
        for layer in 1..=layers {
            let y = cell.y - u8::try_from(layer).expect("a layer fits one cell index");
            push_skirt(out, side, column, run, y, cell);
        }
        column += run;
    }
}

/// Cells needed to bridge a boundary column down to its adjacent coarse column.
fn step_layers(
    blocks: &PaddedBlocks,
    props: &MaterialProps,
    side: Side,
    column: usize,
    top: u8,
    depth: u32,
) -> u32 {
    let drop = match neighbour_top(blocks, props, side, column) {
        Some(adjacent) if adjacent >= top => 0,
        Some(adjacent) => u32::from(top).saturating_sub(u32::from(adjacent)),
        None => u32::from(top).saturating_add(1),
    };
    // Never go below the section floor; `top` is at most 31.
    drop.min(depth).min(u32::from(top))
}

/// Highest opaque cell in the coarse column just outside `side`.
fn neighbour_top(
    blocks: &PaddedBlocks,
    props: &MaterialProps,
    side: Side,
    column: usize,
) -> Option<u8> {
    let column = i32::try_from(column).expect("a column fits i32");
    let (x, z) = match side {
        Side::North => (column, LOCAL_MIN),
        Side::South => (column, LOCAL_MAX),
        Side::East => (LOCAL_MAX, column),
        Side::West => (LOCAL_MIN, column),
    };
    (LOCAL_MIN..=LOCAL_MAX)
        .rev()
        .find(|&y| props.material(blocks.get(x, y, z)).opaque)
        .and_then(|y| u8::try_from(y).ok())
}

/// Appends one side's merged runs, top cell first.
fn push_side_skirts(out: &mut Vec<Quad>, mesh: &SectionMesh, side: Side, depth: u32) {
    let cells = boundary_cells(mesh, side);
    let mut column = 0;
    while column < PLANE_WIDTH {
        let Some(cell) = cells[column] else {
            column += 1;
            continue;
        };
        let mut run = 1;
        while column + run < PLANE_WIDTH && cells[column + run] == Some(cell) {
            run += 1;
        }
        let layers = u32::from(cell.y).min(depth);
        // Start one cell below the boundary face: its own side wall already
        // covers `cell.y`, and a coplanar duplicate would z-fight.
        for layer in 1..=layers {
            let y = cell.y - u8::try_from(layer).expect("a layer fits one cell index");
            push_skirt(out, side, column, run, y, cell);
        }
        column += run;
    }
}

/// Appends one merged skirt quad of one cell layer.
fn push_skirt(out: &mut Vec<Quad>, side: Side, column: usize, run: usize, y: u8, cell: SkirtCell) {
    let start = u8::try_from(column).expect("a column fits one cell index");
    let width = u8::try_from(run - 1).expect("a run fits one record width");
    let plane = side.plane();
    let (x, z) = match side {
        Side::North | Side::South => (start, plane),
        Side::East | Side::West => (plane, start),
    };
    out.push(Quad::encode(
        x,
        y,
        z,
        width,
        0,
        side.normal(),
        AO_OPEN,
        cell.sky,
        cell.blk,
        cell.tile,
        cell.tint,
    ));
}

/// The lowest exposed top face of every boundary column along `side`.
fn boundary_cells(mesh: &SectionMesh, side: Side) -> [Option<SkirtCell>; PLANE_WIDTH] {
    let mut cells: [Option<SkirtCell>; PLANE_WIDTH] = [None; PLANE_WIDTH];
    for quad in &mesh.opaque {
        let Some((start, end)) = boundary_span(*quad, side) else {
            continue;
        };
        let candidate = SkirtCell {
            y: quad.y(),
            sky: quad.sky(),
            blk: quad.blk(),
            tile: quad.tile(),
            tint: quad.tint(),
        };
        for column in start..=end {
            let slot = usize::from(column);
            if slot >= PLANE_WIDTH {
                continue;
            }
            if cells[slot].is_none_or(|current| current.y > candidate.y) {
                cells[slot] = Some(candidate);
            }
        }
    }
    cells
}

/// The inclusive column range a `+Y` face covers on `side`'s boundary plane.
fn boundary_span(quad: Quad, side: Side) -> Option<(u8, u8)> {
    if quad.normal() != NORMAL_POS_Y {
        return None;
    }
    let (plane, thickness, along, length) = match side {
        Side::North | Side::South => (quad.z(), quad.h1(), quad.x(), quad.w1()),
        Side::East | Side::West => (quad.x(), quad.w1(), quad.z(), quad.h1()),
    };
    let on_plane = match side {
        Side::North | Side::West => plane == 0,
        Side::South | Side::East => plane.checked_add(thickness) == Some(side.plane()),
    };
    if !on_plane {
        return None;
    }
    Some((along, along.checked_add(length)?))
}
