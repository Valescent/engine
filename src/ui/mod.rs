//! Gameplay UI state and screens.
//!
//! The app owns the `egui::Context` and the winit input state. This module owns
//! what is drawn and which screen owns the pointer or keyboard.

mod hud;
mod inventory;
mod item_icon;

use egui::{Context, Key};

use crate::game::{HOTBAR_SLOTS, Inventory, ItemRegistry, ItemStack, OFFHAND_SLOT};

/// The current UI state.
#[allow(
    clippy::struct_excessive_bools,
    reason = "inventory mode, close request, open-key suppression, and a drag in progress are independent UI states"
)]
#[derive(Default)]
pub(crate) struct UiState {
    inventory_open: bool,
    close_requested: bool,
    suppress_close_key: bool,
    held: Option<ItemStack>,
    focused_slot: usize,
    dragging: bool,
}

impl UiState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Draws one frame of UI.
    pub(crate) fn draw(
        &mut self,
        ctx: &Context,
        inventory: &mut Inventory,
        registry: &ItemRegistry,
        target: Option<&str>,
    ) {
        if self.inventory_open {
            if self.suppress_close_key {
                self.suppress_close_key = false;
            } else if ctx.input(|input| input.key_pressed(Key::E) || input.key_pressed(Key::Escape))
            {
                self.close_requested = true;
            }
            inventory::draw(ctx, self, inventory, registry);
        } else {
            hud::draw(
                ctx,
                hud::HudData {
                    inventory,
                    registry,
                    target,
                },
            );
        }
    }

    pub(crate) fn open_inventory(&mut self, selected: usize) {
        self.inventory_open = true;
        self.close_requested = false;
        self.suppress_close_key = true;
        self.held = None;
        self.dragging = false;
        self.focused_slot = selected.min(OFFHAND_SLOT);
    }

    pub(crate) fn close_inventory(&mut self) {
        self.inventory_open = false;
        self.close_requested = false;
        self.held = None;
        self.dragging = false;
    }

    pub(crate) fn inventory_open(&self) -> bool {
        self.inventory_open
    }

    pub(crate) fn take_close_requested(&mut self) -> bool {
        std::mem::take(&mut self.close_requested)
    }

    pub(crate) fn take_held(&mut self) -> Option<ItemStack> {
        self.held.take()
    }

    pub(crate) fn set_held(&mut self, stack: Option<ItemStack>) {
        self.held = stack;
    }

    pub(crate) fn captures_pointer(&self) -> bool {
        self.inventory_open
    }

    pub(crate) fn captures_keyboard(&self) -> bool {
        self.inventory_open
    }

    fn pick_up(&mut self, index: usize, inventory: &mut Inventory) {
        if self.held.is_some() {
            return;
        }
        let Some(stack) = inventory.slot(index) else {
            return;
        };
        self.held = inventory.remove(index, stack.count);
    }

    fn take_half(&mut self, index: usize, inventory: &mut Inventory) {
        if self.held.is_some() {
            return;
        }
        let Some(stack) = inventory.slot(index) else {
            return;
        };
        self.held = inventory.remove(index, stack.count.div_ceil(2));
    }

    fn activate(
        &mut self,
        index: usize,
        inventory: &mut Inventory,
        registry: &ItemRegistry,
        secondary: bool,
        shift: bool,
    ) {
        if shift && self.held.is_none() {
            let range = if index < HOTBAR_SLOTS {
                HOTBAR_SLOTS..OFFHAND_SLOT
            } else {
                0..HOTBAR_SLOTS
            };
            inventory.quick_move(index, range, registry);
            return;
        }
        if secondary {
            if self.held.is_some() {
                self.place_held(index, inventory, registry, true);
            } else {
                self.take_half(index, inventory);
            }
        } else if self.held.is_some() {
            self.place_held(index, inventory, registry, false);
        } else {
            self.pick_up(index, inventory);
        }
    }

    fn place_held(
        &mut self,
        index: usize,
        inventory: &mut Inventory,
        registry: &ItemRegistry,
        one: bool,
    ) {
        let Some(held) = self.held else {
            return;
        };
        let maximum = registry.def(held.item).max_stack;
        match inventory.slot(index) {
            None => {
                let count = if one { 1 } else { held.count };
                inventory.set_slot(index, Some(ItemStack { count, ..held }));
                self.held = (held.count > count).then_some(ItemStack {
                    count: held.count - count,
                    ..held
                });
            }
            Some(target) if target.item == held.item && target.durability == held.durability => {
                let room = maximum.saturating_sub(target.count);
                let requested = if one { 1 } else { held.count };
                let moved = room.min(requested);
                if moved > 0 {
                    inventory.set_slot(
                        index,
                        Some(ItemStack {
                            count: target.count + moved,
                            ..target
                        }),
                    );
                }
                self.held = (held.count > moved).then_some(ItemStack {
                    count: held.count - moved,
                    ..held
                });
            }
            Some(target) if !one => {
                inventory.set_slot(index, Some(held));
                self.held = Some(target);
            }
            Some(_) => {}
        }
    }

    fn return_held(&mut self, inventory: &mut Inventory, registry: &ItemRegistry) {
        if let Some(held) = self.held.take()
            && let Some(leftover) = inventory.add(held, registry)
        {
            self.held = Some(leftover);
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, RawInput, Rect, vec2};

    use super::*;
    use crate::content;

    fn registry() -> ItemRegistry {
        ItemRegistry::builtin(&content::v1_default())
    }

    fn input() -> RawInput {
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 720.0))),
            ..Default::default()
        }
    }

    fn key_input(key: egui::Key) -> RawInput {
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 720.0))),
            events: vec![egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        }
    }

    fn draw_with(
        state: &mut UiState,
        inventory: &mut Inventory,
        registry: &ItemRegistry,
        input: RawInput,
    ) {
        let ctx = Context::default();
        let mut output = ctx.run_pass(input, |ctx| {
            state.draw(ctx, inventory, registry, Some("core:stone"));
        });
        assert_ne!(output.shapes.len(), 0);
        output.textures_delta.clear();
    }

    fn draw(state: &mut UiState, inventory: &mut Inventory, registry: &ItemRegistry) {
        draw_with(state, inventory, registry, input());
    }

    #[test]
    fn the_hud_draws_without_a_window() {
        let registry = registry();
        let mut inventory = Inventory::new();
        let mut state = UiState::new();

        draw(&mut state, &mut inventory, &registry);

        assert!(!state.captures_pointer());
        assert!(!state.captures_keyboard());
    }

    #[test]
    fn the_inventory_draws_and_captures_input() {
        let registry = registry();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:stone", 12));
        let mut state = UiState::new();
        state.open_inventory(0);

        draw(&mut state, &mut inventory, &registry);

        assert!(state.captures_pointer());
        assert!(state.captures_keyboard());
    }

    #[test]
    fn click_picks_up_and_places_a_stack() {
        let registry = registry();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:stone", 12));
        let mut state = UiState::new();
        state.open_inventory(0);

        state.activate(0, &mut inventory, &registry, false, false);
        assert_eq!(state.held.unwrap().count, 12);
        assert!(inventory.slot(0).is_none());

        state.activate(1, &mut inventory, &registry, false, false);
        assert!(state.held.is_none());
        assert_eq!(inventory.slot(1).unwrap().count, 12);
    }

    #[test]
    fn right_click_splits_and_places_one() {
        let registry = registry();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:stone", 12));
        let mut state = UiState::new();
        state.open_inventory(0);

        state.activate(0, &mut inventory, &registry, true, false);
        assert_eq!(state.held.unwrap().count, 6);
        assert_eq!(inventory.slot(0).unwrap().count, 6);

        state.activate(1, &mut inventory, &registry, true, false);
        assert_eq!(state.held.unwrap().count, 5);
        assert_eq!(inventory.slot(1).unwrap().count, 1);
    }

    #[test]
    fn shift_click_moves_between_the_hotbar_and_main_inventory() {
        let registry = registry();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:stone", 12));
        let mut state = UiState::new();
        state.open_inventory(0);

        state.activate(0, &mut inventory, &registry, false, true);

        assert!(inventory.slot(0).is_none());
        assert_eq!(inventory.slot(HOTBAR_SLOTS).unwrap().count, 12);
    }

    #[test]
    fn the_opening_key_is_suppressed_and_the_next_press_closes() {
        let registry = registry();
        let mut inventory = Inventory::new();
        let mut state = UiState::new();
        state.open_inventory(0);

        // The same E press that opened the screen is still in this frame.
        draw_with(
            &mut state,
            &mut inventory,
            &registry,
            key_input(egui::Key::E),
        );
        assert!(!state.take_close_requested());

        // A new press on the next frame closes it.
        draw_with(
            &mut state,
            &mut inventory,
            &registry,
            key_input(egui::Key::E),
        );
        assert!(state.take_close_requested());
    }
}
