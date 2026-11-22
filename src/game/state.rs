//! Fixed-tick gameplay state: inventory, break progress, drops, and persistence.

use std::path::Path;

use glam::{IVec3, Vec3};
use tracing::debug;

use super::entities::EntityStore;
use super::inventory::Inventory;
use super::item::ItemRegistry;
use super::save::{PlayerSave, PlayerSaveError};
use super::tools::Tool;
use crate::player::BlockSource;

/// Fixed simulation step.
pub(crate) const TICK_SECONDS: f32 = 1.0 / 60.0;
/// Longest frame delta the accumulator accepts.
const MAX_ACCUMULATED: f32 = 0.25;
/// Radius at which dropped items are collected.
pub(crate) const PICKUP_RADIUS: f32 = 1.5;

/// A break in progress.
#[derive(Clone, Copy, Debug)]
struct BreakTarget {
    cell: IVec3,
    total: f32,
}

/// Everything the player owns and the world simulates between saves.
#[derive(Clone, Debug, Default)]
pub(crate) struct GameState {
    pub(crate) inventory: Inventory,
    pub(crate) entities: EntityStore,
    break_target: Option<BreakTarget>,
    break_elapsed: f32,
    completed_break: Option<IVec3>,
    accumulator: f32,
}

/// Where a loaded save puts the player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LoadedGame {
    pub(crate) position: Vec3,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
}

impl GameState {
    /// Creates a new run with the temporary starting kit.
    ///
    /// The wooden pickaxe exists only until crafting lands in `0.0.10.3`; it
    /// proves the minimum-tier rule while stone is otherwise unobtainable.
    pub(crate) fn new(registry: &ItemRegistry) -> Self {
        let mut state = Self::default();
        if let Some(pickaxe) = registry.stack("core:wooden_pickaxe", 1) {
            let _ = state.inventory.add(pickaxe, registry);
        }
        state
    }

    /// Sets the block the player is currently holding the break button on.
    ///
    /// Progress resets when the target changes. `held = false` cancels it.
    pub(crate) fn set_break_target(&mut self, cell: IVec3, total: f32, held: bool) {
        if !held {
            self.break_target = None;
            self.break_elapsed = 0.0;
            return;
        }
        match self.break_target {
            Some(target) if target.cell == cell => {
                self.break_target = Some(BreakTarget {
                    total: total.max(TICK_SECONDS),
                    ..target
                });
            }
            _ => {
                self.break_target = Some(BreakTarget {
                    cell,
                    total: total.max(TICK_SECONDS),
                });
                self.break_elapsed = 0.0;
            }
        }
    }

    /// Cancels the current break.
    pub(crate) fn cancel_break(&mut self) {
        self.break_target = None;
        self.break_elapsed = 0.0;
    }

    /// The current break's cell and completion fraction, for the highlight.
    pub(crate) fn break_fraction(&self) -> Option<(IVec3, f32)> {
        let target = self.break_target?;
        Some((
            target.cell,
            (self.break_elapsed / target.total).clamp(0.0, 1.0),
        ))
    }

    /// Takes the block that finished breaking this frame, if any.
    pub(crate) fn take_completed_break(&mut self) -> Option<IVec3> {
        self.completed_break.take()
    }

    /// Advances entity physics and break progress on the fixed tick.
    pub(crate) fn tick(&mut self, delta: f32, world: &dyn BlockSource) {
        if delta <= 0.0 {
            return;
        }
        self.accumulator = (self.accumulator + delta).min(MAX_ACCUMULATED);
        while self.accumulator >= TICK_SECONDS {
            self.accumulator -= TICK_SECONDS;
            self.entities.tick(world, TICK_SECONDS);
            if let Some(target) = self.break_target {
                self.break_elapsed += TICK_SECONDS;
                if self.break_elapsed >= target.total {
                    self.completed_break = Some(target.cell);
                    self.cancel_break();
                }
            }
        }
    }

    /// Collects nearby drops into the inventory.
    pub(crate) fn pickup(&mut self, position: Vec3, registry: &ItemRegistry) -> u16 {
        self.entities
            .pickup(position, PICKUP_RADIUS, &mut self.inventory, registry)
    }

    /// The held tool's class and tier, if the selected item is a tool.
    pub(crate) fn selected_tool(&self, registry: &ItemRegistry) -> Option<Tool> {
        self.inventory
            .selected()
            .and_then(|stack| registry.tool(stack.item))
    }

    /// Applies one durability point to the selected item. Returns `true` when
    /// the tool broke.
    pub(crate) fn damage_selected(&mut self) -> bool {
        let index = self.inventory.selected_index();
        self.inventory.damage(index, 1)
    }

    /// Writes the player and entity state.
    pub(crate) fn save(
        &self,
        path: &Path,
        seed: u64,
        position: Vec3,
        yaw: f32,
        pitch: f32,
        registry: &ItemRegistry,
    ) -> Result<(), PlayerSaveError> {
        let save = PlayerSave {
            seed,
            position,
            yaw,
            pitch,
            inventory: self.inventory.clone(),
            entities: self.entities.records(registry),
        };
        save.write(path, registry)
    }

    /// Restores a save from another run of the same seed.
    ///
    /// Returns `None` for a missing file, a seed mismatch, or a corrupt file
    /// that has already been quarantined by the caller.
    pub(crate) fn load(
        &mut self,
        path: &Path,
        seed: u64,
        registry: &ItemRegistry,
    ) -> Result<Option<LoadedGame>, PlayerSaveError> {
        let Some(save) = PlayerSave::load(path, registry)? else {
            return Ok(None);
        };
        if save.seed != seed {
            debug!(
                saved_seed = save.seed,
                seed, "player save belongs to another world; ignoring it"
            );
            return Ok(None);
        }
        self.inventory = save.inventory;
        self.entities = EntityStore::from_records(save.entities, registry);
        Ok(Some(LoadedGame {
            position: save.position,
            yaw: save.yaw,
            pitch: save.pitch,
        }))
    }
}

#[cfg(test)]
mod tests {
    use crate::content;
    use crate::world::World;

    use super::super::item::ItemKind;
    use super::*;

    fn registry() -> ItemRegistry {
        ItemRegistry::builtin(&content::v1_default())
    }

    #[test]
    fn a_new_run_starts_with_the_temporary_pickaxe() {
        let registry = registry();
        let state = GameState::new(&registry);

        assert_eq!(
            registry.name(state.inventory.selected().unwrap().item),
            "core:wooden_pickaxe"
        );
    }

    #[test]
    fn a_break_completes_on_the_fixed_tick() {
        let mut state = GameState::new(&registry());
        let cell = IVec3::new(1, 2, 3);
        state.set_break_target(cell, 0.1, true);

        for _ in 0..7 {
            state.tick(1.0 / 60.0, &World::default());
        }

        assert_eq!(state.take_completed_break(), Some(cell));
        assert!(state.break_fraction().is_none());
    }

    #[test]
    fn releasing_or_changing_the_target_cancels_progress() {
        let mut state = GameState::new(&registry());
        state.set_break_target(IVec3::new(1, 2, 3), 1.0, true);
        state.tick(1.0 / 60.0, &World::default());
        let progress = state.break_fraction().unwrap().1;
        assert!(progress > 0.0);

        state.set_break_target(IVec3::ZERO, 1.0, true);
        assert_eq!(state.break_fraction().unwrap().1, 0.0);

        state.set_break_target(IVec3::ZERO, 1.0, false);
        assert!(state.break_fraction().is_none());
    }

    #[test]
    fn save_and_load_restore_the_inventory_and_drops() {
        let registry = registry();
        let directory = std::env::temp_dir().join(format!("valescent-game-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("player.dat");

        let mut state = GameState::new(&registry);
        state
            .inventory
            .add(registry.stack("core:stone", 9).unwrap(), &registry);
        state.entities.spawn(
            registry.stack("core:dirt", 2).unwrap(),
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::ZERO,
            &registry,
        );
        state
            .save(&path, 42, Vec3::new(4.0, 5.0, 6.0), 0.5, -0.25, &registry)
            .unwrap();

        let mut loaded = GameState::default();
        let player = loaded.load(&path, 42, &registry).unwrap().unwrap();

        assert_eq!(player.position, Vec3::new(4.0, 5.0, 6.0));
        assert_eq!(player.yaw, 0.5);
        assert_eq!(
            loaded.inventory.count(registry.id("core:stone").unwrap()),
            9
        );
        assert_eq!(loaded.entities.len(), 1);

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_seed_mismatch_is_ignored() {
        let registry = registry();
        let directory =
            std::env::temp_dir().join(format!("valescent-game-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("player.dat");
        GameState::new(&registry)
            .save(&path, 1, Vec3::ZERO, 0.0, 0.0, &registry)
            .unwrap();

        let mut loaded = GameState::default();
        assert!(loaded.load(&path, 2, &registry).unwrap().is_none());

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn gather_place_save_and_reopen() {
        let registry = registry();
        let pack = content::v1_default();
        let directory =
            std::env::temp_dir().join(format!("valescent-game-loop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("player.dat");

        let mut state = GameState::new(&registry);
        let tool = state.selected_tool(&registry);
        let stone = crate::game::drops_for(&pack, &registry, "core:stone", tool)
            .pop()
            .expect("a wooden pickaxe drops stone");
        state
            .entities
            .spawn(stone, Vec3::new(0.5, 1.0, 0.5), Vec3::ZERO, &registry);
        let picked = state.pickup(Vec3::new(0.5, 1.0, 0.5), &registry);
        assert_eq!(picked, 1);

        let stone_id = registry.id("core:stone").unwrap();
        let stone_slot = (0..super::super::inventory::HOTBAR_SLOTS)
            .find(|index| {
                state
                    .inventory
                    .slot(*index)
                    .is_some_and(|stack| stack.item == stone_id)
            })
            .expect("the stone landed in the hotbar");
        state.inventory.select(stone_slot);
        assert!(matches!(
            registry.def(stone_id).kind,
            ItemKind::Block { .. }
        ));
        let placed = state.inventory.remove(stone_slot, 1).unwrap();
        assert_eq!(placed.count, 1);
        assert_eq!(state.inventory.count(stone_id), 0);

        state
            .save(&path, 7, Vec3::new(2.0, 3.0, 4.0), 0.25, -0.5, &registry)
            .unwrap();
        let mut reopened = GameState::default();
        let loaded = reopened.load(&path, 7, &registry).unwrap().unwrap();

        assert_eq!(loaded.position, Vec3::new(2.0, 3.0, 4.0));
        assert_eq!(loaded.yaw, 0.25);
        assert_eq!(loaded.pitch, -0.5);
        assert_eq!(
            reopened
                .inventory
                .count(registry.id("core:wooden_pickaxe").unwrap()),
            1
        );

        let _ = std::fs::remove_dir_all(directory);
    }
}
