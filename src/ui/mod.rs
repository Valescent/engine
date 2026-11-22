//! Gameplay UI state and screens.
//!
//! The app owns the `egui::Context` and the winit input state. This module owns
//! what is drawn and, later, which screen owns the pointer or keyboard.

mod hud;

use egui::Context;

pub(crate) use hud::HudData;

/// The current UI state.
///
/// `0.0.10.2` starts with a non-interactive HUD. The inventory and crafting
/// screens add a `screen` field here and make the capture queries return true.
#[derive(Default)]
pub(crate) struct UiState;

#[allow(
    clippy::unused_self,
    reason = "the inventory and crafting screens mutate this state in 0.0.10.2"
)]
impl UiState {
    pub(crate) fn new() -> Self {
        Self
    }

    /// Draws one frame of UI.
    pub(crate) fn draw(&mut self, ctx: &Context, data: HudData<'_>) {
        hud::draw(ctx, data);
    }

    /// Whether an open screen owns the pointer.
    pub(crate) fn captures_pointer(&self) -> bool {
        false
    }

    /// Whether an open screen owns the keyboard.
    pub(crate) fn captures_keyboard(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, RawInput, Rect, vec2};

    use super::*;
    use crate::content;
    use crate::game::{Inventory, ItemRegistry};

    fn registry() -> ItemRegistry {
        ItemRegistry::builtin(&content::v1_default())
    }

    fn input() -> RawInput {
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 720.0))),
            ..Default::default()
        }
    }

    #[test]
    fn the_hud_draws_without_a_window() {
        let registry = registry();
        let inventory = Inventory::new();
        let ctx = Context::default();
        let mut state = UiState::new();

        let mut output = ctx.run_pass(input(), |ctx| {
            state.draw(
                ctx,
                HudData {
                    inventory: &inventory,
                    registry: &registry,
                    target: Some("core:stone"),
                },
            );
        });

        assert_ne!(output.shapes.len(), 0);
        output.textures_delta.clear();
    }

    #[test]
    fn the_hud_does_not_capture_input() {
        let state = UiState::new();

        assert!(!state.captures_pointer());
        assert!(!state.captures_keyboard());
    }
}
