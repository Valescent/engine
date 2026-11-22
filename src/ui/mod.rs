//! Gameplay UI state and screens.
//!
//! The app owns the `egui::Context` and the winit input state. This module owns
//! what is drawn and which screen owns the pointer or keyboard.

mod crafting;
mod hud;
mod inventory;
mod item_icon;

use egui::{Context, Key};

use crate::game::{
    CraftError, CraftGrid, HOTBAR_SLOTS, Inventory, ItemRegistry, ItemStack, OFFHAND_SLOT,
    RecipeBook, Station, craft_all_from_grid, craft_direct_all, craft_once_from_grid, salvage_held,
};

/// The current UI state.
#[allow(
    clippy::struct_excessive_bools,
    reason = "inventory mode, close request, open-key suppression, and a drag in progress are independent UI states"
)]
#[derive(Default)]
pub(crate) struct UiState {
    inventory_open: bool,
    station: Station,
    close_requested: bool,
    suppress_close_key: bool,
    held: Option<ItemStack>,
    focused_slot: usize,
    dragging: bool,
    craft: CraftGrid,
    recipe_search: String,
    selected_recipe: Option<usize>,
    craft_error: Option<CraftError>,
    search_focused: bool,
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
        recipes: &RecipeBook,
        target: Option<&str>,
    ) {
        if self.inventory_open {
            if self.suppress_close_key {
                self.suppress_close_key = false;
            } else {
                let escape = ctx.input(|input| input.key_pressed(Key::Escape));
                let close = ctx.input(|input| input.key_pressed(Key::E));
                if escape || (close && !self.search_focused) {
                    self.close_requested = true;
                }
            }
            inventory::draw(ctx, self, inventory, registry, recipes);
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

    /// Opens the inventory screen with its 2x2 grid.
    pub(crate) fn open_inventory(&mut self, selected: usize) {
        self.open(selected, Station::Inventory);
    }

    /// Opens the workbench screen with its 3x3 grid.
    pub(crate) fn open_workbench(&mut self, selected: usize) {
        self.open(selected, Station::Workbench);
    }

    fn open(&mut self, selected: usize, station: Station) {
        self.inventory_open = true;
        self.station = station;
        self.close_requested = false;
        self.suppress_close_key = true;
        self.held = None;
        self.dragging = false;
        self.focused_slot = selected.min(OFFHAND_SLOT);
        self.craft = CraftGrid::new(station.grid_size());
        self.recipe_search.clear();
        self.selected_recipe = None;
        self.craft_error = None;
        self.search_focused = false;
    }

    pub(crate) fn close_inventory(&mut self) {
        self.inventory_open = false;
        self.close_requested = false;
        self.held = None;
        self.dragging = false;
        self.craft = CraftGrid::default();
        self.recipe_search.clear();
        self.selected_recipe = None;
        self.craft_error = None;
        self.search_focused = false;
    }

    pub(crate) fn inventory_open(&self) -> bool {
        self.inventory_open
    }

    /// The open station, or the inventory when the screen is closed.
    pub(crate) fn station(&self) -> Station {
        self.station
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

    /// Returns the crafting grid to the inventory. Leaves everything
    /// untouched and returns `false` when the inventory cannot hold it all.
    pub(crate) fn stow_crafting(
        &mut self,
        inventory: &mut Inventory,
        registry: &ItemRegistry,
    ) -> bool {
        if self.craft.is_empty() {
            return true;
        }
        let mut next_inventory = inventory.clone();
        let mut next_craft = self.craft.clone();
        for stack in next_craft.drain() {
            if next_inventory.add(stack, registry).is_some() {
                return false;
            }
        }
        *inventory = next_inventory;
        self.craft = next_craft;
        true
    }

    /// Best-effort return of the cursor and grid before a save or quit.
    pub(crate) fn stow_ui_items(&mut self, inventory: &mut Inventory, registry: &ItemRegistry) {
        if let Some(held) = self.held.take()
            && let Some(leftover) = inventory.add(held, registry)
        {
            self.held = Some(leftover);
        }
        let _ = self.stow_crafting(inventory, registry);
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

    pub(super) fn click_craft_slot(
        &mut self,
        index: usize,
        inventory: &mut Inventory,
        registry: &ItemRegistry,
        secondary: bool,
        shift: bool,
    ) {
        if shift && self.held.is_none() {
            let Some(stack) = self.craft.take(index, u16::MAX) else {
                return;
            };
            if let Some(leftover) = inventory.add(stack, registry) {
                self.craft.set_slot(index, Some(leftover));
            }
            return;
        }
        if secondary {
            if self.held.is_some() {
                self.place_held_in_craft(index, registry, true);
            } else {
                self.take_craft_half(index);
            }
        } else if self.held.is_some() {
            self.place_held_in_craft(index, registry, false);
        } else {
            self.held = self.craft.take(index, u16::MAX);
        }
    }

    fn place_held_in_craft(&mut self, index: usize, registry: &ItemRegistry, one: bool) {
        let Some(held) = self.held else {
            return;
        };
        let maximum = registry.def(held.item).max_stack;
        match self.craft.slot(index) {
            None => {
                let count = if one { 1 } else { held.count };
                self.craft
                    .set_slot(index, Some(ItemStack { count, ..held }));
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
                    self.craft.set_slot(
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
                self.craft.set_slot(index, Some(held));
                self.held = Some(target);
            }
            Some(_) => {}
        }
    }

    fn take_craft_half(&mut self, index: usize) {
        if self.held.is_some() {
            return;
        }
        let Some(stack) = self.craft.slot(index) else {
            return;
        };
        self.held = self.craft.take(index, stack.count.div_ceil(2));
    }

    pub(super) fn click_craft_output(
        &mut self,
        inventory: &mut Inventory,
        recipes: &RecipeBook,
        registry: &ItemRegistry,
        all: bool,
    ) {
        self.craft_error = None;
        if all {
            let (crafted, error) = craft_all_from_grid(
                &mut self.craft,
                recipes,
                self.station,
                inventory,
                registry,
                u16::MAX,
            );
            if crafted == 0 {
                self.craft_error = error;
            }
        } else {
            self.craft_error =
                craft_once_from_grid(&mut self.craft, recipes, self.station, inventory, registry)
                    .err();
        }
    }

    pub(super) fn craft_direct_recipe(
        &mut self,
        index: usize,
        inventory: &mut Inventory,
        recipes: &RecipeBook,
        registry: &ItemRegistry,
        limit: u16,
    ) {
        self.craft_error = None;
        let Some(recipe) = recipes.get(index) else {
            return;
        };
        if !self.station.can_make(recipe.recipe.station) {
            self.craft_error = Some(CraftError::NotAtStation);
            return;
        }
        let (crafted, error) = craft_direct_all(recipe, inventory, registry, limit);
        if crafted == 0 {
            self.craft_error = error;
        }
    }

    pub(super) fn salvage_held_item(&mut self, inventory: &mut Inventory, registry: &ItemRegistry) {
        self.craft_error = salvage_held(&mut self.held, inventory, registry).err();
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
    use crate::game::TOTAL_SLOTS;

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
        let recipes = RecipeBook::builtin(registry).expect("the built-in recipes are valid");
        let mut output = ctx.run_pass(input, |ctx| {
            state.draw(ctx, inventory, registry, &recipes, Some("core:stone"));
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

    #[test]
    fn the_workbench_opens_a_three_by_three_grid() {
        let mut state = UiState::new();
        state.open_workbench(0);

        assert_eq!(state.station(), Station::Workbench);
        assert_eq!(state.craft.size(), 3);
    }

    #[test]
    fn crafting_output_consumes_the_grid_and_adds_to_inventory() {
        let registry = registry();
        let recipes = RecipeBook::builtin(&registry).unwrap();
        let mut inventory = Inventory::new();
        let mut state = UiState::new();
        state.open_inventory(0);
        state.craft.set_slot(0, registry.stack("core:wood", 1));

        state.click_craft_output(&mut inventory, &recipes, &registry, false);

        assert!(state.craft_error.is_none());
        assert!(state.craft.is_empty());
        assert_eq!(inventory.count(registry.id("core:planks").unwrap()), 4);
    }

    #[test]
    fn closing_returns_grid_items_to_the_inventory() {
        let registry = registry();
        let mut inventory = Inventory::new();
        let mut state = UiState::new();
        state.open_inventory(0);
        state.craft.set_slot(0, registry.stack("core:wood", 1));

        assert!(state.stow_crafting(&mut inventory, &registry));
        assert!(state.craft.is_empty());
        assert_eq!(inventory.count(registry.id("core:wood").unwrap()), 1);
    }

    #[test]
    fn a_full_inventory_keeps_grid_items_in_place() {
        let registry = registry();
        let mut inventory = Inventory::new();
        for index in 0..TOTAL_SLOTS {
            inventory.set_slot(index, registry.stack("core:dirt", 64));
        }
        let mut state = UiState::new();
        state.open_inventory(0);
        state.craft.set_slot(0, registry.stack("core:wood", 1));

        assert!(!state.stow_crafting(&mut inventory, &registry));
        assert_eq!(state.craft.slot(0).unwrap().count, 1);
    }

    #[test]
    fn clicking_moves_a_stack_between_inventory_and_the_craft_grid() {
        let registry = registry();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:stone", 5));
        let mut state = UiState::new();
        state.open_inventory(0);

        state.activate(0, &mut inventory, &registry, false, false);
        state.click_craft_slot(0, &mut inventory, &registry, false, false);

        assert!(state.held.is_none());
        assert_eq!(state.craft.slot(0).unwrap().count, 5);
        assert!(inventory.slot(0).is_none());

        state.click_craft_slot(0, &mut inventory, &registry, false, false);
        assert_eq!(state.held.unwrap().count, 5);
        assert!(state.craft.slot(0).is_none());
    }

    #[test]
    fn shift_clicking_a_craft_slot_moves_it_to_the_inventory() {
        let registry = registry();
        let mut inventory = Inventory::new();
        let mut state = UiState::new();
        state.open_workbench(0);
        state.craft.set_slot(0, registry.stack("core:planks", 3));

        state.click_craft_slot(0, &mut inventory, &registry, false, true);

        assert!(state.craft.slot(0).is_none());
        assert_eq!(inventory.count(registry.id("core:planks").unwrap()), 3);
    }

    #[test]
    fn a_workbench_recipe_is_rejected_on_the_inventory_screen() {
        let registry = registry();
        let recipes = RecipeBook::builtin(&registry).unwrap();
        let index = recipes
            .iter()
            .position(|recipe| recipe.name() == "core:wooden_pickaxe")
            .unwrap();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:planks", 3));
        inventory.set_slot(1, registry.stack("core:stick", 2));
        let mut state = UiState::new();
        state.open_inventory(0);

        state.craft_direct_recipe(index, &mut inventory, &recipes, &registry, 1);

        assert_eq!(state.craft_error, Some(CraftError::NotAtStation));
        assert_eq!(inventory.count(registry.id("core:planks").unwrap()), 3);
        assert_eq!(inventory.count(registry.id("core:stick").unwrap()), 2);
    }

    #[test]
    fn a_workbench_recipe_crafts_on_the_workbench_screen() {
        let registry = registry();
        let recipes = RecipeBook::builtin(&registry).unwrap();
        let index = recipes
            .iter()
            .position(|recipe| recipe.name() == "core:wooden_pickaxe")
            .unwrap();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:planks", 3));
        inventory.set_slot(1, registry.stack("core:stick", 2));
        let mut state = UiState::new();
        state.open_workbench(0);

        state.craft_direct_recipe(index, &mut inventory, &recipes, &registry, 1);

        assert!(state.craft_error.is_none());
        assert_eq!(
            inventory.count(registry.id("core:wooden_pickaxe").unwrap()),
            1
        );
        assert_eq!(inventory.count(registry.id("core:planks").unwrap()), 0);
        assert_eq!(inventory.count(registry.id("core:stick").unwrap()), 0);
    }

    #[test]
    fn salvaging_the_held_tool_returns_its_materials() {
        let registry = registry();
        let mut inventory = Inventory::new();
        let mut state = UiState::new();
        state.open_inventory(0);
        state.held = registry.stack("core:stone_pickaxe", 1);

        state.salvage_held_item(&mut inventory, &registry);

        assert!(state.craft_error.is_none());
        assert!(state.held.is_none());
        assert_eq!(inventory.count(registry.id("core:stone").unwrap()), 1);
        assert_eq!(inventory.count(registry.id("core:stick").unwrap()), 1);
    }
}
