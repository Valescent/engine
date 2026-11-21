//! What a feature may write: the footprint, column field, palette, and masks
//! (`H2.3`, `H3.4`).

use crate::section::BlockId;

/// Columns on one edge of the gathered footprint: `3 * 32`.
pub const FOOTPRINT: i32 = 96;

/// Columns in the gathered footprint.
pub const COLUMNS: usize = 9_216;

const _: () = assert!(FOOTPRINT == 96 && COLUMNS == 9_216);

/// What a feature is allowed to replace (`H2.3`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Mask {
    /// Only blocks in the caller-supplied host set (ores, boulders).
    ReplaceHost,
    /// Only air (leaves, plants).
    AirOnly,
    /// Air or an existing leaf (logs).
    AirOrLeaf,
    /// Any solid block that is not water or bedrock (snow layers).
    ReplaceSurface,
}

/// Whether a mask replaces solid rock, as opposed to filling air.
pub(super) const fn is_solid_mask(mask: Mask) -> bool {
    matches!(mask, Mask::ReplaceHost | Mask::ReplaceSurface)
}

/// Whether a mask only fills air.
pub(super) const fn is_air_mask(mask: Mask) -> bool {
    matches!(mask, Mask::AirOnly | Mask::AirOrLeaf)
}

/// The block ids application needs by name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Palette {
    /// Id of air.
    pub air: BlockId,
    /// Id of water. Never written.
    pub water: BlockId,
    /// Id of bedrock. Never written.
    pub bedrock: BlockId,
    /// Id of a tree trunk or branch.
    pub log: BlockId,
    /// Id of a tree leaf.
    pub leaf: BlockId,
    /// Id of a boulder.
    pub boulder: BlockId,
}

/// The block id for one ore spec, keyed by the spec's stable name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OreBlock {
    /// Stable name of the ore spec it belongs to.
    pub name: &'static str,
    /// Id written where the vein replaces host rock.
    pub id: BlockId,
}

/// Column flags the tree filter reads (`H3.4`).
pub const COLUMN_RIVER: u8 = 1;

/// The column is inside a bank band around a river or lake (`H3.4`).
pub const COLUMN_BANK: u8 = 2;

/// Per-column 2D data for the gathered footprint.
///
/// Index `0` is the lowest `(x, z)` of the footprint. Every array is
/// [`COLUMNS`] long and laid out `x` fastest, `z` slowest.
#[derive(Clone, Copy, Debug)]
pub struct ColumnField<'a> {
    /// World `(x, z)` of the first column.
    pub origin: [i32; 2],
    /// Top solid block per column (`HeightTile` before detail and caves).
    pub tops: &'a [i32],
    /// Largest one-block step to a neighbour (`HeightTile::slope_drop`).
    pub slope: &'a [i32],
    /// Biome id per column.
    pub biome: &'a [u8],
    /// [`COLUMN_RIVER`] / [`COLUMN_BANK`] bits per column.
    pub flags: &'a [u8],
}

impl ColumnField<'_> {
    /// Slot of a world column, or `None` outside the footprint.
    fn index(&self, x: i32, z: i32) -> Option<usize> {
        let local_x = x - self.origin[0];
        let local_z = z - self.origin[1];
        if !(0..FOOTPRINT).contains(&local_x) || !(0..FOOTPRINT).contains(&local_z) {
            return None;
        }
        let index = local_x + FOOTPRINT * local_z;
        usize::try_from(index).ok()
    }

    /// Top solid block of one column.
    #[must_use]
    pub fn top(&self, x: i32, z: i32) -> Option<i32> {
        self.index(x, z)
            .and_then(|index| self.tops.get(index))
            .copied()
    }

    /// Slope drop of one column.
    #[must_use]
    pub fn slope(&self, x: i32, z: i32) -> Option<i32> {
        self.index(x, z)
            .and_then(|index| self.slope.get(index))
            .copied()
    }

    /// Biome id of one column.
    #[must_use]
    pub fn biome(&self, x: i32, z: i32) -> Option<u8> {
        self.index(x, z)
            .and_then(|index| self.biome.get(index))
            .copied()
    }

    /// Flags of one column.
    #[must_use]
    pub fn flags(&self, x: i32, z: i32) -> Option<u8> {
        self.index(x, z)
            .and_then(|index| self.flags.get(index))
            .copied()
    }
}

/// [`mask_allows`] with the context reduced to the facts it reads.
pub(super) fn mask_accepts(
    mask: Mask,
    current: BlockId,
    palette: Palette,
    host_rock: &[BlockId],
) -> bool {
    if current == palette.water || current == palette.bedrock {
        return false;
    }
    match mask {
        Mask::ReplaceHost => current != palette.air && host_rock.contains(&current),
        Mask::AirOnly => current == palette.air,
        Mask::AirOrLeaf => current == palette.air || current == palette.leaf,
        Mask::ReplaceSurface => current != palette.air,
    }
}
