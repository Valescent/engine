//! Face collection, merge masks, greedy runs, and light/AO sampling.

use crate::padded::{LOCAL_SIZE, PaddedBlocks, PaddedLight};
use crate::section::{BlockId, Section};

use super::cross::collect_cross;
use super::lod::{MeshLod, coarse_sky};
use super::material::{Material, MaterialProps};
use super::record::{
    NORMAL_NEG_X, NORMAL_NEG_Y, NORMAL_NEG_Z, NORMAL_POS_X, NORMAL_POS_Y, NORMAL_POS_Z, Quad,
    SectionMesh,
};
use super::{PLANE_CELLS, PLANE_WIDTH};

/// A face direction and its two in-plane tangents (`u`, `v`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Face {
    /// `+X`, tangent `u = z`, `v = y`.
    PosX,
    /// `-X`, tangent `u = z`, `v = y`.
    NegX,
    /// `+Y`, tangent `u = x`, `v = z`.
    PosY,
    /// `-Y`, tangent `u = x`, `v = z`.
    NegY,
    /// `+Z`, tangent `u = x`, `v = y`.
    PosZ,
    /// `-Z`, tangent `u = x`, `v = y`.
    NegZ,
}

/// The fixed face order of `M1.4`.
const FACES: [Face; 6] = [
    Face::PosX,
    Face::NegX,
    Face::PosY,
    Face::NegY,
    Face::PosZ,
    Face::NegZ,
];

impl Face {
    /// The record's normal code.
    const fn code(self) -> u8 {
        match self {
            Self::PosX => NORMAL_POS_X,
            Self::NegX => NORMAL_NEG_X,
            Self::PosY => NORMAL_POS_Y,
            Self::NegY => NORMAL_NEG_Y,
            Self::PosZ => NORMAL_POS_Z,
            Self::NegZ => NORMAL_NEG_Z,
        }
    }

    /// Outward normal as a unit step.
    const fn normal(self) -> [i32; 3] {
        match self {
            Self::PosX => [1, 0, 0],
            Self::NegX => [-1, 0, 0],
            Self::PosY => [0, 1, 0],
            Self::NegY => [0, -1, 0],
            Self::PosZ => [0, 0, 1],
            Self::NegZ => [0, 0, -1],
        }
    }

    /// `(normal axis, u axis, v axis)`.
    const fn axes(self) -> (usize, usize, usize) {
        match self {
            Self::PosX | Self::NegX => (0, 2, 1),
            Self::PosY | Self::NegY => (1, 0, 2),
            Self::PosZ | Self::NegZ => (2, 0, 1),
        }
    }
}

/// The merge key of one cell face (`M1.1`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FaceKey {
    /// Block id, so unlike blocks never merge.
    block: BlockId,
    /// Sky nibble of the air-side cell.
    sky: u8,
    /// Block nibble of the air-side cell.
    blk: u8,
    /// Packed four-corner AO. Always `0` for water and coarse levels.
    ao: u8,
}

impl FaceKey {
    /// A water key: no AO (`M2.2`).
    const fn water(block: BlockId, sky: u8, blk: u8) -> Self {
        Self {
            block,
            sky,
            blk,
            ao: 0,
        }
    }
}

/// The local cell on one face plane.
fn cell_at(face: Face, slice: i32, u: i32, v: i32) -> [i32; 3] {
    let (normal_axis, u_axis, v_axis) = face.axes();
    let mut cell = [0; 3];
    cell[normal_axis] = slice;
    cell[u_axis] = u;
    cell[v_axis] = v;
    cell
}

/// Index of an in-plane coordinate in a 32x32 mask.
fn plane_index(u: i32, v: i32) -> usize {
    let u = usize::try_from(u).expect("a plane coordinate fits usize");
    let v = usize::try_from(v).expect("a plane coordinate fits usize");
    u + PLANE_WIDTH * v
}

/// Narrows a signed local coordinate into a record field.
pub(super) fn signed(value: i32) -> u8 {
    u8::try_from(value).expect("a local coordinate fits 5 bits")
}

/// The source of per-face light.
#[derive(Clone, Copy)]
pub(super) enum LightSource<'a> {
    /// Full nibbles from `light2`.
    Nibbles(&'a PaddedLight),
    /// Per-face sky approximation, with no block light (`N3.5`).
    Coarse(u32),
}

/// Greedy-meshes one section into packed records (`W6`).
///
/// `Lod0` reads AO and light nibbles; `Lod1` and `Lod2` skip AO and block light
/// and use the coarse sky rule of
/// [`mesh_coarse`](crate::mesher2::mesh_coarse). The `light` argument is
/// ignored above `Lod0`.
pub(crate) fn mesh(
    section: &Section,
    blocks: &PaddedBlocks,
    light: &PaddedLight,
    props: &MaterialProps,
    lod: MeshLod,
) -> SectionMesh {
    if section.is_empty(0) {
        return SectionMesh::default();
    }
    let source = match lod {
        MeshLod::Lod0 => LightSource::Nibbles(light),
        MeshLod::Lod1 | MeshLod::Lod2 => LightSource::Coarse(lod.cells_per_block()),
    };
    let mut out = SectionMesh::default();
    collect_all(blocks, source, props, &mut out);
    out
}

/// Meshes one section from its full 26-neighbour ownership array (`I2.4`).
///
/// This is the constructor that sees diagonal sections: `neighbours` is indexed
/// by [`crate::padded::NEIGHBOUR_ORDER`] (six faces, twelve edges, eight
/// corners), and a corner AO sample that falls in a diagonal section reads that
/// section's real block instead of defaulting to air. `None` entries keep the
/// [`PaddedBlocks::from_sections`] world-edge mirror rule.
///
/// The six face neighbours are the first six entries; see
/// `mesh_from_face_neighbours` for the older six-neighbour call shape.
pub fn mesh_from_neighbours(
    section: &Section,
    neighbours: &[Option<&Section>; 26],
    light: &PaddedLight,
    props: &MaterialProps,
    lod: MeshLod,
) -> SectionMesh {
    let blocks = PaddedBlocks::from_sections(section, neighbours);
    mesh(section, &blocks, light, props, lod)
}

/// Meshes one section from the legacy six face neighbours only.
///
/// `neighbours` uses the first six entries of
/// [`crate::padded::NEIGHBOUR_ORDER`]: `+X, -X, +Y, -Y, +Z, -Z`. The twenty edge
/// and corner neighbours are filled with explicit air, so corner AO that would
/// look diagonally stays fully open. For any section whose AO samples never
/// leave the six faces this is byte-identical to [`mesh_from_neighbours`] with
/// air diagonals, and to the pre-26-neighbour behaviour. `None` face entries
/// keep the world-edge mirror rule.
pub(crate) fn mesh_from_face_neighbours(
    section: &Section,
    neighbours: &[Option<&Section>; 6],
    light: &PaddedLight,
    props: &MaterialProps,
    lod: MeshLod,
) -> SectionMesh {
    let air = Section::uniform(0);
    let mut owners: [Option<&Section>; 26] = [Some(&air); 26];
    owners[..6].copy_from_slice(neighbours);
    mesh_from_neighbours(section, &owners, light, props, lod)
}

/// Collects all three ranges in the fixed face order.
pub(super) fn collect_all(
    blocks: &PaddedBlocks,
    source: LightSource<'_>,
    props: &MaterialProps,
    out: &mut SectionMesh,
) {
    for face in FACES {
        let mut opaque = Vec::new();
        let mut cutout = Vec::new();
        let mut water = Vec::new();
        collect_face(
            face,
            blocks,
            source,
            props,
            &mut opaque,
            &mut cutout,
            &mut water,
        );
        sort_quads(&mut opaque);
        sort_quads(&mut cutout);
        sort_quads(&mut water);
        out.opaque.append(&mut opaque);
        out.cutout.append(&mut cutout);
        out.water.append(&mut water);
    }
    let mut cross = Vec::new();
    collect_cross(blocks, source, props, &mut cross);
    sort_quads(&mut cross);
    out.cutout.append(&mut cross);
}

/// Collects the merged faces of one direction.
fn collect_face(
    face: Face,
    blocks: &PaddedBlocks,
    source: LightSource<'_>,
    props: &MaterialProps,
    out_opaque: &mut Vec<Quad>,
    out_cutout: &mut Vec<Quad>,
    out_water: &mut Vec<Quad>,
) {
    let normal = face.normal();
    let mut opaque_mask = [None; PLANE_CELLS];
    let mut water_mask = [None; PLANE_CELLS];
    for slice in 0..LOCAL_SIZE {
        for v in 0..LOCAL_SIZE {
            for u in 0..LOCAL_SIZE {
                let cell = cell_at(face, slice, u, v);
                let id = blocks.get(cell[0], cell[1], cell[2]);
                if id == 0 {
                    continue;
                }
                let material = props.material(id);
                if material.cross_quad {
                    continue;
                }
                let neighbour = [
                    cell[0] + normal[0],
                    cell[1] + normal[1],
                    cell[2] + normal[2],
                ];
                let neighbour_material =
                    props.material(blocks.get(neighbour[0], neighbour[1], neighbour[2]));
                let plane = plane_index(u, v);
                if material.water {
                    if is_air(neighbour_material) {
                        let (sky, blk) = face_light(source, blocks, props, neighbour);
                        water_mask[plane] = Some(FaceKey::water(id, sky, blk));
                    }
                    continue;
                }
                if neighbour_material.opaque {
                    continue;
                }
                let (sky, blk) = face_light(source, blocks, props, neighbour);
                let ao = match source {
                    LightSource::Nibbles(_) => face_ao(blocks, props, cell, face),
                    LightSource::Coarse(_) => 0,
                };
                let key = FaceKey {
                    block: id,
                    sky,
                    blk,
                    ao,
                };
                if material.opaque {
                    opaque_mask[plane] = Some(key);
                } else {
                    out_cutout.push(quad_from(face, cell, 1, 1, key, props));
                }
            }
        }
        greedy_emit(face, slice, &mut opaque_mask, props, out_opaque);
        greedy_emit(face, slice, &mut water_mask, props, out_water);
    }
}

/// Greedily merges one mask plane into records.
fn greedy_emit(
    face: Face,
    slice: i32,
    mask: &mut [Option<FaceKey>; PLANE_CELLS],
    props: &MaterialProps,
    out: &mut Vec<Quad>,
) {
    let mut v = 0;
    while v < LOCAL_SIZE {
        let mut u = 0;
        while u < LOCAL_SIZE {
            let Some(key) = mask[plane_index(u, v)] else {
                u += 1;
                continue;
            };
            let mut width = 1;
            while u + width < LOCAL_SIZE && mask[plane_index(u + width, v)] == Some(key) {
                width += 1;
            }
            let mut height = 1;
            'grow: while v + height < LOCAL_SIZE {
                for offset in 0..width {
                    if mask[plane_index(u + offset, v + height)] != Some(key) {
                        break 'grow;
                    }
                }
                height += 1;
            }
            for row in 0..height {
                for column in 0..width {
                    mask[plane_index(u + column, v + row)] = None;
                }
            }
            let cell = cell_at(face, slice, u, v);
            out.push(quad_from(face, cell, width, height, key, props));
            u += width;
        }
        v += 1;
    }
}

/// Builds one record from a merged rectangle.
fn quad_from(
    face: Face,
    cell: [i32; 3],
    width: i32,
    height: i32,
    key: FaceKey,
    props: &MaterialProps,
) -> Quad {
    let material = props.material(key.block);
    Quad::encode(
        signed(cell[0]),
        signed(cell[1]),
        signed(cell[2]),
        signed(width - 1),
        signed(height - 1),
        face.code(),
        key.ao,
        key.sky,
        key.blk,
        material.tile_for(face.code()),
        material.tint,
    )
}

/// Sorts one face group into `(y, z, x)` order, with stable tie-breakers.
fn sort_quads(quads: &mut [Quad]) {
    quads.sort_by_key(|quad| {
        (
            quad.y(),
            quad.z(),
            quad.x(),
            quad.normal(),
            quad.w1(),
            quad.h1(),
        )
    });
}

/// Whether a material is an air cell for water-face purposes.
fn is_air(material: Material) -> bool {
    !material.solid && !material.water
}

/// Reads light for the air-side cell.
pub(super) fn face_light(
    source: LightSource<'_>,
    blocks: &PaddedBlocks,
    props: &MaterialProps,
    at: [i32; 3],
) -> (u8, u8) {
    match source {
        LightSource::Nibbles(light) => light.get(at[0], at[1], at[2]),
        LightSource::Coarse(scale) => (coarse_sky(blocks, props, at, scale), 0),
    }
}

/// Four-corner AO for one face (`L4.1`).
///
/// Corner `0` is `(-u, -v)`, then `(+u, -v)`, `(+u, +v)`, `(-u, +v)`, packed two
/// bits each. Only opaque full cubes occlude.
fn face_ao(blocks: &PaddedBlocks, props: &MaterialProps, cell: [i32; 3], face: Face) -> u8 {
    let normal = face.normal();
    let (_, u_axis, v_axis) = face.axes();
    let mut packed = 0_u8;
    for (corner, (du, dv)) in [(-1, -1), (1, -1), (1, 1), (-1, 1)].into_iter().enumerate() {
        let mut base = [
            cell[0] + normal[0],
            cell[1] + normal[1],
            cell[2] + normal[2],
        ];
        let mut side_u = base;
        let mut side_v = base;
        side_u[u_axis] += du;
        side_v[v_axis] += dv;
        base[u_axis] += du;
        base[v_axis] += dv;
        let occludes_u = occludes(blocks, props, side_u);
        let occludes_v = occludes(blocks, props, side_v);
        let occludes_corner = occludes(blocks, props, base);
        let value = if occludes_u && occludes_v {
            0
        } else {
            3_u8.saturating_sub(u8::from(occludes_u))
                .saturating_sub(u8::from(occludes_v))
                .saturating_sub(u8::from(occludes_corner))
        };
        packed |= value << (2 * corner);
    }
    packed
}

/// Whether a cell holds an opaque full cube.
fn occludes(blocks: &PaddedBlocks, props: &MaterialProps, cell: [i32; 3]) -> bool {
    props.material(blocks.get(cell[0], cell[1], cell[2])).opaque
}
