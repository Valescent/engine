//! Material light rules: the level range, opacity, sky attenuation, and
//! the cost of entering a cell.

use crate::section::BlockId;

use super::bfs::Channel;

/// Highest light level (`L1.1`).
pub const LIGHT_LEVELS: u8 = 15;

/// Skylight lost per water block (`L2.4`).
pub const WATER_SKY_ATTEN: u8 = 1;

/// Block id of air, by registry convention.
pub const AIR_ID: BlockId = 0;

/// Block id of leaves in the current registry.
pub const LEAVES_ID: BlockId = 5;

/// Block id of the lamp in the current registry.
pub const LAMP_ID: BlockId = 6;

/// Block id of water in the current registry.
pub const WATER_ID: BlockId = 7;

/// Light emitted by a lamp (`L2.1` is still open, so this is the provisional one).
pub const LAMP_EMISSION: u8 = 14;

/// Provisional id range for cross-quad plants, which never occlude (`H4.2`).
///
/// `A5.1` has not pinned a block registry with a `render_shape` field, so the
/// default table cannot identify plants. This range is a placeholder: replace
/// it with the manifest lookup when the registry lands.
pub const CROSS_QUAD_IDS: core::ops::RangeInclusive<BlockId> = 64..=95;

/// Block returned for `y < 0`: bedrock is opaque, so stone's id stands in.
pub const BEDROCK_ID: BlockId = 1;

/// Per-material light behaviour (`L1.1`, `L2.4`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LightProps {
    /// Skylight lost on entering the block.
    pub sky_atten: u8,
    /// Extra block-light loss on entering the block.
    pub block_opacity: u8,
}

impl LightProps {
    /// Fully transparent: air, cross-quads.
    pub const CLEAR: Self = Self {
        sky_atten: 0,
        block_opacity: 0,
    };

    /// Blocks both channels.
    pub const OPAQUE: Self = Self {
        sky_atten: LIGHT_LEVELS,
        block_opacity: LIGHT_LEVELS,
    };

    /// Attenuates skylight by [`WATER_SKY_ATTEN`] and passes block light.
    pub const WATER: Self = Self {
        sky_atten: WATER_SKY_ATTEN,
        block_opacity: 0,
    };

    /// Passes skylight and dims block light.
    pub const LEAVES: Self = Self {
        sky_atten: 0,
        block_opacity: 1,
    };

    /// Builds a rule, masking both fields to four bits.
    #[must_use]
    pub const fn new(sky_atten: u8, block_opacity: u8) -> Self {
        Self {
            sky_atten: sky_atten & 0x0F,
            block_opacity: block_opacity & 0x0F,
        }
    }
}

/// A full block-id → [`LightProps`] table.
///
/// A plain array, never a map, so a lookup in a BFS step is an index. Ids at or
/// above 256 use the fallback. The fallback defaults to [`LightProps::OPAQUE`],
/// so an unknown material can only ever under-light, never leak.
#[derive(Clone, Debug)]
pub struct LightTable {
    props: [LightProps; 256],
    emission: [u8; 256],
    fallback: LightProps,
}

impl Default for LightTable {
    fn default() -> Self {
        default_table()
    }
}

/// The v1 default material table (`L1.1`, `L2.4`).
///
/// Air and the provisional cross-quad range are `0/0`, leaves are `0/1`,
/// water is `1/0`, and everything else is opaque `15/15`. A lamp is opaque to
/// skylight, passes block light, and emits [`LAMP_EMISSION`].
#[must_use]
pub fn default_table() -> LightTable {
    let mut table = LightTable::new(LightProps::OPAQUE);
    table.set(AIR_ID, LightProps::CLEAR);
    table.set(LEAVES_ID, LightProps::LEAVES);
    table.set(WATER_ID, LightProps::WATER);
    table.set(LAMP_ID, LightProps::new(LIGHT_LEVELS, 0));
    table.set_emission(LAMP_ID, LAMP_EMISSION);
    for id in CROSS_QUAD_IDS {
        table.set(id, LightProps::CLEAR);
    }
    table
}

impl LightTable {
    /// A table where every id maps to `fallback`.
    #[must_use]
    pub fn new(fallback: LightProps) -> Self {
        Self {
            props: [fallback; 256],
            emission: [0; 256],
            fallback,
        }
    }

    /// Sets the rule for one id. Ids at or above 256 are not table entries.
    pub fn set(&mut self, id: BlockId, props: LightProps) {
        if let Some(slot) = self.props.get_mut(usize::from(id)) {
            *slot = props;
        }
    }

    /// Sets how much light one id emits. Ids at or above 256 never emit.
    pub fn set_emission(&mut self, id: BlockId, level: u8) {
        if let Some(slot) = self.emission.get_mut(usize::from(id)) {
            *slot = level & 0x0F;
        }
    }

    /// The rule for one id.
    #[must_use]
    pub fn props(&self, id: BlockId) -> LightProps {
        self.props
            .get(usize::from(id))
            .copied()
            .unwrap_or(self.fallback)
    }

    /// The emission for one id.
    #[must_use]
    pub fn emission(&self, id: BlockId) -> u8 {
        self.emission.get(usize::from(id)).copied().unwrap_or(0)
    }

    /// The rule for ids at or above the table size.
    #[must_use]
    pub fn fallback(&self) -> LightProps {
        self.fallback
    }
}

/// Material bits: four bits each for sky attenuation, block opacity, emission.
pub(super) fn packed_props(table: &LightTable, id: BlockId) -> u16 {
    let props = table.props(id);
    u16::from(props.sky_atten & 0x0F)
        | (u16::from(props.block_opacity & 0x0F) << 4)
        | (u16::from(table.emission(id) & 0x0F) << 8)
}

pub(super) fn sky_atten(cell: u16) -> u8 {
    u8::try_from(cell & 0x0F).expect("a nibble fits u8")
}

pub(super) fn block_opacity(cell: u16) -> u8 {
    u8::try_from((cell >> 4) & 0x0F).expect("a nibble fits u8")
}

pub(super) fn emission(cell: u16) -> u8 {
    u8::try_from((cell >> 8) & 0x0F).expect("a nibble fits u8")
}

/// Loss on entering a cell laterally.
fn lateral_cost(channel: Channel, cell: u16) -> u8 {
    let cost = match channel {
        Channel::Sky => sky_atten(cell),
        Channel::Block => block_opacity(cell),
    };
    cost.max(1)
}

/// Loss on entering a cell from the neighbour `dy` away.
///
/// Sky light straight down loses only the destination's attenuation, which is
/// zero for air: the vertical rule applies to light that arrived laterally too,
/// not just to the pass-1 seed, so air keeps its lossless descent.
///
/// Sky light is never let into an attenuating cell sideways. Water takes its sky
/// from the column above, so `WATER_SKY_ATTEN` accumulates per block of depth
/// instead of every cell being refreshed to the surface value by the open air
/// beside it (`L2.4`). Opaque cells already cost the whole range laterally, so
/// only partially attenuating materials — water in the default table — change.
pub(super) fn step_cost(channel: Channel, cell: u16, dy: i32) -> u8 {
    match channel {
        Channel::Sky if dy < 0 => sky_atten(cell),
        Channel::Sky if dy == 0 && sky_atten(cell) > 0 => LIGHT_LEVELS,
        _ => lateral_cost(channel, cell),
    }
}
