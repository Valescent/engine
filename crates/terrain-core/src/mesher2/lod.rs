//! Mesh detail levels and the coarse mesh path with no stored light.

use crate::padded::{LOCAL_MAX, NIBBLE_MAX, PaddedBlocks};

use super::greedy::{LightSource, collect_all};
use super::material::MaterialProps;
use super::record::SectionMesh;

pub(crate) const SHADE_PER_CELL: u8 = 1;

/// Extra sky-light loss per water block above a coarse face (`L2.4`, `N3.5`).
pub const WATER_SKY_ATTEN: u8 = 1;

/// Mesh detail level (`N1.1`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum MeshLod {
    /// One block per cell: full AO and light.
    Lod0,
    /// Two blocks per cell.
    Lod1,
    /// Four blocks per cell.
    Lod2,
}

impl MeshLod {
    /// World blocks covered by one coarse cell.
    pub const fn cells_per_block(self) -> u32 {
        match self {
            Self::Lod0 => 1,
            Self::Lod1 => 2,
            Self::Lod2 => 4,
        }
    }
}

/// Coarse mesh with no stored light (`N3.5`).
///
/// AO and block light are zero. Sky per face is `15` when the air-side cell is
/// open to the ceiling; otherwise `15 - SHADE_PER_CELL * depth` where `depth` is
/// the distance in world blocks to the first opaque block above, then minus
/// `WATER_SKY_ATTEN` per water block above, clamped at zero. `cells_per_block`
/// scales a coarse cell to world blocks; `0` means "use `lod`'s default".
pub fn mesh_coarse(
    blocks: &PaddedBlocks,
    props: &MaterialProps,
    lod: MeshLod,
    cells_per_block: u32,
) -> SectionMesh {
    let scale = if cells_per_block == 0 {
        lod.cells_per_block()
    } else {
        cells_per_block
    };
    let mut out = SectionMesh::default();
    collect_all(blocks, LightSource::Coarse(scale), props, &mut out);
    out
}

/// Coarse per-face sky from the open-ceiling rule (`N3.5`).
pub(super) fn coarse_sky(
    blocks: &PaddedBlocks,
    props: &MaterialProps,
    at: [i32; 3],
    cells_per_block: u32,
) -> u8 {
    let mut water = 0_u32;
    let mut opaque_depth = None;
    let mut y = at[1] + 1;
    while y <= LOCAL_MAX {
        let material = props.material(blocks.get(at[0], y, at[2]));
        if material.opaque {
            opaque_depth = Some(y - at[1]);
            break;
        }
        if material.water {
            water += 1;
        }
        y += 1;
    }
    let depth = opaque_depth.map_or(0, |depth| {
        let depth = u32::try_from(depth).expect("a depth fits u32");
        depth
            .saturating_mul(cells_per_block)
            .saturating_mul(u32::from(SHADE_PER_CELL))
    });
    let water = water
        .saturating_mul(cells_per_block)
        .saturating_mul(u32::from(WATER_SKY_ATTEN));
    let sky = u32::from(NIBBLE_MAX)
        .saturating_sub(depth)
        .saturating_sub(water);
    u8::try_from(sky).expect("a clamped nibble fits u8")
}
