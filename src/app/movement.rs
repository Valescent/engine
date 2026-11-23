//! Per-frame movement input for the live player.

use glam::Vec3;
use winit::keyboard::KeyCode;

use crate::player::{BlockSource, PlayerInput};

use super::App;
use super::input::axis;

impl App {
    /// Builds this frame's movement input and advances the player against
    /// whichever block source is live: the demo patch, or the legacy world.
    pub(super) fn step_player(&mut self, delta_seconds: f32) {
        let forward_axis = axis(
            self.pressed.contains(&KeyCode::KeyW),
            self.pressed.contains(&KeyCode::KeyS),
        );
        let right_axis = axis(
            self.pressed.contains(&KeyCode::KeyD),
            self.pressed.contains(&KeyCode::KeyA),
        );
        let moving = forward_axis != 0.0 || right_axis != 0.0;
        let sprinting = !self.player.flying
            && moving
            && self.modifiers.shift_key()
            && self.game.survival.can_sprint();
        self.game.set_sprinting(sprinting);

        let mut jump_pressed = std::mem::take(&mut self.jump_requested);
        if !self.player.flying
            && jump_pressed
            && self.player.on_ground
            && !self.game.survival.spend_jump()
        {
            jump_pressed = false;
        }
        let input = PlayerInput {
            forward: forward_axis,
            right: right_axis,
            jump: if self.player.flying {
                self.pressed.contains(&KeyCode::Space)
            } else {
                jump_pressed
            },
            descend: self.player.flying && self.modifiers.control_key(),
            sprint: sprinting,
            yaw: self.camera.yaw(),
        };
        profiling::scope!("player update");
        let streaming = self.streaming.as_ref();
        let demo = self.demo.as_ref();
        let ready = |position: Vec3| match demo {
            Some(demo) => demo.is_ready(position),
            None => streaming.is_none_or(|streaming| streaming.aabb_ready(position)),
        };
        let source: &dyn BlockSource = match &self.demo {
            Some(demo) => demo,
            None => &self.world,
        };
        self.player
            .update_guarded(source, input, delta_seconds, &ready);
    }
}
