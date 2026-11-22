//! Gameplay state that does not need a window or a GPU.
//!
//! The modules here own items, inventories, tools, drops, entities, the fixed
//! tick, and the player save. `app` translates input into these calls; the
//! renderer only reads the resulting entity list.

mod drops;
mod entities;
mod inventory;
mod item;
mod save;
mod state;
mod tools;

pub(crate) use drops::drops_for;
pub(crate) use item::{ItemKind, ItemRegistry};
pub(crate) use save::quarantine;
pub(crate) use state::GameState;
pub(crate) use tools::break_seconds;
