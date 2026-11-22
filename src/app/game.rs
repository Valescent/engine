//! App-side gameplay glue: targets, break progress, drops, placement, and the
//! player save.
//!
//! The fixed-tick state lives in `crate::game`; this module is the only place
//! that knows which world is live and how to turn a raycast into an edit.

use std::path::{Path, PathBuf};

use glam::{IVec3, Vec3};
use tracing::{debug, info, warn};

use crate::game::{ItemKind, break_seconds, drops_for, quarantine};
use crate::player::BlockSource;
use crate::world::WORLD_DIRECTORY;

use super::App;
use super::input::place_position;

const PLAYER_SAVE: &str = "player.dat";

impl App {
    /// The player save path for the live world.
    pub(super) fn game_save_path(&self) -> PathBuf {
        if self.demo.is_some() {
            Path::new(crate::demo::TERRAIN_SAVE_DIR).join(PLAYER_SAVE)
        } else {
            Path::new(WORLD_DIRECTORY).join(PLAYER_SAVE)
        }
    }

    /// Restores the player save when it belongs to this seed.
    pub(crate) fn load_game(&mut self) {
        if self.stress {
            return;
        }
        let path = self.game_save_path();
        let seed = self.seed.unwrap_or(0);
        match self.game.load(&path, seed, &self.items) {
            Ok(Some(loaded)) => {
                self.player.position = loaded.position;
                self.camera.set_position(self.player.eye_position());
                self.camera.set_yaw(loaded.yaw);
                self.camera.set_pitch(loaded.pitch);
                self.has_player_save = true;
                info!(?path, "player restored");
            }
            Ok(None) => {}
            Err(error) => {
                warn!(%error, ?path, "unreadable player save; starting fresh");
                if let Err(error) = quarantine(&path) {
                    warn!(%error, "failed to quarantine the player save");
                }
            }
        }
    }

    /// Writes the player and dropped-item state.
    pub(super) fn save_game(&mut self) {
        if self.stress {
            return;
        }
        let path = self.game_save_path();
        let seed = self.seed.unwrap_or(0);
        match self.game.save(
            &path,
            seed,
            self.player.position,
            self.camera.yaw(),
            self.camera.pitch(),
            &self.items,
        ) {
            Ok(()) => debug!(?path, "player saved"),
            Err(error) => warn!(%error, "failed to save the player"),
        }
    }

    /// Starts a break. The legacy world keeps its instant click path.
    pub(super) fn begin_break(&mut self) {
        if self.demo.is_none() {
            self.break_block();
            return;
        }
        self.break_held = true;
    }

    /// Cancels a break when the button is released or input is lost.
    pub(super) fn end_break(&mut self) {
        self.break_held = false;
        self.game.cancel_break();
    }

    /// Advances the fixed tick and applies the block that finished breaking.
    pub(super) fn tick_game(&mut self, delta: f32) {
        if self.demo.is_some() {
            self.step_break_target();
        } else {
            self.game.set_break_target(IVec3::ZERO, 0.0, false);
        }
        let source: &dyn BlockSource = match &self.demo {
            Some(demo) => demo,
            None => &self.world,
        };
        self.game.tick(delta, source);
        if let Some(cell) = self.game.take_completed_break() {
            self.finish_break(cell);
        }
        let picked = self.game.pickup(self.player.position, &self.items);
        if picked > 0 {
            debug!(picked, "picked up items");
        }
    }

    fn step_break_target(&mut self) {
        if !self.break_held || !self.mouse_captured || !self.focused {
            self.game.set_break_target(IVec3::ZERO, 0.0, false);
            return;
        }
        let Some(hit) = self.target_hit() else {
            self.game.set_break_target(IVec3::ZERO, 0.0, false);
            return;
        };
        let Some((name, hardness)) = self.target_block(hit.block) else {
            self.game.set_break_target(IVec3::ZERO, 0.0, false);
            return;
        };
        let tool = self.game.selected_tool(&self.items);
        let total = break_seconds(hardness, &name, tool);
        self.game.set_break_target(hit.block, total, true);
    }

    fn target_block(&self, cell: IVec3) -> Option<(String, f32)> {
        let demo = self.demo.as_ref()?;
        let id = demo.block_at(cell)?;
        let name = self.pack.name(id)?.to_owned();
        let record = self.pack.records().get(usize::from(id))?;
        Some((name, record.hardness))
    }

    /// Whether the targeted block is a placed workbench.
    pub(super) fn targets_workbench(&self) -> bool {
        let Some(hit) = self.target_hit() else {
            return false;
        };
        self.target_block(hit.block)
            .is_some_and(|(name, _)| name == "core:workbench")
    }

    fn finish_break(&mut self, cell: IVec3) {
        let Some((name, _hardness)) = self.target_block(cell) else {
            return;
        };
        let tool = self.game.selected_tool(&self.items);
        let drops = drops_for(&self.pack, &self.items, &name, tool);
        let air = self.demo.as_ref().map(crate::demo::DemoRuntime::air);
        let removed = self
            .demo
            .as_mut()
            .is_some_and(|demo| air.is_some_and(|air| demo.edit(cell, air)));
        if !removed {
            return;
        }
        self.game.damage_selected();
        for stack in drops {
            self.game.entities.spawn(
                stack,
                block_centre(cell),
                spread_velocity(cell),
                &self.items,
            );
        }
    }

    /// Places the selected block item into the demo patch.
    pub(super) fn place_demo_block(&mut self) {
        let Some(hit) = self.target_hit() else {
            return;
        };
        let Some(cell) = place_position(hit) else {
            return;
        };
        if self.player.aabb().intersects_block(cell) {
            return;
        }
        let Some(demo) = &self.demo else {
            return;
        };
        let Some(existing) = demo.block_at(cell) else {
            return;
        };
        let replaceable = self
            .pack
            .records()
            .get(usize::from(existing))
            .is_some_and(|record| record.replaceable);
        if !replaceable {
            return;
        }
        let air = demo.air();
        let needs_clear = existing != air;
        let Some(stack) = self.game.inventory.selected() else {
            return;
        };
        let ItemKind::Block { block } = self.items.def(stack.item).kind else {
            return;
        };
        let name = self.items.name(stack.item);
        if (name == "core:plant" || name == "core:torch") && !demo.is_solid(cell - IVec3::Y) {
            return;
        }
        if needs_clear {
            let cleared = self.demo.as_mut().is_some_and(|demo| demo.edit(cell, air));
            if !cleared {
                return;
            }
        }
        let placed = self
            .demo
            .as_mut()
            .is_some_and(|demo| demo.edit(cell, block));
        if !placed {
            return;
        }
        self.game
            .inventory
            .remove(self.game.inventory.selected_index(), 1);
        debug!(?cell, %name, "placed item");
    }

    /// Uploads dropped-item cubes for this frame.
    pub(super) fn upload_items(&mut self) {
        let entries: Vec<(Vec3, &str)> = self
            .game
            .entities
            .iter()
            .map(|entity| (entity.position, self.items.name(entity.stack.item)))
            .collect();
        if let Some(renderer) = &mut self.renderer {
            renderer.set_items(&entries);
        }
    }
}

fn block_centre(cell: IVec3) -> Vec3 {
    cell.as_vec3() + Vec3::splat(0.5)
}

#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn spread_velocity(cell: IVec3) -> Vec3 {
    let seed = cell.x.wrapping_mul(73_856_093)
        ^ cell.y.wrapping_mul(19_349_663)
        ^ cell.z.wrapping_mul(83_492_791);
    let seed = seed.cast_unsigned();
    let x = f32::from((seed & 0xFF) as u8) / 255.0 - 0.5;
    let z = f32::from(((seed >> 8) & 0xFF) as u8) / 255.0 - 0.5;
    Vec3::new(x * 1.2, 2.5, z * 1.2)
}
