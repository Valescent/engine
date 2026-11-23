//! World queries for survival damage and respawn search.
//!
//! Implementations live in the app layer so the headless game state never
//! depends on the demo or legacy world types.

use glam::IVec3;

/// The block queries survival needs.
pub(crate) trait EnvironmentProbe {
    /// Whether a cell is solid for collision and support.
    fn is_solid(&self, cell: IVec3) -> bool;
    /// Whether a cell is water.
    fn is_water(&self, cell: IVec3) -> bool;
    /// Whether a cell is lava. The placeholder pack has no lava yet.
    fn is_lava(&self, _cell: IVec3) -> bool {
        false
    }
    /// Whether a cell is fire. The placeholder pack has no fire yet.
    fn is_fire(&self, _cell: IVec3) -> bool {
        false
    }
}
