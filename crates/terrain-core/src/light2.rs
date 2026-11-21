//! `W7` — per-column sky and block light with incremental repair.
//!
//! `L1.1`, `L1.2`, `L2.2`–`L2.4`, `L3.1`–`L3.3`. One job lights a whole
//! 12-section column: twelve 32³ sections stacked to `WORLD_HEIGHT`, plus a
//! one-block border borrowed from the 8 horizontal neighbours. There is no sky
//! summary, no cross-job dependency, and no persistence: light is a pure
//! function of blocks and is recomputed on load.
//!
//! # Storage
//!
//! Light is 15 levels, two channels (sky, block), one nibble per cell.
//! [`ColumnLight`] holds one [`LightSlot`] per section per channel. A slot is
//! either [`LightSlot::Uniform`] (one byte, no allocation — all air is sky 15 /
//! block 0, all solid is sky 0) or [`LightSlot::Packed`] (a [`NibbleArray`] of
//! 2048 words). Promotion and demotion happen on write, so a section that
//! becomes uniform again releases its array.
//!
//! # Propagation
//!
//! Pass 1 seeds the sky vertically from the ceiling. Entering a cell costs its
//! material's `sky_atten`, so open air loses nothing straight down while water
//! loses `WATER_SKY_ATTEN` on every block of the descent. Pass 2 is a lateral BFS
//! seeded only at cells that can improve a neighbour, costing `max(1, sky_atten)`
//! per lateral step and the same zero-cost-down rule when it moves straight down,
//! so light that arrived sideways still runs down a shaft without loss. Sky light
//! never enters an attenuating cell sideways: water takes its sky from the column
//! above, so an open air cell beside a channel cannot flatten the depth charge to
//! the surface level. Block light is a plain BFS from emitters costing
//! `max(1, block_opacity)` per step; water is free for block light, which is what
//! `L2.4` requires.
//!
//! Every loop has a fixed bound. The BFS queue is a fixed-capacity ring buffer
//! ([`QUEUE_CAPACITY`]); if a pathological field ever fills it, the remainder is
//! relaxed by a bounded sweep that reaches the same fixed point.
//!
//! # Repair
//!
//! [`ColumnLight::relight_column`] is two-phase: the writable region is cleared,
//! then re-lit from sky seeds, emitters, and the unchanged cells around it.
//! Every cell is a maximum over sources, so the result is the unique fixed point
//! and cannot depend on order. The writable region is the whole border ring for
//! every `y`, and the interior from the floor up to 15 blocks above the highest
//! touched section. The floor matters because the vertical sky seed is lossless:
//! a block placed at `y` darkens every cell below it, while lateral light cannot
//! reach more than 15 blocks. Everything above the region is unchanged. A fuzz
//! test pins bit-equality with a fresh column.
//!
//! The border ring is recomputed rather than stored. `ColumnLight` keeps only
//! the 12 owned sections, so a bounded repair has to re-derive the ring from
//! blocks; storing it would let the repair touch just the edited halo.
//!
//! # Modules
//!
//! `table` holds the material rules, `column` the light slots, block access,
//! and the full build, `bfs` the propagation scratch, and `repair` the
//! incremental span.

mod bfs;
mod column;
mod repair;
mod table;
#[cfg(test)]
mod tests;

pub use bfs::QUEUE_CAPACITY;
pub use column::{
    COLUMN_HEIGHT, COLUMN_SECTIONS, ColumnBlocks, ColumnLight, LightSlot, LightStats, NIBBLE_WORDS,
    NibbleArray, SECTION_CELLS, compute_column, compute_column_stats,
};
pub use repair::{ColumnView, Edit, RELIGHT_RADIUS};
pub use table::{
    AIR_ID, BEDROCK_ID, CROSS_QUAD_IDS, LAMP_EMISSION, LAMP_ID, LEAVES_ID, LIGHT_LEVELS,
    LightProps, LightTable, WATER_ID, WATER_SKY_ATTEN, default_table,
};
