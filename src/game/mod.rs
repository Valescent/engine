//! Gameplay state that does not need a window or a GPU.
//!
//! The modules here own items, inventories, tools, drops, entities, the fixed
//! tick, and the player save. `app` translates input into these calls; the
//! renderer only reads the resulting entity list.

mod crafting;
mod drops;
mod entities;
mod environment;
mod inventory;
mod item;
mod recipe;
mod save;
mod spawn;
mod state;
mod survival;
mod tools;

pub(crate) use crafting::{
    CraftError, CraftGrid, craft_all_from_grid, craft_direct_all, craft_once_from_grid,
    salvage_held,
};
pub(crate) use drops::drops_for;
pub(crate) use environment::EnvironmentProbe;
pub(crate) use inventory::{HOTBAR_SLOTS, Inventory, OFFHAND_SLOT, TOTAL_SLOTS};
pub(crate) use item::{ItemKind, ItemRegistry, ItemStack};
pub(crate) use recipe::{CompiledRecipe, RecipeBook, Station, display_name};
pub(crate) use save::quarantine;
pub(crate) use spawn::find_safe_spawn;
pub(crate) use state::GameState;
pub(crate) use survival::fall_damage;
pub(crate) use tools::break_seconds;
