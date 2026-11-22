//! Inventory slots and stack operations.

use std::ops::Range;

use super::item::{ItemId, ItemRegistry, ItemStack};

/// Number of hotbar slots.
pub(crate) const HOTBAR_SLOTS: usize = 9;
/// Number of main inventory slots.
pub(crate) const MAIN_SLOTS: usize = 27;
/// The offhand slot index.
pub(crate) const OFFHAND_SLOT: usize = HOTBAR_SLOTS + MAIN_SLOTS;
/// Total slot count, including the offhand.
pub(crate) const TOTAL_SLOTS: usize = OFFHAND_SLOT + 1;

/// A fixed inventory: nine hotbar slots, twenty-seven main slots, one offhand.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Inventory {
    slots: [Option<ItemStack>; TOTAL_SLOTS],
    selected: usize,
}

impl Default for Inventory {
    fn default() -> Self {
        Self {
            slots: [None; TOTAL_SLOTS],
            selected: 0,
        }
    }
}

#[allow(dead_code, reason = "inventory API used by the 0.0.10.2 UI and tests")]
impl Inventory {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The stack in one slot.
    pub(crate) fn slot(&self, index: usize) -> Option<ItemStack> {
        self.slots.get(index).copied().flatten()
    }

    /// Replaces one slot. Used by save/load and tests.
    pub(crate) fn set_slot(&mut self, index: usize, stack: Option<ItemStack>) {
        if let Some(slot) = self.slots.get_mut(index) {
            *slot = stack;
        }
    }

    /// The selected hotbar index.
    pub(crate) fn selected_index(&self) -> usize {
        self.selected
    }

    /// Selects a hotbar slot, clamped to the hotbar range.
    pub(crate) fn select(&mut self, index: usize) {
        self.selected = index.min(HOTBAR_SLOTS - 1);
    }

    /// The selected stack.
    pub(crate) fn selected(&self) -> Option<ItemStack> {
        self.slot(self.selected)
    }

    /// The selected item id.
    pub(crate) fn selected_item(&self) -> Option<ItemId> {
        self.selected().map(|stack| stack.item)
    }

    /// Number of a given item across all slots.
    pub(crate) fn count(&self, item: ItemId) -> u32 {
        self.slots
            .iter()
            .flatten()
            .filter(|stack| stack.item == item)
            .map(|stack| u32::from(stack.count))
            .sum()
    }

    /// Whether every slot is occupied.
    pub(crate) fn is_full(&self) -> bool {
        self.slots.iter().all(Option::is_some)
    }

    /// Adds a stack, merging into existing stacks first and then filling empty
    /// slots. Returns the part that did not fit.
    pub(crate) fn add(
        &mut self,
        mut stack: ItemStack,
        registry: &ItemRegistry,
    ) -> Option<ItemStack> {
        if stack.count == 0 {
            return None;
        }

        for slot in &mut self.slots {
            let Some(existing) = slot else {
                continue;
            };
            if existing.item != stack.item || existing.durability != stack.durability {
                continue;
            }
            let max = registry.def(existing.item).max_stack;
            let room = max.saturating_sub(existing.count);
            if room == 0 {
                continue;
            }
            let moved = room.min(stack.count);
            existing.count += moved;
            stack.count -= moved;
            if stack.count == 0 {
                return None;
            }
        }

        for slot in &mut self.slots {
            if slot.is_some() {
                continue;
            }
            let max = registry.def(stack.item).max_stack;
            let placed = max.min(stack.count);
            *slot = Some(ItemStack {
                count: placed,
                ..stack
            });
            stack.count -= placed;
            if stack.count == 0 {
                return None;
            }
        }

        Some(stack)
    }

    /// Removes up to `count` from one slot and returns the removed stack.
    pub(crate) fn remove(&mut self, index: usize, count: u16) -> Option<ItemStack> {
        let slot = self.slots.get_mut(index)?;
        let mut stack = (*slot)?;
        if count == 0 {
            return None;
        }
        let removed = count.min(stack.count);
        stack.count = removed;
        let remaining = self.slots[index].as_mut()?;
        remaining.count -= removed;
        if remaining.count == 0 {
            self.slots[index] = None;
        }
        Some(stack)
    }

    /// Removes one item from a slot.
    pub(crate) fn take_one(&mut self, index: usize) -> Option<ItemStack> {
        self.remove(index, 1)
    }

    /// Splits half of a stack into the first empty slot and returns the moved
    /// stack.
    pub(crate) fn split(&mut self, index: usize) -> Option<ItemStack> {
        let stack = self.slot(index)?;
        if stack.count < 2 {
            return None;
        }
        let destination = self.slots.iter().position(Option::is_none)?;
        let moved = stack.count / 2;
        let kept = stack.count - moved;
        self.slots[index] = Some(ItemStack {
            count: kept,
            ..stack
        });
        let split = ItemStack {
            count: moved,
            ..stack
        };
        self.slots[destination] = Some(split);
        Some(split)
    }

    /// Moves or swaps the stack in `from` into `to`.
    ///
    /// Same-item stacks merge up to the destination's limit; the remainder
    /// stays in `from`. Different stacks swap.
    pub(crate) fn move_stack(&mut self, from: usize, to: usize, registry: &ItemRegistry) {
        if from == to || from >= TOTAL_SLOTS || to >= TOTAL_SLOTS {
            return;
        }
        let Some(source) = self.slots[from] else {
            return;
        };
        let target = self.slots[to];
        match target {
            None => {
                self.slots[to] = Some(source);
                self.slots[from] = None;
            }
            Some(mut target)
                if target.item == source.item && target.durability == source.durability =>
            {
                let max = registry.def(target.item).max_stack;
                let room = max.saturating_sub(target.count);
                let moved = room.min(source.count);
                target.count += moved;
                self.slots[to] = Some(target);
                if moved == source.count {
                    self.slots[from] = None;
                } else {
                    self.slots[from] = Some(ItemStack {
                        count: source.count - moved,
                        ..source
                    });
                }
            }
            Some(target) => {
                self.slots[to] = Some(source);
                self.slots[from] = Some(target);
            }
        }
    }

    /// Moves a stack into `range`, merging into existing stacks first and then
    /// filling empty slots. The source must be outside the destination range.
    ///
    /// Returns `true` when the source slot is empty afterwards.
    pub(crate) fn quick_move(
        &mut self,
        index: usize,
        range: Range<usize>,
        registry: &ItemRegistry,
    ) -> bool {
        if range.contains(&index) || index >= TOTAL_SLOTS {
            return false;
        }
        let Some(mut stack) = self.slots[index] else {
            return false;
        };
        let max = registry.def(stack.item).max_stack;

        for slot_index in range.clone() {
            if stack.count == 0 {
                break;
            }
            let Some(existing) = self.slots[slot_index].as_mut() else {
                continue;
            };
            if existing.item != stack.item || existing.durability != stack.durability {
                continue;
            }
            let room = max.saturating_sub(existing.count);
            let moved = room.min(stack.count);
            existing.count += moved;
            stack.count -= moved;
        }

        for slot_index in range {
            if stack.count == 0 {
                break;
            }
            if self.slots[slot_index].is_some() {
                continue;
            }
            let placed = max.min(stack.count);
            self.slots[slot_index] = Some(ItemStack {
                count: placed,
                ..stack
            });
            stack.count -= placed;
        }

        if stack.count == 0 {
            self.slots[index] = None;
            true
        } else {
            self.slots[index] = Some(stack);
            false
        }
    }

    /// Applies durability damage to a slot. Returns whether the item broke.
    pub(crate) fn damage(&mut self, index: usize, amount: u16) -> bool {
        let Some(stack) = self.slots.get_mut(index).and_then(Option::as_mut) else {
            return false;
        };
        if stack.durability == 0 {
            return false;
        }
        stack.durability = stack.durability.saturating_sub(amount);
        if stack.durability == 0 {
            self.slots[index] = None;
            return true;
        }
        false
    }

    /// Iterates occupied slots in slot order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (usize, ItemStack)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.as_ref().map(|stack| (index, *stack)))
    }
}

#[cfg(test)]
mod tests {
    use crate::content;

    use super::super::item::ItemRegistry;
    use super::*;

    fn registry() -> ItemRegistry {
        ItemRegistry::builtin(&content::v1_default())
    }

    #[test]
    fn add_merges_then_fills_empty_slots() {
        let registry = registry();
        let stone = registry.stack("core:stone", 64).unwrap();
        let mut inventory = Inventory::new();

        assert!(inventory.add(stone, &registry).is_none());
        assert_eq!(inventory.slot(0).unwrap().count, 64);

        let leftover = inventory.add(registry.stack("core:stone", 10).unwrap(), &registry);
        assert!(leftover.is_none());
        assert_eq!(inventory.slot(1).unwrap().count, 10);
    }

    #[test]
    fn add_returns_the_part_that_does_not_fit() {
        let registry = registry();
        let mut inventory = Inventory::new();
        for index in 0..TOTAL_SLOTS - 1 {
            inventory.set_slot(index, registry.stack("core:stone", 64));
        }
        inventory.set_slot(OFFHAND_SLOT, registry.stack("core:stone", 60));

        let leftover = inventory
            .add(registry.stack("core:stone", 10).unwrap(), &registry)
            .unwrap();

        assert_eq!(leftover.count, 6);
        assert_eq!(inventory.slot(OFFHAND_SLOT).unwrap().count, 64);
    }

    #[test]
    fn remove_and_take_one_update_counts() {
        let registry = registry();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:stone", 5));

        assert_eq!(inventory.take_one(0).unwrap().count, 1);
        assert_eq!(inventory.slot(0).unwrap().count, 4);
        assert_eq!(inventory.remove(0, 10).unwrap().count, 4);
        assert!(inventory.slot(0).is_none());
    }

    #[test]
    fn split_moves_half_to_the_first_empty_slot() {
        let registry = registry();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:stone", 9));

        let split = inventory.split(0).unwrap();

        assert_eq!(split.count, 4);
        assert_eq!(inventory.slot(0).unwrap().count, 5);
        assert_eq!(inventory.slot(1).unwrap().count, 4);
    }

    #[test]
    fn move_stack_merges_same_items_and_swaps_different_ones() {
        let registry = registry();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:stone", 50));
        inventory.set_slot(1, registry.stack("core:stone", 20));
        inventory.move_stack(0, 1, &registry);
        assert_eq!(inventory.slot(1).unwrap().count, 64);
        assert_eq!(inventory.slot(0).unwrap().count, 6);

        inventory.set_slot(2, registry.stack("core:dirt", 3));
        inventory.set_slot(3, registry.stack("core:stone", 1));
        inventory.move_stack(2, 3, &registry);
        assert_eq!(
            inventory.slot(2).unwrap().item,
            registry.id("core:stone").unwrap()
        );
        assert_eq!(
            inventory.slot(3).unwrap().item,
            registry.id("core:dirt").unwrap()
        );
    }

    #[test]
    fn quick_move_merges_then_fills_the_target_range() {
        let registry = registry();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:stone", 70));
        inventory.set_slot(HOTBAR_SLOTS, registry.stack("core:stone", 10));

        assert!(inventory.quick_move(0, HOTBAR_SLOTS..OFFHAND_SLOT, &registry));

        assert!(inventory.slot(0).is_none());
        assert_eq!(inventory.slot(HOTBAR_SLOTS).unwrap().count, 64);
        assert_eq!(inventory.slot(HOTBAR_SLOTS + 1).unwrap().count, 10);
    }

    #[test]
    fn quick_move_leaves_the_remainder_when_the_range_is_full() {
        let registry = registry();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:stone", 10));
        for index in HOTBAR_SLOTS..OFFHAND_SLOT {
            inventory.set_slot(index, registry.stack("core:dirt", 64));
        }

        assert!(!inventory.quick_move(0, HOTBAR_SLOTS..OFFHAND_SLOT, &registry));
        assert_eq!(inventory.slot(0).unwrap().count, 10);
    }

    #[test]
    fn damage_breaks_a_tool_at_zero() {
        let registry = registry();
        let mut inventory = Inventory::new();
        inventory.set_slot(0, registry.stack("core:wooden_pickaxe", 1));

        assert!(!inventory.damage(0, 59));
        assert_eq!(inventory.slot(0).unwrap().durability, 1);
        assert!(inventory.damage(0, 1));
        assert!(inventory.slot(0).is_none());
    }

    #[test]
    fn selection_is_clamped_to_the_hotbar() {
        let mut inventory = Inventory::new();

        inventory.select(42);

        assert_eq!(inventory.selected_index(), HOTBAR_SLOTS - 1);
    }
}
