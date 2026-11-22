//! Crafting grids, transactional crafting, direct inventory crafts, and
//! static salvage.

use thiserror::Error;

use super::inventory::{Inventory, TOTAL_SLOTS};
use super::item::{ItemId, ItemRegistry, ItemStack};
use super::recipe::{CompiledRecipe, MAX_CRAFT_SLOTS, RecipeBook, RecipeMatch, Station};

/// A transient 2x2 or 3x3 crafting grid.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CraftGrid {
    size: u8,
    slots: [Option<ItemStack>; MAX_CRAFT_SLOTS],
}

impl Default for CraftGrid {
    fn default() -> Self {
        Self::new(2)
    }
}

impl CraftGrid {
    /// Creates an empty grid with an edge of 2 or 3.
    pub(crate) fn new(size: u8) -> Self {
        assert!(matches!(size, 2 | 3), "craft grids are 2x2 or 3x3");
        Self {
            size,
            slots: [None; MAX_CRAFT_SLOTS],
        }
    }

    /// The grid edge.
    pub(crate) fn size(&self) -> u8 {
        self.size
    }

    /// All nine backing slots. Slots outside the active size stay empty.
    pub(crate) fn slots(&self) -> &[Option<ItemStack>; MAX_CRAFT_SLOTS] {
        &self.slots
    }

    /// One active slot.
    pub(crate) fn slot(&self, index: usize) -> Option<ItemStack> {
        if index < usize::from(self.size) * usize::from(self.size) {
            self.slots.get(index).copied().flatten()
        } else {
            None
        }
    }

    /// Replaces one active slot.
    pub(crate) fn set_slot(&mut self, index: usize, stack: Option<ItemStack>) {
        if index < usize::from(self.size) * usize::from(self.size)
            && let Some(slot) = self.slots.get_mut(index)
        {
            *slot = stack.filter(|stack| stack.count > 0);
        }
    }

    /// Removes up to `count` items from one slot.
    pub(crate) fn take(&mut self, index: usize, count: u16) -> Option<ItemStack> {
        if index >= usize::from(self.size) * usize::from(self.size) || count == 0 {
            return None;
        }
        let slot = self.slots.get_mut(index)?;
        let mut stack = (*slot)?;
        let removed = count.min(stack.count);
        stack.count = removed;
        let remaining = slot.as_mut()?;
        remaining.count -= removed;
        if remaining.count == 0 {
            *slot = None;
        }
        Some(stack)
    }

    /// Removes every item from the grid.
    pub(crate) fn drain(&mut self) -> Vec<ItemStack> {
        let mut drained = Vec::new();
        for index in 0..usize::from(self.size) * usize::from(self.size) {
            if let Some(stack) = self.slots[index].take() {
                drained.push(stack);
            }
        }
        drained
    }

    /// Whether every active slot is empty.
    pub(crate) fn is_empty(&self) -> bool {
        (0..usize::from(self.size) * usize::from(self.size))
            .all(|index| self.slots[index].is_none())
    }
}

/// A failed craft or salvage.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub(crate) enum CraftError {
    /// The grid does not match a recipe.
    #[error("the grid does not match a recipe")]
    NoMatch,
    /// The inventory or grid is missing an ingredient.
    #[error("missing ingredients")]
    MissingIngredients,
    /// The output does not fit in the inventory.
    #[error("the inventory is full")]
    InventoryFull,
    /// The held item has no salvage returns.
    #[error("that item cannot be salvaged")]
    NotSalvageable,
    /// There is no held item to salvage.
    #[error("no item is held")]
    NothingHeld,
    /// The recipe needs a station the current screen does not offer.
    #[error("that recipe needs a workbench")]
    NotAtStation,
}

/// Crafts one grid recipe, consuming the grid only when the output fits.
pub(crate) fn craft_from_grid(
    grid: &mut CraftGrid,
    matched: RecipeMatch<'_>,
    inventory: &mut Inventory,
    registry: &ItemRegistry,
) -> Result<ItemStack, CraftError> {
    let mut next_grid = grid.clone();
    if !matched
        .recipe
        .consume_grid(&mut next_grid, matched.mirrored)
    {
        return Err(CraftError::NoMatch);
    }
    let output = matched
        .recipe
        .output_stack(registry)
        .ok_or(CraftError::MissingIngredients)?;
    let mut next_inventory = inventory.clone();
    if next_inventory.add(output, registry).is_some() {
        return Err(CraftError::InventoryFull);
    }
    *grid = next_grid;
    *inventory = next_inventory;
    Ok(output)
}

/// Finds and crafts one recipe from the current grid.
pub(crate) fn craft_once_from_grid(
    grid: &mut CraftGrid,
    book: &RecipeBook,
    station: Station,
    inventory: &mut Inventory,
    registry: &ItemRegistry,
) -> Result<ItemStack, CraftError> {
    let matched = book.find(grid, station).ok_or(CraftError::NoMatch)?;
    craft_from_grid(grid, matched, inventory, registry)
}

/// Crafts as many grid recipes as the ingredients and inventory allow.
pub(crate) fn craft_all_from_grid(
    grid: &mut CraftGrid,
    book: &RecipeBook,
    station: Station,
    inventory: &mut Inventory,
    registry: &ItemRegistry,
    limit: u16,
) -> (u16, Option<CraftError>) {
    let mut crafted = 0;
    while crafted < limit {
        let Some(matched) = book.find(grid, station) else {
            break;
        };
        match craft_from_grid(grid, matched, inventory, registry) {
            Ok(_) => crafted += 1,
            Err(error) => return (crafted, Some(error)),
        }
    }
    (crafted, None)
}

/// Crafts one recipe directly from inventory contents.
///
/// This is the recipe-book shortcut. It ignores arrangement and consumes the
/// compiled ingredient totals, but still rolls back completely when anything
/// is missing or the output does not fit.
pub(crate) fn craft_direct(
    recipe: &CompiledRecipe,
    inventory: &mut Inventory,
    registry: &ItemRegistry,
) -> Result<ItemStack, CraftError> {
    let mut next = inventory.clone();
    for ingredient in recipe.requirements() {
        consume_ingredient(&mut next, ingredient)?;
    }
    let output = recipe
        .output_stack(registry)
        .ok_or(CraftError::MissingIngredients)?;
    if next.add(output, registry).is_some() {
        return Err(CraftError::InventoryFull);
    }
    *inventory = next;
    Ok(output)
}

/// Crafts a direct recipe up to `limit` times. A later failure keeps the
/// successful crafts and reports the stop reason.
pub(crate) fn craft_direct_all(
    recipe: &CompiledRecipe,
    inventory: &mut Inventory,
    registry: &ItemRegistry,
    limit: u16,
) -> (u16, Option<CraftError>) {
    let mut crafted = 0;
    while crafted < limit {
        match craft_direct(recipe, inventory, registry) {
            Ok(_) => crafted += 1,
            Err(error) => return (crafted, Some(error)),
        }
    }
    (crafted, None)
}

fn consume_ingredient(
    inventory: &mut Inventory,
    ingredient: &super::recipe::CompiledIngredient,
) -> Result<(), CraftError> {
    let mut remaining = ingredient.count;
    for index in 0..TOTAL_SLOTS {
        if remaining == 0 {
            break;
        }
        let Some(stack) = inventory.slot(index) else {
            continue;
        };
        if !ingredient.accepts(stack.item) {
            continue;
        }
        let take = remaining.min(stack.count);
        let _ = inventory.remove(index, take);
        remaining -= take;
    }
    if remaining == 0 {
        Ok(())
    } else {
        Err(CraftError::MissingIngredients)
    }
}

/// The static salvage returns for one item, or an empty list.
pub(crate) fn salvage_output(item: ItemId, registry: &ItemRegistry) -> Vec<ItemStack> {
    let name = registry.name(item);
    let returns: &[(&str, u16)] = match name {
        "core:wooden_pickaxe" | "core:wooden_axe" | "core:wooden_shovel" | "core:wooden_sword" => {
            &[("core:planks", 1), ("core:stick", 1)]
        }
        "core:stone_pickaxe" | "core:stone_axe" | "core:stone_shovel" | "core:stone_sword" => {
            &[("core:stone", 1), ("core:stick", 1)]
        }
        "core:iron_pickaxe" => &[("core:ore_iron", 1), ("core:stick", 1)],
        "core:workbench" => &[("core:planks", 2)],
        "core:torch" => &[("core:stick", 1)],
        _ => &[],
    };
    returns
        .iter()
        .filter_map(|(name, count)| registry.stack(name, *count))
        .collect()
}

/// Salvages one item from the cursor stack. Returns the produced stacks.
pub(crate) fn salvage_held(
    held: &mut Option<ItemStack>,
    inventory: &mut Inventory,
    registry: &ItemRegistry,
) -> Result<Vec<ItemStack>, CraftError> {
    let Some(stack) = *held else {
        return Err(CraftError::NothingHeld);
    };
    let outputs = salvage_output(stack.item, registry);
    if outputs.is_empty() {
        return Err(CraftError::NotSalvageable);
    }
    let mut next = inventory.clone();
    for output in &outputs {
        if next.add(*output, registry).is_some() {
            return Err(CraftError::InventoryFull);
        }
    }
    *inventory = next;
    *held = (stack.count > 1).then_some(ItemStack {
        count: stack.count - 1,
        ..stack
    });
    Ok(outputs)
}

#[cfg(test)]
mod tests {
    use crate::content;

    use super::super::recipe::Station;
    use super::*;

    fn tables() -> (ItemRegistry, RecipeBook) {
        let registry = ItemRegistry::builtin(&content::v1_default());
        let book = RecipeBook::builtin(&registry).expect("the built-in recipes are valid");
        (registry, book)
    }

    fn grid(registry: &ItemRegistry, size: u8, items: &[(usize, &str, u16)]) -> CraftGrid {
        let mut grid = CraftGrid::new(size);
        for &(index, name, count) in items {
            grid.set_slot(index, registry.stack(name, count));
        }
        grid
    }

    #[test]
    fn wood_becomes_planks_in_the_inventory_grid() {
        let (registry, book) = tables();
        let mut grid = grid(&registry, 2, &[(0, "core:wood", 1)]);
        let mut inventory = Inventory::new();

        let output = craft_once_from_grid(
            &mut grid,
            &book,
            Station::Inventory,
            &mut inventory,
            &registry,
        )
        .unwrap();

        assert_eq!(registry.name(output.item), "core:planks");
        assert_eq!(output.count, 4);
        assert!(grid.is_empty());
        assert_eq!(inventory.count(registry.id("core:planks").unwrap()), 4);
    }

    #[test]
    fn a_shaped_craft_consumes_one_item_per_slot() {
        let (registry, book) = tables();
        let mut grid = grid(
            &registry,
            2,
            &[(0, "core:planks", 1), (2, "core:planks", 1)],
        );
        let mut inventory = Inventory::new();

        craft_once_from_grid(
            &mut grid,
            &book,
            Station::Inventory,
            &mut inventory,
            &registry,
        )
        .unwrap();

        assert!(grid.is_empty());
        assert_eq!(inventory.count(registry.id("core:stick").unwrap()), 4);
    }

    #[test]
    fn a_full_inventory_rolls_back_the_grid_and_output() {
        let (registry, book) = tables();
        let mut grid = grid(&registry, 2, &[(0, "core:wood", 1)]);
        let mut inventory = Inventory::new();
        for index in 0..TOTAL_SLOTS {
            inventory.set_slot(index, registry.stack("core:dirt", 64));
        }

        let error = craft_once_from_grid(
            &mut grid,
            &book,
            Station::Inventory,
            &mut inventory,
            &registry,
        )
        .unwrap_err();

        assert_eq!(error, CraftError::InventoryFull);
        assert_eq!(registry.name(grid.slot(0).unwrap().item), "core:wood");
        assert_eq!(inventory.count(registry.id("core:wood").unwrap()), 0);
    }

    #[test]
    fn shift_crafting_builds_every_available_recipe() {
        let (registry, book) = tables();
        let mut grid = grid(&registry, 2, &[(0, "core:wood", 3)]);
        let mut inventory = Inventory::new();

        let (crafted, error) = craft_all_from_grid(
            &mut grid,
            &book,
            Station::Inventory,
            &mut inventory,
            &registry,
            10,
        );

        assert_eq!((crafted, error), (3, None));
        assert!(grid.is_empty());
        assert_eq!(inventory.count(registry.id("core:planks").unwrap()), 12);
    }

    #[test]
    fn direct_craft_consumes_inventory_totals() {
        let (registry, book) = tables();
        let recipe = book
            .iter()
            .find(|recipe| recipe.name() == "core:sticks_from_planks")
            .unwrap();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:planks", 2));

        let output = craft_direct(recipe, &mut inventory, &registry).unwrap();

        assert_eq!(registry.name(output.item), "core:stick");
        assert_eq!(inventory.count(registry.id("core:planks").unwrap()), 0);
        assert_eq!(inventory.count(registry.id("core:stick").unwrap()), 4);
    }

    #[test]
    fn direct_craft_rolls_back_when_an_ingredient_is_missing() {
        let (registry, book) = tables();
        let recipe = book
            .iter()
            .find(|recipe| recipe.name() == "core:sticks_from_planks")
            .unwrap();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:planks", 1));

        let error = craft_direct(recipe, &mut inventory, &registry).unwrap_err();

        assert_eq!(error, CraftError::MissingIngredients);
        assert_eq!(inventory.count(registry.id("core:planks").unwrap()), 1);
    }

    #[test]
    fn direct_craft_all_stops_at_the_limit() {
        let (registry, book) = tables();
        let recipe = book
            .iter()
            .find(|recipe| recipe.name() == "core:planks_from_wood")
            .unwrap();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:wood", 4));

        let (crafted, error) = craft_direct_all(recipe, &mut inventory, &registry, 2);

        assert_eq!((crafted, error), (2, None));
        assert_eq!(inventory.count(registry.id("core:wood").unwrap()), 2);
    }

    #[test]
    fn salvage_returns_match_the_static_table() {
        let (registry, _) = tables();
        let planks = registry.id("core:planks").unwrap();

        let wooden = salvage_output(registry.id("core:wooden_pickaxe").unwrap(), &registry);
        assert_eq!(wooden.len(), 2);
        assert_eq!(wooden[0].item, planks);
        assert_eq!(wooden[1].item, registry.id("core:stick").unwrap());

        let workbench = salvage_output(registry.id("core:workbench").unwrap(), &registry);
        assert_eq!(workbench.len(), 1);
        assert_eq!(workbench[0].item, planks);
        assert_eq!(workbench[0].count, 2);

        let torch = salvage_output(registry.id("core:torch").unwrap(), &registry);
        assert_eq!(torch.len(), 1);
        assert_eq!(torch[0].item, registry.id("core:stick").unwrap());
    }

    #[test]
    fn salvage_held_consumes_one_item_and_keeps_the_rest() {
        let (registry, _) = tables();
        let mut inventory = Inventory::new();
        let mut held = registry.stack("core:torch", 2);

        salvage_held(&mut held, &mut inventory, &registry).unwrap();

        assert_eq!(held.unwrap().count, 1);
        assert_eq!(inventory.count(registry.id("core:stick").unwrap()), 1);
    }

    #[test]
    fn salvage_fails_cleanly_when_the_item_has_no_returns() {
        let (registry, _) = tables();
        let mut inventory = Inventory::new();
        let mut held = registry.stack("core:apple", 1);

        let error = salvage_held(&mut held, &mut inventory, &registry).unwrap_err();

        assert_eq!(error, CraftError::NotSalvageable);
        assert_eq!(held.unwrap().count, 1);
    }

    #[test]
    fn the_wood_to_stone_chain_crafts_every_step() {
        let (registry, book) = tables();
        let recipe = |name: &str| {
            book.iter()
                .find(|recipe| recipe.name() == name)
                .unwrap_or_else(|| panic!("missing recipe {name}"))
        };
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:wood", 12));

        let (planks, error) = craft_direct_all(
            recipe("core:planks_from_wood"),
            &mut inventory,
            &registry,
            12,
        );
        assert_eq!((planks, error), (12, None));
        craft_direct(recipe("core:sticks_from_planks"), &mut inventory, &registry).unwrap();
        craft_direct(recipe("core:workbench"), &mut inventory, &registry).unwrap();
        craft_direct(recipe("core:wooden_pickaxe"), &mut inventory, &registry).unwrap();

        assert_eq!(
            inventory.count(registry.id("core:wooden_pickaxe").unwrap()),
            1
        );
        let leftover = inventory.add(registry.stack("core:stone", 3).unwrap(), &registry);
        assert!(
            leftover.is_none(),
            "inventory: {:?}",
            inventory.iter().collect::<Vec<_>>()
        );

        craft_direct(recipe("core:stone_pickaxe"), &mut inventory, &registry).unwrap();

        assert_eq!(
            inventory.count(registry.id("core:stone_pickaxe").unwrap()),
            1
        );
        assert_eq!(inventory.count(registry.id("core:wood").unwrap()), 0);
        assert_eq!(inventory.count(registry.id("core:stone").unwrap()), 0);
        assert_eq!(inventory.count(registry.id("core:stick").unwrap()), 0);
    }
}
