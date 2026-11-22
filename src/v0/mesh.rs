//! Greedy chunk meshing, LOD downsampling, and LOD selection.
//!
//! Chunk meshes are built in chunk-local space; the renderer adds each chunk's
//! origin. One greedy mesher serves both LODs: LOD0 works on `32` cells of one
//! block, LOD1 on `16` cells of `2 × 2 × 2` blocks.

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::Arc;

use glam::IVec3;

use crate::block::Block;
use crate::light::LightField;
use crate::quad::{FLAG_WATER, QuadRecord};
use crate::voxel::Vertex;
use crate::world::{CHUNK_SIZE, CHUNK_STORAGE_BYTES, Chunk, ChunkCoord, World, packed_block};

/// Distance in chunk units at or below which a chunk renders at LOD0.
pub(crate) const LOD0_DISTANCE: f32 = 4.0;

/// Distance in chunk units at or above which a chunk renders at LOD1.
pub(crate) const LOD1_DISTANCE: f32 = 8.0;

/// Distance in chunk units at or above which a chunk renders at LOD2.
pub(crate) const LOD2_DISTANCE: f32 = 16.0;

/// Three rendering levels of detail.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum Lod {
    Lod0,
    Lod1,
    Lod2,
}

impl Lod {
    /// Blocks per cell, used for positions and UVs.
    pub(crate) fn scale(self) -> f32 {
        match self {
            Self::Lod0 => 1.0,
            Self::Lod1 => 2.0,
            Self::Lod2 => 4.0,
        }
    }

    /// Blocks per cell on each axis.
    fn cell_scale(self) -> i32 {
        match self {
            Self::Lod0 => 1,
            Self::Lod1 => 2,
            Self::Lod2 => 4,
        }
    }

    /// Cells per chunk axis.
    fn cell_count(self) -> i32 {
        CHUNK_SIZE / self.cell_scale()
    }
}

/// Picks a LOD from the current one and the chunk distance, with hysteresis.
///
/// `current` is `None` on the first selection. Distances between thresholds
/// retain the current LOD so boundaries do not flicker.
pub(crate) fn select_lod(current: Option<Lod>, distance: f32) -> Lod {
    let Some(current) = current else {
        return select_lod(Some(Lod::Lod0), distance);
    };

    match current {
        Lod::Lod0 => {
            if distance >= LOD2_DISTANCE {
                Lod::Lod2
            } else if distance >= LOD1_DISTANCE {
                Lod::Lod1
            } else {
                Lod::Lod0
            }
        }
        Lod::Lod1 => {
            if distance >= LOD2_DISTANCE {
                Lod::Lod2
            } else if distance <= LOD0_DISTANCE {
                Lod::Lod0
            } else {
                Lod::Lod1
            }
        }
        Lod::Lod2 => {
            if distance <= LOD0_DISTANCE {
                Lod::Lod0
            } else if distance <= LOD1_DISTANCE {
                Lod::Lod1
            } else {
                Lod::Lod2
            }
        }
    }
}

/// Immutable full-resolution copy of one chunk's blocks.
#[derive(Clone, Debug, Default)]
pub(crate) struct ChunkSnapshot {
    cells: Arc<Vec<u8>>,
    water: bool,
}

impl ChunkSnapshot {
    /// Copies a chunk out of the world. A missing chunk becomes all air.
    pub(crate) fn new(world: &World, chunk: ChunkCoord) -> Self {
        let chunk_data = world.chunk(chunk);
        let cells = chunk_data.map_or_else(
            || Arc::new(vec![0; CHUNK_STORAGE_BYTES]),
            Chunk::shared_storage,
        );
        Self {
            cells,
            water: chunk_data.is_some_and(Chunk::has_water),
        }
    }

    /// Wraps an existing packed grid without copying it.
    pub(crate) fn from_chunk(data: &Chunk) -> Self {
        Self {
            cells: data.shared_storage(),
            water: data.has_water(),
        }
    }

    /// Returns the block at a chunk-local coordinate, or air outside the chunk.
    pub(crate) fn block(&self, local: IVec3) -> Block {
        if in_chunk(local) {
            packed_block(&self.cells, index(local))
        } else {
            Block::Air
        }
    }

    /// Whether the source chunk held any water.
    pub(crate) fn has_water(&self) -> bool {
        self.water
    }
}

/// The one-cell border around a chunk, keyed by the owning chunk's local
/// coordinates (so a `-X` neighbour lands at `x = -1`).
#[derive(Clone, Debug, Default)]
pub(crate) struct BorderBlocks {
    blocks: HashMap<IVec3, Block>,
}

impl BorderBlocks {
    /// Copies the non-air border cells of the six neighbouring chunks.
    pub(crate) fn new(world: &World, chunk: ChunkCoord) -> Self {
        let mut blocks = HashMap::new();
        for normal in FACE_NORMALS {
            let Some(data) = world.chunk(chunk.offset(normal)) else {
                continue;
            };
            for u in 0..CHUNK_SIZE {
                for v in 0..CHUNK_SIZE {
                    let local = face_local(normal, u, v);
                    let block = data.block(local);
                    if block != Block::Air {
                        blocks.insert(local + normal * CHUNK_SIZE, block);
                    }
                }
            }
        }
        Self { blocks }
    }

    /// Returns the border block at an out-of-chunk local coordinate.
    pub(crate) fn block(&self, local: IVec3) -> Block {
        self.blocks.get(&local).copied().unwrap_or(Block::Air)
    }
}

/// Builds one non-indexed greedy water mesh for a coarse section grid.
///
/// `y_range` limits the sample layers scanned. It must cover every water
/// sample; callers pass a conservative range so empty ranges are skipped
/// without changing which faces are emitted.
pub(crate) fn build_section_water_mesh(
    snapshot: &ChunkSnapshot,
    border: &BorderBlocks,
    light: Option<&LightField>,
    scale: f32,
    y_range: RangeInclusive<i32>,
) -> MeshData {
    if !snapshot.has_water() {
        return MeshData::default();
    }
    let full_block = |local: IVec3| full_block(snapshot, border, local);
    let matches = |block: Block| block == Block::Water;
    let exposed = |cell: IVec3, normal: IVec3| full_block(cell + normal) == Block::Air;
    let corners = |anchor: IVec3, face: usize| water_corners(&full_block, light, anchor, face);
    greedy(
        CHUNK_SIZE,
        scale,
        &full_block,
        &matches,
        &exposed,
        &|cell| cell,
        &corners,
        crate::quad::FLAG_WATER,
        y_range,
    )
}

/// Builds one non-indexed greedy mesh for a coarse section sample grid.
///
/// `scale` is the section's sample spacing in blocks. Positions and UVs are in
/// sample units multiplied by `scale`; face order, AO, and light packing match
/// the chunk mesher. `y_range` has the same contract as the water variant.
pub(crate) fn build_section_mesh(
    snapshot: &ChunkSnapshot,
    border: &BorderBlocks,
    light: Option<&LightField>,
    scale: f32,
    y_range: RangeInclusive<i32>,
) -> MeshData {
    let full_block = |local: IVec3| full_block(snapshot, border, local);
    let matches = Block::is_solid;
    let exposed = |cell: IVec3, normal: IVec3| !full_block(cell + normal).is_solid();
    let corners = |anchor: IVec3, face: usize| corner_values(&full_block, light, anchor, face);
    greedy(
        CHUNK_SIZE,
        scale,
        &full_block,
        &matches,
        &exposed,
        &|cell| cell,
        &corners,
        0,
        y_range,
    )
}

/// Builds one non-indexed greedy mesh for a chunk snapshot.
///
/// `light` supplies per-corner sky and block light; without it, light values are
/// zero and only AO is baked.
pub(crate) fn build_mesh(
    snapshot: &ChunkSnapshot,
    border: &BorderBlocks,
    light: Option<&LightField>,
    lod: Lod,
) -> MeshData {
    let full_block = |local: IVec3| full_block(snapshot, border, local);

    let matches = Block::is_solid;
    let corners = |anchor: IVec3, face: usize| corner_values(&full_block, light, anchor, face);

    match lod {
        Lod::Lod0 => {
            let exposed = |cell: IVec3, normal: IVec3| !full_block(cell + normal).is_solid();
            greedy(
                lod.cell_count(),
                lod.scale(),
                &full_block,
                &matches,
                &exposed,
                &|cell| cell,
                &corners,
                0,
                0..=lod.cell_count() - 1,
            )
        }
        Lod::Lod1 | Lod::Lod2 => {
            let size = lod.cell_count();
            let (cells, representatives) = downsample(snapshot, size, lod.cell_scale());
            let cell_block = |cell: IVec3| {
                if in_grid(cell, size) {
                    cells[index_in(cell, size)]
                } else {
                    Block::Air
                }
            };
            let anchor = |cell: IVec3| {
                if in_grid(cell, size) {
                    representatives[index_in(cell, size)]
                } else {
                    IVec3::ZERO
                }
            };
            let exposed = |cell: IVec3, normal: IVec3| !cell_block(cell + normal).is_solid();
            greedy(
                lod.cell_count(),
                lod.scale(),
                &cell_block,
                &matches,
                &exposed,
                &anchor,
                &corners,
                0,
                0..=lod.cell_count() - 1,
            )
        }
    }
}

/// Builds the transparent water mesh: full resolution, water cells only, and
/// faces emitted only against air.
pub(crate) fn build_water_mesh(
    snapshot: &ChunkSnapshot,
    border: &BorderBlocks,
    light: Option<&LightField>,
) -> MeshData {
    if !snapshot.has_water() {
        return MeshData::default();
    }

    let full_block = |local: IVec3| full_block(snapshot, border, local);
    let matches = |block: Block| block == Block::Water;
    let exposed = |cell: IVec3, normal: IVec3| full_block(cell + normal) == Block::Air;
    let corners = |anchor: IVec3, face: usize| water_corners(&full_block, light, anchor, face);

    greedy(
        CHUNK_SIZE,
        1.0,
        &full_block,
        &matches,
        &exposed,
        &|cell| cell,
        &corners,
        FLAG_WATER,
        0..=CHUNK_SIZE - 1,
    )
}

fn full_block(snapshot: &ChunkSnapshot, border: &BorderBlocks, local: IVec3) -> Block {
    if in_chunk(local) {
        snapshot.block(local)
    } else {
        border.block(local)
    }
}

/// Downsamples a chunk to one block per LOD cell: solid when any source block
/// is solid, typed by the topmost solid source, then lowest X, then lowest Z.
///
/// Also returns the full-resolution local coordinate of each cell's
/// representative block, which light and AO sampling use.
fn downsample(snapshot: &ChunkSnapshot, size: i32, scale: i32) -> (Box<[Block]>, Box<[IVec3]>) {
    let cells = usize::try_from(size * size * size).expect("grid fits");
    let mut grid = vec![Block::Air; cells].into_boxed_slice();
    let mut representatives = vec![IVec3::ZERO; cells].into_boxed_slice();
    for cell in grid_coords(size) {
        let (block, representative) = downsample_cell(snapshot, cell, scale);
        let index = index_in(cell, size);
        grid[index] = block;
        representatives[index] = representative;
    }
    (grid, representatives)
}

fn downsample_cell(snapshot: &ChunkSnapshot, cell: IVec3, scale: i32) -> (Block, IVec3) {
    let base = cell * scale;
    for y in (0..scale).rev() {
        for x in 0..scale {
            for z in 0..scale {
                let local = base + IVec3::new(x, y, z);
                let block = snapshot.block(local);
                if block.is_solid() {
                    return (block, local);
                }
            }
        }
    }
    (Block::Air, IVec3::ZERO)
}

/// Face order from the v0.0.5 spec: `+X`, `-X`, `+Y`, `-Y`, `+Z`, `-Z`.
const FACE_NORMALS: [IVec3; 6] = [
    IVec3::X,
    IVec3::NEG_X,
    IVec3::Y,
    IVec3::NEG_Y,
    IVec3::Z,
    IVec3::NEG_Z,
];

/// A chunk mesh: four vertices per quad plus triangle indices.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MeshData {
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) indices: Vec<u32>,
    /// The ADR-0015 records for the same quads, uploaded by the quad-pool renderer.
    pub(crate) quads: Vec<QuadRecord>,
}

impl MeshData {
    pub(crate) fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// Builds a width-first greedy mesh of the solid cells returned by `block_at`.
///
/// `anchor` maps a cell to the full-resolution coordinate whose light and AO
/// are sampled; LOD0 uses the cell itself and LOD1 uses its representative
/// block. `full_block_at` resolves solidity for that sampling.
#[allow(clippy::too_many_arguments)]
fn greedy(
    n: i32,
    scale: f32,
    block_at: &impl Fn(IVec3) -> Block,
    matches: &impl Fn(Block) -> bool,
    exposed: &impl Fn(IVec3, IVec3) -> bool,
    anchor: &impl Fn(IVec3) -> IVec3,
    corners: &impl Fn(IVec3, usize) -> [u32; 4],
    flags: u32,
    y_range: RangeInclusive<i32>,
) -> MeshData {
    let cells = usize::try_from(n * n).expect("mask fits");
    let mut mesh = MeshData::default();
    let mut mask: Vec<Option<MaskKey>> = vec![None; cells];
    let y_min = (*y_range.start()).clamp(0, n - 1);
    let y_max = (*y_range.end()).clamp(0, n - 1);

    for (face, normal) in FACE_NORMALS.into_iter().enumerate() {
        // Faces `+Y`/`-Y` slice along Y and scan the full XZ grid; every other
        // face has Y as its `v` axis. Skipping the out-of-range layers is
        // exact because a matching block cannot exist outside the caller's
        // conservative range, so the mask entries stay `None`.
        let y_is_slice = matches!(face, 2 | 3);
        for slice in 0..n {
            if y_is_slice && !(y_min..=y_max).contains(&slice) {
                continue;
            }
            mask.fill(None);
            let (v_start, v_end) = if y_is_slice {
                (0, n - 1)
            } else {
                (y_min, y_max)
            };
            for v in v_start..=v_end {
                for u in 0..n {
                    let cell = cell_coord(face, slice, u, v);
                    let block = block_at(cell);
                    if !matches(block) || !exposed(cell, normal) {
                        continue;
                    }
                    mask[mask_index(u, v, n)] = Some(MaskKey {
                        block,
                        layer: face_layer(block, normal),
                        corners: corners(anchor(cell), face),
                    });
                }
            }

            for v in 0..n {
                for u in 0..n {
                    let Some(key) = mask[mask_index(u, v, n)] else {
                        continue;
                    };

                    let mut width = 1;
                    while u + width < n && mask[mask_index(u + width, v, n)] == Some(key) {
                        width += 1;
                    }

                    let mut height = 1;
                    'height: while v + height < n {
                        for offset in 0..width {
                            if mask[mask_index(u + offset, v + height, n)] != Some(key) {
                                break 'height;
                            }
                        }
                        height += 1;
                    }

                    for row in 0..height {
                        for column in 0..width {
                            mask[mask_index(u + column, v + row, n)] = None;
                        }
                    }

                    push_quad(
                        &mut mesh,
                        face,
                        normal,
                        (slice, u, v, width, height),
                        scale,
                        key.layer,
                        key.corners,
                        flags,
                    );
                }
            }
        }
    }

    mesh
}

/// Greedy merge key for one exposed cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MaskKey {
    block: Block,
    layer: u32,
    /// Packed light/AO at the four cell corners, in quad corner order.
    corners: [u32; 4],
}

/// Ambient occlusion and light at the four corners of one exposed face.
///
/// Each corner inspects the outside cell and its two edge neighbours plus the
/// diagonal in the plane of the face. Two solid edges fully occlude the corner;
/// light is the component-wise maximum over the non-solid cells in that set.
fn corner_values(
    block_at: &impl Fn(IVec3) -> Block,
    light: Option<&LightField>,
    anchor: IVec3,
    face: usize,
) -> [u32; 4] {
    let ring = FaceNeighbourhood::new(block_at, light, anchor, face);
    [(-1, -1), (-1, 1), (1, 1), (1, -1)].map(|(su, sv)| {
        let (sky, block, ao) = ring.corner(su, sv);
        pack_light_ao(sky, block, ao)
    })
}

/// Solidity and light of the nine cells around a face's outside cell.
///
/// Every corner of a face samples this 3x3 ring, so gathering it once avoids
/// re-reading the same cells for each corner.
struct FaceNeighbourhood {
    solid: [[bool; 3]; 3],
    sky: [[u8; 3]; 3],
    block: [[u8; 3]; 3],
}

impl FaceNeighbourhood {
    fn new(
        block_at: &impl Fn(IVec3) -> Block,
        light: Option<&LightField>,
        anchor: IVec3,
        face: usize,
    ) -> Self {
        let outside = anchor + FACE_NORMALS[face];
        let (u_axis, v_axis) = face_axes(face);
        let mut neighbourhood = Self {
            solid: [[false; 3]; 3],
            sky: [[0; 3]; 3],
            block: [[0; 3]; 3],
        };
        for su in -1..=1 {
            for sv in -1..=1 {
                let i = ring_index(su);
                let j = ring_index(sv);
                let cell = outside + u_axis * su + v_axis * sv;
                let solid = block_at(cell).is_solid();
                neighbourhood.solid[i][j] = solid;
                if !solid && let Some(light) = light {
                    neighbourhood.sky[i][j] = light.sky(cell);
                    neighbourhood.block[i][j] = light.block(cell);
                }
            }
        }
        neighbourhood
    }

    /// Sky, block, and AO for the corner in direction `(su, sv)`.
    fn corner(&self, su: i32, sv: i32) -> (u8, u8, u8) {
        let i = ring_index(su);
        let j = ring_index(sv);
        let solid_a = self.solid[i][1];
        let solid_b = self.solid[1][j];
        let solid_diagonal = self.solid[i][j];
        let ao = if solid_a && solid_b {
            0
        } else {
            3 - (u8::from(solid_a) + u8::from(solid_b) + u8::from(solid_diagonal))
        };
        let sky = self.sky[i][1]
            .max(self.sky[1][j])
            .max(self.sky[i][j])
            .max(self.sky[1][1]);
        let block = self.block[i][1]
            .max(self.block[1][j])
            .max(self.block[i][j])
            .max(self.block[1][1]);
        (sky, block, ao)
    }
}

fn ring_index(delta: i32) -> usize {
    usize::try_from(delta + 1).expect("ring index fits")
}

/// Packs sky light, block light, and AO into the vertex field.
pub(crate) fn pack_light_ao(sky: u8, block: u8, ao: u8) -> u32 {
    u32::from(sky & 0xF) | (u32::from(block & 0xF) << 4) | (u32::from(ao & 0x3) << 8)
}

/// Water corners take the water cell's own light, uniform across the face,
/// with the same AO rule as solid faces.
fn water_corners(
    block_at: &impl Fn(IVec3) -> Block,
    light: Option<&LightField>,
    anchor: IVec3,
    face: usize,
) -> [u32; 4] {
    let ring = FaceNeighbourhood::new(block_at, None, anchor, face);
    let (sky, block) = light.map_or((0, 0), |light| (light.sky(anchor), light.block(anchor)));

    [(-1, -1), (-1, 1), (1, 1), (1, -1)].map(|(su, sv)| {
        let (_, _, ao) = ring.corner(su, sv);
        pack_light_ao(sky, block, ao)
    })
}

fn face_axes(face: usize) -> (IVec3, IVec3) {
    match face {
        0 | 1 => (IVec3::Z, IVec3::Y),
        2 | 3 => (IVec3::X, IVec3::Z),
        _ => (IVec3::X, IVec3::Y),
    }
}

/// Cell coordinate for `(slice, u, v)` on a face, from the spec's table.
fn cell_coord(face: usize, slice: i32, u: i32, v: i32) -> IVec3 {
    match face {
        0 | 1 => IVec3::new(slice, v, u),
        2 | 3 => IVec3::new(u, slice, v),
        _ => IVec3::new(u, v, slice),
    }
}

fn mask_index(u: i32, v: i32, n: i32) -> usize {
    usize::try_from(v * n + u).expect("mask index fits")
}

fn face_layer(block: Block, normal: IVec3) -> u32 {
    let layers = block.layers().expect("solid blocks have texture layers");
    match normal.y.cmp(&0) {
        std::cmp::Ordering::Greater => layers.top,
        std::cmp::Ordering::Less => layers.bottom,
        std::cmp::Ordering::Equal => layers.side,
    }
}

/// Emits one merged quad with UVs in block units.
#[allow(clippy::cast_precision_loss)]
#[allow(clippy::too_many_arguments)]
fn push_quad(
    mesh: &mut MeshData,
    face: usize,
    normal: IVec3,
    quad: (i32, i32, i32, i32, i32),
    scale: f32,
    layer: u32,
    corner_light: [u32; 4],
    flags: u32,
) {
    let (slice, u0, v0, width, height) = quad;
    let corners = quad_corners(face, slice, u0, v0, width, height);
    let us = width as f32 * scale;
    let vs = height as f32 * scale;
    let uvs = [[0.0, 0.0], [0.0, vs], [us, vs], [us, 0.0]];
    let normal = normal.as_vec3().to_array();
    let base = u32::try_from(mesh.vertices.len()).expect("vertex count fits in u32");

    mesh.quads.push(QuadRecord::new(
        cell_coord(face, slice, u0, v0),
        u32::try_from(width).expect("width fits"),
        u32::try_from(height).expect("height fits"),
        u8::try_from(face).expect("face fits"),
        flags,
        u8::try_from(layer).expect("layer fits"),
        corner_light,
    ));

    for index in 0..4 {
        mesh.vertices.push(Vertex::new(
            corners[index].map(|coordinate| coordinate * scale),
            normal,
            uvs[index],
            layer,
            corner_light[index],
        ));
    }
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// Quad corners in cell units, in the spec's corner order.
#[allow(clippy::cast_precision_loss)]
fn quad_corners(
    face: usize,
    slice: i32,
    u0: i32,
    v0: i32,
    width: i32,
    height: i32,
) -> [[f32; 3]; 4] {
    let s = slice as f32;
    let u = u0 as f32;
    let v = v0 as f32;
    let right = (u0 + width) as f32;
    let top = (v0 + height) as f32;
    let forward = s + 1.0;

    match face {
        0 => [
            [forward, v, u],
            [forward, top, u],
            [forward, top, right],
            [forward, v, right],
        ],
        1 => [[s, v, right], [s, top, right], [s, top, u], [s, v, u]],
        2 => [
            [u, forward, v],
            [u, forward, top],
            [right, forward, top],
            [right, forward, v],
        ],
        3 => [[u, s, top], [u, s, v], [right, s, v], [right, s, top]],
        4 => [
            [right, v, forward],
            [right, top, forward],
            [u, top, forward],
            [u, v, forward],
        ],
        _ => [[u, v, s], [u, top, s], [right, top, s], [right, v, s]],
    }
}

fn in_chunk(local: IVec3) -> bool {
    in_grid(local, CHUNK_SIZE)
}

fn in_grid(local: IVec3, size: i32) -> bool {
    local.cmpge(IVec3::ZERO).all() && local.cmplt(IVec3::splat(size)).all()
}

fn index(local: IVec3) -> usize {
    index_in(local, CHUNK_SIZE)
}

fn index_in(local: IVec3, size: i32) -> usize {
    let stride = usize::try_from(size).expect("grid size fits");
    let x = usize::try_from(local.x).expect("in-grid coordinate fits");
    let y = usize::try_from(local.y).expect("in-grid coordinate fits");
    let z = usize::try_from(local.z).expect("in-grid coordinate fits");
    (x * stride + y) * stride + z
}

fn grid_coords(size: i32) -> impl Iterator<Item = IVec3> {
    (0..size)
        .flat_map(move |x| (0..size).flat_map(move |y| (0..size).map(move |z| IVec3::new(x, y, z))))
}

/// The neighbour-chunk cell on the face shared with the owning chunk.
fn face_local(normal: IVec3, u: i32, v: i32) -> IVec3 {
    let near = if normal.cmplt(IVec3::ZERO).any() {
        CHUNK_SIZE - 1
    } else {
        0
    };
    if normal.x != 0 {
        IVec3::new(near, u, v)
    } else if normal.y != 0 {
        IVec3::new(u, near, v)
    } else {
        IVec3::new(u, v, near)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with(blocks: &[(IVec3, Block)]) -> World {
        let mut world = World::default();
        for &(coordinate, block) in blocks {
            world.set_block(coordinate, block);
        }
        world
    }

    fn chunk_at(world: IVec3) -> ChunkCoord {
        ChunkCoord::from_world(world).0
    }

    fn mesh(world: &World, chunk: ChunkCoord, lod: Lod) -> Vec<Vertex> {
        build_mesh(
            &ChunkSnapshot::new(world, chunk),
            &BorderBlocks::new(world, chunk),
            None,
            lod,
        )
        .vertices
    }

    fn lit_mesh(world: &World, chunk: ChunkCoord, lod: Lod) -> Vec<Vertex> {
        build_mesh(
            &ChunkSnapshot::new(world, chunk),
            &BorderBlocks::new(world, chunk),
            world.light_field(chunk).map(std::convert::AsRef::as_ref),
            lod,
        )
        .vertices
    }

    fn water_mesh(world: &World, chunk: ChunkCoord, lit: bool) -> Vec<Vertex> {
        build_water_mesh(
            &ChunkSnapshot::new(world, chunk),
            &BorderBlocks::new(world, chunk),
            if lit {
                world.light_field(chunk).map(std::convert::AsRef::as_ref)
            } else {
                None
            },
        )
        .vertices
    }

    fn ao(aos: [u8; 4]) -> [u32; 4] {
        aos.map(|ao| pack_light_ao(0, 0, ao))
    }

    /// One expected quad: `(face, slice, u0, v0, width, height, scale, layer)`.
    type Quad = (usize, i32, i32, i32, i32, i32, f32, u32);

    #[allow(clippy::cast_precision_loss)]
    fn quad(spec: Quad) -> Vec<Vertex> {
        let (face, slice, u0, v0, width, height, scale, layer) = spec;
        let normal = FACE_NORMALS[face].as_vec3().to_array();
        let us = width as f32 * scale;
        let vs = height as f32 * scale;
        let uvs = [[0.0, 0.0], [0.0, vs], [us, vs], [us, 0.0]];
        let corners = quad_corners(face, slice, u0, v0, width, height);
        (0..4)
            .map(|index| {
                Vertex::new(
                    corners[index].map(|coordinate| coordinate * scale),
                    normal,
                    uvs[index],
                    layer,
                    pack_light_ao(0, 0, 3),
                )
            })
            .collect()
    }

    fn expected(quads: &[Quad]) -> Vec<Vertex> {
        quads.iter().flat_map(|&spec| quad(spec)).collect()
    }

    #[test]
    fn one_voxel_matches_the_spec_corner_table() {
        let world = world_with(&[(IVec3::new(3, 4, 5), Block::Stone)]);
        let vertices = mesh(&world, chunk_at(IVec3::new(3, 4, 5)), Lod::Lod0);

        assert_eq!(
            vertices,
            expected(&[
                (0, 3, 5, 4, 1, 1, 1.0, 0),
                (1, 3, 5, 4, 1, 1, 1.0, 0),
                (2, 4, 3, 5, 1, 1, 1.0, 0),
                (3, 4, 3, 5, 1, 1, 1.0, 0),
                (4, 5, 3, 4, 1, 1, 1.0, 0),
                (5, 5, 3, 4, 1, 1, 1.0, 0),
            ])
        );
        assert_eq!(vertices.len(), 24);
    }

    #[test]
    fn quad_records_match_the_emitted_vertices() {
        let world = world_with(&[(IVec3::new(5, 5, 5), Block::Stone)]);
        let chunk = chunk_at(IVec3::new(5, 5, 5));
        let snapshot = ChunkSnapshot::new(&world, chunk);
        let border = BorderBlocks::default();
        let mesh = build_mesh(&snapshot, &border, None, Lod::Lod0);

        assert_eq!(mesh.quads.len(), 6);
        assert_eq!(mesh.vertices.len(), 24);
        for (index, quad) in mesh.quads.iter().enumerate() {
            assert_eq!(quad.position(), IVec3::new(5, 5, 5));
            assert_eq!((quad.width(), quad.height()), (1, 1));
            assert!(!quad.has_flag(FLAG_WATER));

            let corners = quad.corners();
            for (corner, cell) in corners.iter().enumerate() {
                let vertex = &mesh.vertices[index * 4 + corner];
                assert_eq!(
                    vertex.cell,
                    [
                        u8::try_from(cell.x).unwrap(),
                        u8::try_from(cell.y).unwrap(),
                        u8::try_from(cell.z).unwrap()
                    ]
                );
                assert_eq!(u32::from(vertex.face), u32::from(quad.face()));
                assert_eq!(u32::from(vertex.texture_layer), u32::from(quad.layer()));
                let (sky, block, ao) = quad.corner(corner);
                assert_eq!(vertex.light_ao, pack_light_ao(sky, block, ao));
            }
        }
    }

    #[test]
    fn water_quad_records_are_flagged() {
        let world = world_with(&[(IVec3::new(2, 2, 2), Block::Water)]);
        let chunk = chunk_at(IVec3::new(2, 2, 2));
        let snapshot = ChunkSnapshot::new(&world, chunk);
        let border = BorderBlocks::default();

        let water = build_water_mesh(&snapshot, &border, None);
        assert_ne!(water.quads.len(), 0);
        assert!(water.quads.iter().all(|quad| quad.has_flag(FLAG_WATER)));

        let opaque = build_mesh(&snapshot, &border, None, Lod::Lod0);
        assert!(opaque.quads.iter().all(|quad| !quad.has_flag(FLAG_WATER)));
    }

    #[test]
    fn two_adjacent_voxels_merge_into_six_quads() {
        let world = world_with(&[(IVec3::ZERO, Block::Stone), (IVec3::X, Block::Stone)]);

        assert_eq!(mesh(&world, chunk_at(IVec3::ZERO), Lod::Lod0).len(), 24);
    }

    #[test]
    fn three_voxels_in_a_line_merge_into_six_quads() {
        let world = world_with(&[
            (IVec3::ZERO, Block::Stone),
            (IVec3::X, Block::Stone),
            (IVec3::new(2, 0, 0), Block::Stone),
        ]);

        assert_eq!(mesh(&world, chunk_at(IVec3::ZERO), Lod::Lod0).len(), 24);
    }

    #[test]
    fn slabs_and_cubes_merge_into_six_quads() {
        let mut slab = World::default();
        let mut cube = World::default();
        for x in 0..2 {
            for z in 0..2 {
                slab.set_block(IVec3::new(x, 0, z), Block::Dirt);
                for y in 0..2 {
                    cube.set_block(IVec3::new(x, y, z), Block::Dirt);
                }
            }
        }

        assert_eq!(mesh(&slab, chunk_at(IVec3::ZERO), Lod::Lod0).len(), 24);
        assert_eq!(mesh(&cube, chunk_at(IVec3::ZERO), Lod::Lod0).len(), 24);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn merged_quad_uses_block_unit_uvs_and_the_face_layer() {
        let mut world = World::default();
        for x in 0..2 {
            for z in 0..3 {
                world.set_block(IVec3::new(x, 0, z), Block::Grass);
            }
        }
        let vertices = mesh(&world, chunk_at(IVec3::ZERO), Lod::Lod0);

        let top: Vec<_> = vertices
            .iter()
            .filter(|vertex| vertex.normal() == [0.0, 1.0, 0.0])
            .collect();
        assert_eq!(top.len(), 4);
        let uvs: Vec<_> = top.iter().map(|vertex| vertex.uv()).collect();
        assert_eq!(uvs, vec![[0.0, 0.0], [0.0, 3.0], [2.0, 3.0], [2.0, 0.0]]);
        assert!(top.iter().all(|vertex| vertex.layer() == 2));
    }

    #[test]
    fn adjacent_voxels_across_a_chunk_border_emit_five_quads_each() {
        let world = world_with(&[
            (IVec3::new(31, 0, 0), Block::Stone),
            (IVec3::new(32, 0, 0), Block::Stone),
        ]);

        assert_eq!(
            mesh(&world, chunk_at(IVec3::new(31, 0, 0)), Lod::Lod0).len(),
            20
        );
        assert_eq!(
            mesh(&world, chunk_at(IVec3::new(32, 0, 0)), Lod::Lod0).len(),
            20
        );
    }

    #[test]
    fn greedy_uses_fewer_vertices_than_the_naive_mesher_on_flat_ground() {
        let mut world = World::default();
        for x in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                world.set_block(IVec3::new(x, 2, z), Block::Grass);
                world.set_block(IVec3::new(x, 1, z), Block::Dirt);
                world.set_block(IVec3::new(x, 0, z), Block::Stone);
            }
        }
        let chunk = chunk_at(IVec3::ZERO);

        let greedy = mesh(&world, chunk, Lod::Lod0).len();
        let naive = crate::voxel::build_chunk_mesh(&world, chunk).len();

        assert!(greedy < naive, "greedy {greedy} naive {naive}");
    }

    #[test]
    fn lod1_downsample_is_solid_when_any_source_is_solid() {
        let world = world_with(&[(IVec3::new(1, 1, 1), Block::Stone)]);
        let snapshot = ChunkSnapshot::new(&world, chunk_at(IVec3::ZERO));
        let (cells, _) = downsample(&snapshot, Lod::Lod1.cell_count(), Lod::Lod1.cell_scale());

        assert_eq!(
            cells[index_in(IVec3::ZERO, Lod::Lod1.cell_count())],
            Block::Stone
        );
        assert_eq!(
            cells[index_in(IVec3::X, Lod::Lod1.cell_count())],
            Block::Air
        );
    }

    #[test]
    fn lod1_downsample_takes_the_topmost_block_then_lowest_x_then_z() {
        let world = world_with(&[
            (IVec3::new(0, 0, 0), Block::Stone),
            (IVec3::new(0, 1, 0), Block::Dirt),
            (IVec3::new(1, 1, 0), Block::Grass),
            (IVec3::new(0, 0, 1), Block::Wood),
        ]);
        let snapshot = ChunkSnapshot::new(&world, chunk_at(IVec3::ZERO));

        assert_eq!(
            downsample(&snapshot, Lod::Lod1.cell_count(), Lod::Lod1.cell_scale()).0
                [index_in(IVec3::ZERO, Lod::Lod1.cell_count())],
            Block::Dirt
        );

        let mut tie = World::default();
        tie.set_block(IVec3::new(1, 1, 1), Block::Leaves);
        tie.set_block(IVec3::new(0, 1, 1), Block::Wood);
        let tie = downsample(
            &ChunkSnapshot::new(&tie, chunk_at(IVec3::ZERO)),
            Lod::Lod1.cell_count(),
            Lod::Lod1.cell_scale(),
        )
        .0;
        assert_eq!(
            tie[index_in(IVec3::ZERO, Lod::Lod1.cell_count())],
            Block::Wood
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn lod1_scales_positions_and_uvs_by_two() {
        let world = world_with(&[(IVec3::new(2, 2, 2), Block::Stone)]);
        let vertices = mesh(&world, chunk_at(IVec3::new(2, 2, 2)), Lod::Lod1);

        assert_eq!(
            vertices,
            expected(&[
                (0, 1, 1, 1, 1, 1, 2.0, 0),
                (1, 1, 1, 1, 1, 1, 2.0, 0),
                (2, 1, 1, 1, 1, 1, 2.0, 0),
                (3, 1, 1, 1, 1, 1, 2.0, 0),
                (4, 1, 1, 1, 1, 1, 2.0, 0),
                (5, 1, 1, 1, 1, 1, 2.0, 0),
            ])
        );
        let uv = vertices[1].uv();
        assert_eq!(uv, [0.0, 2.0]);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn lod1_emits_boundary_faces_even_next_to_solid_neighbours() {
        let world = world_with(&[
            (IVec3::new(31, 0, 0), Block::Stone),
            (IVec3::new(32, 0, 0), Block::Stone),
        ]);
        let chunk = chunk_at(IVec3::new(31, 0, 0));

        let lod0 = mesh(&world, chunk, Lod::Lod0);
        let lod1 = mesh(&world, chunk, Lod::Lod1);

        assert_eq!(lod0.len(), 20);
        assert_ne!(lod1.len(), 0);
        assert_eq!(
            lod1.iter()
                .filter(|vertex| vertex.normal() == [-1.0, 0.0, 0.0])
                .count(),
            4
        );
    }

    #[test]
    fn open_corners_have_full_ambient_occlusion() {
        let world = world_with(&[(IVec3::ZERO, Block::Stone)]);
        let chunk = chunk_at(IVec3::ZERO);
        let snapshot = ChunkSnapshot::new(&world, chunk);
        let border = BorderBlocks::new(&world, chunk);
        let block_at = |cell: IVec3| {
            if in_chunk(cell) {
                snapshot.block(cell)
            } else {
                border.block(cell)
            }
        };

        assert_eq!(
            corner_values(&block_at, None, IVec3::ZERO, 2),
            ao([3, 3, 3, 3])
        );
    }

    #[test]
    fn one_solid_edge_removes_one_occlusion_level() {
        let world = world_with(&[
            (IVec3::ZERO, Block::Stone),
            (IVec3::new(1, 1, 0), Block::Stone),
        ]);
        let chunk = chunk_at(IVec3::ZERO);
        let snapshot = ChunkSnapshot::new(&world, chunk);
        let border = BorderBlocks::new(&world, chunk);
        let block_at = |cell: IVec3| {
            if in_chunk(cell) {
                snapshot.block(cell)
            } else {
                border.block(cell)
            }
        };

        assert_eq!(
            corner_values(&block_at, None, IVec3::ZERO, 2),
            ao([3, 3, 2, 2])
        );
    }

    #[test]
    fn two_solid_edges_fully_occlude_a_corner() {
        let world = world_with(&[
            (IVec3::ZERO, Block::Stone),
            (IVec3::new(1, 1, 0), Block::Stone),
            (IVec3::new(0, 1, 1), Block::Stone),
        ]);
        let chunk = chunk_at(IVec3::ZERO);
        let snapshot = ChunkSnapshot::new(&world, chunk);
        let border = BorderBlocks::new(&world, chunk);
        let block_at = |cell: IVec3| {
            if in_chunk(cell) {
                snapshot.block(cell)
            } else {
                border.block(cell)
            }
        };

        assert_eq!(
            corner_values(&block_at, None, IVec3::ZERO, 2),
            ao([3, 2, 0, 2])
        );
    }

    #[test]
    fn a_diagonal_occluder_removes_one_occlusion_level() {
        let world = world_with(&[
            (IVec3::ZERO, Block::Stone),
            (IVec3::new(1, 1, 1), Block::Stone),
        ]);
        let chunk = chunk_at(IVec3::ZERO);
        let snapshot = ChunkSnapshot::new(&world, chunk);
        let border = BorderBlocks::new(&world, chunk);
        let block_at = |cell: IVec3| {
            if in_chunk(cell) {
                snapshot.block(cell)
            } else {
                border.block(cell)
            }
        };

        assert_eq!(
            corner_values(&block_at, None, IVec3::ZERO, 2),
            ao([3, 3, 2, 3])
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn greedy_splits_quads_when_ao_differs() {
        let mut flat = World::default();
        let mut occluded = World::default();
        for x in 0..4 {
            flat.set_block(IVec3::new(x, 0, 0), Block::Stone);
            occluded.set_block(IVec3::new(x, 0, 0), Block::Stone);
        }
        occluded.set_block(IVec3::new(0, 1, 1), Block::Stone);

        let top_vertices = |world: &World| {
            mesh(world, chunk_at(IVec3::ZERO), Lod::Lod0)
                .iter()
                .filter(|vertex| vertex.normal() == [0.0, 1.0, 0.0] && vertex.position()[1] == 1.0)
                .count()
        };

        assert_eq!(top_vertices(&flat), 4);
        assert_eq!(top_vertices(&occluded), 12);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn lit_faces_carry_sky_light() {
        let mut world = World::default();
        for x in 0..4 {
            for z in 0..4 {
                world.set_block(IVec3::new(x, 0, z), Block::Stone);
            }
        }
        world.relight();

        let vertices = lit_mesh(&world, chunk_at(IVec3::ZERO), Lod::Lod0);
        let top = vertices
            .iter()
            .find(|vertex| vertex.normal() == [0.0, 1.0, 0.0])
            .expect("top face");

        assert_eq!(top.light_ao & 0xF, 15);
        assert_eq!((top.light_ao >> 4) & 0xF, 0);
        assert_eq!((top.light_ao >> 8) & 0x3, 3);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn faces_carry_block_light_from_a_nearby_lamp() {
        let mut world = World::default();
        world.set_block(IVec3::ZERO, Block::Stone);
        world.set_block(IVec3::new(2, 0, 0), Block::Lamp);
        world.relight();

        let vertices = lit_mesh(&world, chunk_at(IVec3::ZERO), Lod::Lod0);
        let side = vertices
            .iter()
            .find(|vertex| vertex.normal() == [1.0, 0.0, 0.0])
            .expect("+X face");

        assert_eq!((side.light_ao >> 4) & 0xF, 13);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn lod1_light_uses_the_representative_block() {
        let mut world = World::default();
        for x in 0..4 {
            for z in 0..4 {
                world.set_block(IVec3::new(x, 0, z), Block::Stone);
            }
        }
        world.relight();

        let vertices = lit_mesh(&world, chunk_at(IVec3::ZERO), Lod::Lod1);
        let top = vertices
            .iter()
            .find(|vertex| vertex.normal() == [0.0, 1.0, 0.0] && vertex.position()[1] == 2.0)
            .expect("LOD1 top face");

        assert_eq!(top.light_ao & 0xF, 15);
        assert_eq!((top.light_ao >> 8) & 0x3, 3);
    }

    #[test]
    fn water_faces_are_emitted_only_against_air() {
        let world = world_with(&[
            (IVec3::ZERO, Block::Water),
            (IVec3::X, Block::Water),
            (IVec3::new(0, 1, 0), Block::Stone),
        ]);
        let vertices = water_mesh(&world, chunk_at(IVec3::ZERO), false);

        assert_eq!(vertices.len(), 24);
        assert!(vertices.iter().all(|vertex| vertex.layer() == 9));
    }

    #[test]
    fn water_at_a_chunk_border_suppresses_the_shared_face() {
        let chunk = chunk_at(IVec3::new(31, 5, 0));
        let border_water = world_with(&[
            (IVec3::new(31, 5, 0), Block::Water),
            (IVec3::new(32, 5, 0), Block::Water),
        ]);
        let shared = build_water_mesh(
            &ChunkSnapshot::new(&border_water, chunk),
            &BorderBlocks::new(&border_water, chunk),
            None,
        );
        assert!(
            !shared
                .quads
                .iter()
                .any(|quad| quad.position() == IVec3::new(31, 5, 0) && quad.face() == 0),
            "the +X face of (31, 5, 0) is shared with the neighbour's water"
        );

        let lone = world_with(&[(IVec3::new(31, 5, 0), Block::Water)]);
        let alone = build_water_mesh(
            &ChunkSnapshot::new(&lone, chunk),
            &BorderBlocks::new(&lone, chunk),
            None,
        );
        assert!(
            alone
                .quads
                .iter()
                .any(|quad| quad.position() == IVec3::new(31, 5, 0) && quad.face() == 0),
            "a lone water cell still emits its +X face"
        );
    }

    #[test]
    fn water_meshes_have_no_opaque_faces() {
        let world = world_with(&[(IVec3::ZERO, Block::Water)]);
        let opaque = mesh(&world, chunk_at(IVec3::ZERO), Lod::Lod0);

        assert_eq!(opaque, Vec::new());
    }

    #[test]
    fn water_mesh_is_empty_for_a_dry_chunk_and_populated_for_water() {
        let chunk = chunk_at(IVec3::ZERO);
        let dry_world = world_with(&[(IVec3::ZERO, Block::Stone)]);

        let dry = ChunkSnapshot::new(&dry_world, chunk);
        assert!(!dry.has_water());
        assert!(build_water_mesh(&dry, &BorderBlocks::new(&dry_world, chunk), None).is_empty());

        let missing = ChunkSnapshot::new(&dry_world, chunk.offset(IVec3::X));
        assert!(!missing.has_water());

        let wet_world = world_with(&[(IVec3::ZERO, Block::Water)]);
        let wet = ChunkSnapshot::new(&wet_world, chunk);
        assert!(wet.has_water());
        assert!(!build_water_mesh(&wet, &BorderBlocks::new(&wet_world, chunk), None).is_empty());
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn water_uses_its_own_light_across_the_face() {
        let mut world = World::default();
        world.set_block(IVec3::ZERO, Block::Water);
        world.set_block(IVec3::X, Block::Lamp);
        world.relight();

        let vertices = water_mesh(&world, chunk_at(IVec3::ZERO), true);

        assert_ne!(vertices.len(), 0);
        for vertex in &vertices {
            assert_eq!(vertex.layer(), 9);
            assert_eq!(vertex.light_ao & 0xF, 15, "sky");
            assert_eq!((vertex.light_ao >> 4) & 0xF, 14, "block light");
        }
    }

    #[test]
    fn pack_light_ao_matches_the_spec_bit_ranges() {
        assert_eq!(pack_light_ao(0, 0, 0), 0);
        assert_eq!(pack_light_ao(15, 0, 0), 0xF);
        assert_eq!(pack_light_ao(0, 15, 0), 0xF0);
        assert_eq!(pack_light_ao(0, 0, 3), 0x300);
        assert_eq!(pack_light_ao(0xF, 0xF, 3), 0x3FF);
        assert_eq!(pack_light_ao(0xFF, 0xFF, 0xFF), 0x3FF);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn lod2_scales_positions_and_uvs_by_four() {
        let world = world_with(&[(IVec3::new(4, 4, 4), Block::Stone)]);
        let vertices = mesh(&world, chunk_at(IVec3::new(4, 4, 4)), Lod::Lod2);

        assert_eq!(
            vertices,
            expected(&[
                (0, 1, 1, 1, 1, 1, 4.0, 0),
                (1, 1, 1, 1, 1, 1, 4.0, 0),
                (2, 1, 1, 1, 1, 1, 4.0, 0),
                (3, 1, 1, 1, 1, 1, 4.0, 0),
                (4, 1, 1, 1, 1, 1, 4.0, 0),
                (5, 1, 1, 1, 1, 1, 4.0, 0),
            ])
        );
        assert_eq!(vertices[1].uv(), [0.0, 4.0]);
    }

    #[test]
    fn lod2_downsample_takes_one_cell_per_sixteen_blocks() {
        let world = world_with(&[
            (IVec3::new(0, 0, 0), Block::Stone),
            (IVec3::new(1, 1, 1), Block::Dirt),
        ]);
        let snapshot = ChunkSnapshot::new(&world, chunk_at(IVec3::ZERO));
        let (cells, representatives) =
            downsample(&snapshot, Lod::Lod2.cell_count(), Lod::Lod2.cell_scale());
        let index = index_in(IVec3::ZERO, Lod::Lod2.cell_count());

        assert_eq!(cells[index], Block::Dirt);
        assert_eq!(representatives[index], IVec3::new(1, 1, 1));
        assert_eq!(
            cells[index_in(IVec3::X, Lod::Lod2.cell_count())],
            Block::Air
        );
    }

    #[test]
    fn every_face_winds_counter_clockwise_around_its_normal() {
        use glam::Vec3;

        for (face, normal) in FACE_NORMALS.into_iter().enumerate() {
            let corners = quad_corners(face, 1, 0, 0, 4, 3).map(Vec3::from_array);
            let first = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
            let second = (corners[2] - corners[0]).cross(corners[3] - corners[0]);

            assert!(first.dot(normal.as_vec3()) > 0.0, "face {face}");
            assert!(second.dot(normal.as_vec3()) > 0.0, "face {face}");
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn lod2_emits_boundary_faces_even_next_to_solid_neighbours() {
        let world = world_with(&[
            (IVec3::new(31, 0, 0), Block::Stone),
            (IVec3::new(32, 0, 0), Block::Stone),
        ]);
        let chunk = chunk_at(IVec3::new(31, 0, 0));

        let lod2 = mesh(&world, chunk, Lod::Lod2);

        assert_ne!(lod2.len(), 0);
        assert!(lod2.iter().any(|vertex| vertex.normal() == [1.0, 0.0, 0.0]));
    }

    #[test]
    fn lod_selection_uses_thresholds_and_hysteresis() {
        assert_eq!(select_lod(None, 0.0), Lod::Lod0);
        assert_eq!(select_lod(None, 4.0), Lod::Lod0);
        assert_eq!(select_lod(None, 7.9), Lod::Lod0);
        assert_eq!(select_lod(None, 8.0), Lod::Lod1);
        assert_eq!(select_lod(None, 12.0), Lod::Lod1);

        assert_eq!(select_lod(Some(Lod::Lod0), 4.0), Lod::Lod0);
        assert_eq!(select_lod(Some(Lod::Lod0), 6.0), Lod::Lod0);
        assert_eq!(select_lod(Some(Lod::Lod0), 8.0), Lod::Lod1);

        assert_eq!(select_lod(Some(Lod::Lod1), 8.0), Lod::Lod1);
        assert_eq!(select_lod(Some(Lod::Lod1), 6.0), Lod::Lod1);
        assert_eq!(select_lod(Some(Lod::Lod1), 4.0), Lod::Lod0);

        assert_eq!(select_lod(None, 15.9), Lod::Lod1);
        assert_eq!(select_lod(None, 16.0), Lod::Lod2);
        assert_eq!(select_lod(Some(Lod::Lod0), 20.0), Lod::Lod2);
        assert_eq!(select_lod(Some(Lod::Lod2), 12.0), Lod::Lod2);
        assert_eq!(select_lod(Some(Lod::Lod2), 8.0), Lod::Lod1);
        assert_eq!(select_lod(Some(Lod::Lod2), 4.0), Lod::Lod0);
    }

    #[test]
    fn snapshot_and_border_read_the_world() {
        let world = world_with(&[
            (IVec3::new(0, 0, 0), Block::Stone),
            (IVec3::new(-1, 0, 0), Block::Dirt),
            (IVec3::new(32, 0, 0), Block::Grass),
        ]);
        let chunk = chunk_at(IVec3::ZERO);

        let snapshot = ChunkSnapshot::new(&world, chunk);
        assert_eq!(snapshot.block(IVec3::ZERO), Block::Stone);
        assert_eq!(snapshot.block(IVec3::new(-1, 0, 0)), Block::Air);

        let border = BorderBlocks::new(&world, chunk);
        assert_eq!(border.block(IVec3::new(-1, 0, 0)), Block::Dirt);
        assert_eq!(border.block(IVec3::new(32, 0, 0)), Block::Grass);
        assert_eq!(border.block(IVec3::new(0, 32, 0)), Block::Air);
    }

    #[test]
    fn empty_chunk_builds_no_vertices() {
        let world = World::default();

        assert_eq!(mesh(&world, chunk_at(IVec3::ZERO), Lod::Lod0), Vec::new());
        assert_eq!(mesh(&world, chunk_at(IVec3::ZERO), Lod::Lod1), Vec::new());
    }
}
