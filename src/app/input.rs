//! Key and mouse input: key mapping, cursor capture and release, hotbar and
//! movement helpers, the repair shortcuts, and the block actions they trigger.

use std::time::Instant;

use glam::IVec3;
use tracing::{debug, info, warn};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::CursorGrabMode;

use crate::block::Block;
use crate::content;
use crate::player::Player;
use crate::raycast::{RaycastHit, raycast, raycast_where};
use crate::save;
use crate::streaming::Streaming;
use crate::world::World;

use super::App;

const INTERACTION_DISTANCE: f32 = 6.0;

fn can_place(world: &World, player: &Player, position: IVec3) -> bool {
    !world.is_solid(position) && !player.aabb().intersects_block(position)
}

fn place_position(hit: RaycastHit) -> Option<IVec3> {
    Some(IVec3::new(
        hit.block.x.checked_add(hit.normal.x)?,
        hit.block.y.checked_add(hit.normal.y)?,
        hit.block.z.checked_add(hit.normal.z)?,
    ))
}

/// The repair action a shortcut key selects.
fn repair_action(code: KeyCode) -> Option<save::RepairAction> {
    match code {
        KeyCode::KeyR => Some(save::RepairAction::RestoreBackup),
        KeyCode::KeyQ => Some(save::RepairAction::Quarantine),
        KeyCode::KeyS => Some(save::RepairAction::Salvage),
        _ => None,
    }
}

/// The stable content name of a hotbar block (`A5.1`).
///
/// The hotbar still selects legacy [`Block`] values; the new terrain only knows
/// names, so the mapping lives here.
fn legacy_block_name(block: Block) -> &'static str {
    match block {
        Block::Air => "core:air",
        Block::Stone => "core:stone",
        Block::Dirt => "core:dirt",
        Block::Grass => "core:grass",
        Block::Wood => "core:wood",
        Block::Leaves => "core:leaves",
        Block::Lamp => "core:lamp",
        Block::Water => "core:water",
        Block::Sand => "core:sand",
    }
}

impl App {
    pub(super) fn handle_key(&mut self, event: &KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else {
            return;
        };

        match event.state {
            ElementState::Pressed => {
                if event.repeat {
                    return;
                }
                if self
                    .demo
                    .as_ref()
                    .is_some_and(|demo| demo.repair_key().is_some())
                {
                    if let Some(action) = repair_action(code) {
                        self.resolve_repair(action);
                    }
                    return;
                }
                if code == KeyCode::KeyS && self.modifiers.control_key() {
                    self.save_world();
                } else if code == KeyCode::Space {
                    self.jump_requested = true;
                    // Held while flying: `Space` ascends continuously.
                    self.pressed.insert(code);
                } else if code == KeyCode::Escape {
                    self.release_cursor();
                } else if code == KeyCode::KeyV {
                    self.toggle_vsync();
                } else if code == KeyCode::F3 {
                    self.toggle_overlay();
                } else if code == KeyCode::KeyF {
                    self.player.toggle_fly();
                    info!(flying = self.player.flying, "debug fly toggled");
                } else if code == KeyCode::KeyT {
                    self.sky.skip_hour();
                    debug!(time_of_day = self.sky.time_of_day(), "time advanced");
                } else if code == KeyCode::F4 {
                    self.unlit = !self.unlit;
                    debug!(unlit = self.unlit, "unlit shading toggled");
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                } else if let Some(block) = hotbar_block(code) {
                    self.selected_block = block;
                    debug!(?block, "selected block");
                } else if is_movement_key(code) {
                    self.pressed.insert(code);
                }
            }
            ElementState::Released => {
                self.pressed.remove(&code);
            }
        }
    }

    pub(super) fn capture_cursor(&mut self) {
        if self.mouse_captured {
            return;
        }
        let Some(window) = &self.window else {
            return;
        };

        if let Err(locked_error) = window.set_cursor_grab(CursorGrabMode::Locked) {
            debug!(%locked_error, "locked cursor grab unavailable; trying confined grab");
            if let Err(error) = window.set_cursor_grab(CursorGrabMode::Confined) {
                warn!(%error, "failed to capture the cursor");
                return;
            }
        }
        window.set_cursor_visible(false);
        self.mouse_captured = true;
        debug!("cursor captured");
    }

    pub(super) fn release_cursor(&mut self) {
        if self.mouse_captured {
            debug!("cursor released");
        }
        if let Some(window) = &self.window {
            if let Err(error) = window.set_cursor_grab(CursorGrabMode::None) {
                warn!(%error, "failed to release the cursor");
            }
            window.set_cursor_visible(true);
        }
        self.mouse_captured = false;
    }

    pub(super) fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if focused {
            self.last_frame = Instant::now();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        } else {
            self.pressed.clear();
            self.jump_requested = false;
            self.highlighted_block = None;
            if let Some(renderer) = &mut self.renderer {
                renderer.set_highlight(&[]);
            }
            self.release_cursor();
        }
    }

    pub(super) fn apply_mouse_look(&mut self, delta_x: f32, delta_y: f32) {
        if self.mouse_captured && self.focused {
            self.camera.look(delta_x, delta_y);
        }
    }

    pub(super) fn target_hit(&self) -> Option<RaycastHit> {
        let origin = self.player.eye_position();
        let direction = self.camera.forward();
        match &self.demo {
            // The patch is what the player can see and stand on, so interaction
            // must target it, not the legacy world underneath (`F1.5`).
            Some(demo) => raycast_where(
                |cell| demo.is_solid(cell),
                origin,
                direction,
                INTERACTION_DISTANCE,
            ),
            None => raycast(&self.world, origin, direction, INTERACTION_DISTANCE),
        }
    }

    pub(super) fn break_block(&mut self) {
        if self.demo.is_some() {
            self.demo_edit(true);
            return;
        }
        let Some(hit) = self.target_hit() else {
            return;
        };
        let dirty = self.world.set_block(hit.block, Block::Air);
        debug!(block = ?hit.block, "broke block");
        self.invalidate_chunks(&dirty);
    }

    pub(super) fn place_block(&mut self) {
        if self.demo.is_some() {
            self.demo_edit(false);
            return;
        }
        let Some(hit) = self.target_hit() else {
            return;
        };
        let Some(position) = place_position(hit) else {
            return;
        };
        if !can_place(&self.world, &self.player, position) {
            return;
        }
        let dirty = self.world.set_block(position, self.selected_block);
        debug!(block = ?position, selected = ?self.selected_block, "placed block");
        self.invalidate_chunks(&dirty);
    }

    /// Applies the player's `world-repair` choice and uploads the reloaded set.
    pub(super) fn resolve_repair(&mut self, action: save::RepairAction) {
        let column = Streaming::column_at(self.player.position);
        let column = (column.x, column.y);
        let repaired = self
            .demo
            .as_mut()
            .is_some_and(|demo| demo.repair(action, column));
        if repaired {
            self.upload_demo();
        }
    }

    /// Applies one break or place to the demo patch.
    ///
    /// The demo owns the `y = 0` rule, the empty-cell rule, the save queue, and
    /// the rebuild; the app resolves the target and the selected block name.
    fn demo_edit(&mut self, break_it: bool) {
        let Some(hit) = self.target_hit() else {
            return;
        };
        let (cell, id) = if break_it {
            let Some(demo) = &self.demo else {
                return;
            };
            (hit.block, demo.air())
        } else {
            let Some(cell) = place_position(hit) else {
                return;
            };
            if self.player.aabb().intersects_block(cell) {
                return;
            }
            let Some(id) = content::v1_default().id(legacy_block_name(self.selected_block)) else {
                return;
            };
            (cell, id)
        };
        if let Some(demo) = &mut self.demo {
            demo.edit(cell, id);
        }
    }
}

pub(super) fn axis(positive: bool, negative: bool) -> f32 {
    match (positive, negative) {
        (true, false) => 1.0,
        (false, true) => -1.0,
        _ => 0.0,
    }
}

fn is_movement_key(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::KeyW | KeyCode::KeyA | KeyCode::KeyS | KeyCode::KeyD
    )
}

fn hotbar_block(code: KeyCode) -> Option<Block> {
    let slot = match code {
        KeyCode::Digit1 => 1,
        KeyCode::Digit2 => 2,
        KeyCode::Digit3 => 3,
        KeyCode::Digit4 => 4,
        KeyCode::Digit5 => 5,
        _ => return None,
    };
    Block::from_hotbar(slot)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use glam::Vec3;

    use crate::world::ChunkCoord;

    use super::super::tests::{InlinePool, demo_world};
    use super::super::{SPAWN_X, SPAWN_Z};
    use super::*;

    #[test]
    fn repair_shortcuts_map_to_actions() {
        assert_eq!(
            repair_action(KeyCode::KeyR),
            Some(save::RepairAction::RestoreBackup)
        );
        assert_eq!(
            repair_action(KeyCode::KeyQ),
            Some(save::RepairAction::Quarantine)
        );
        assert_eq!(
            repair_action(KeyCode::KeyS),
            Some(save::RepairAction::Salvage)
        );
        assert_eq!(repair_action(KeyCode::KeyX), None);
    }
    #[test]
    fn mouse_look_requires_capture_and_focus() {
        let mut app = App::new(demo_world());
        let initial = app.camera.view_projection();

        app.apply_mouse_look(100.0, 0.0);
        assert_eq!(app.camera.view_projection(), initial);

        app.mouse_captured = true;
        app.focused = false;
        app.apply_mouse_look(100.0, 0.0);
        assert_eq!(app.camera.view_projection(), initial);

        app.focused = true;
        app.apply_mouse_look(100.0, 0.0);
        assert_ne!(app.camera.view_projection(), initial);
    }

    #[test]
    fn an_edit_waits_for_light_before_requesting_meshes() {
        let mut app = App::new(demo_world());
        app.player = Player::new(Vec3::new(32.5, 2.5, 32.5));
        app.camera.set_position(app.player.eye_position());
        app.camera.look(0.0, 100_000.0);

        let pool = Arc::new(InlinePool::default());
        app.mesher = Some(pool.clone());
        let target = IVec3::new(SPAWN_X, 1, SPAWN_Z);
        let chunk = ChunkCoord::from_world(target).0;

        app.break_block();
        assert!(!app.queue.light_is_current(chunk));
        assert!(pool.light_jobs.load(Ordering::Relaxed) >= 1);
        assert_eq!(pool.mesh_jobs.load(Ordering::Relaxed), 0);

        app.upload_finished_meshes(&*pool);
        assert!(app.queue.light_is_current(chunk));
        assert!(app.world.light_field(chunk).is_some());

        app.request_levels(&*pool);
        assert!(pool.mesh_jobs.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn breaking_a_block_invalidates_light_and_waits_for_rebuild() {
        let mut app = App::new(demo_world());
        app.player = Player::new(Vec3::new(32.5, 2.5, 32.5));
        app.camera.set_position(app.player.eye_position());
        app.camera.look(0.0, 100_000.0);

        let target = IVec3::new(SPAWN_X, 1, SPAWN_Z);
        let chunk = ChunkCoord::from_world(target).0;
        assert!(app.queue.light_is_current(chunk));

        app.break_block();

        assert_eq!(app.world.block(target), Block::Air);
        assert!(!app.queue.light_is_current(chunk));
    }
    #[test]
    fn placement_rejects_occupied_cells_and_the_player_box() {
        let mut world = World::default();
        world.set_block(IVec3::new(1, 0, 0), Block::Stone);
        let player = Player::new(Vec3::new(0.5, 0.0, 0.5));

        assert!(!can_place(&world, &player, IVec3::new(1, 0, 0)));
        assert!(!can_place(&world, &player, IVec3::ZERO));
        assert!(can_place(&world, &player, IVec3::new(2, 0, 0)));
    }

    #[test]
    fn breaking_a_targeted_block_clears_it() {
        let mut app = App::new(demo_world());
        app.player = Player::new(Vec3::new(32.5, 2.5, 32.5));
        app.camera.set_position(app.player.eye_position());
        app.camera.look(0.0, 100_000.0);

        assert_eq!(
            app.world.block(IVec3::new(SPAWN_X, 1, SPAWN_Z)),
            Block::Grass
        );
        app.break_block();
        assert_eq!(app.world.block(IVec3::new(SPAWN_X, 1, SPAWN_Z)), Block::Air);
    }
}
