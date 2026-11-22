//! Shaped and tagged grid matching, ingredient consumption, and the
//! multiset allocator.

use super::super::crafting::CraftGrid;
use super::super::item::{ItemId, ItemStack};
use super::{
    Bounds, CompiledIngredient, CompiledPattern, CompiledRecipe, MAX_CRAFT_SLOTS, occupied_bounds,
};

impl CompiledRecipe {
    pub(super) fn matches(
        &self,
        slots: &[Option<ItemStack>; MAX_CRAFT_SLOTS],
        size: u8,
    ) -> Option<bool> {
        if size == 0 || size > 3 {
            return None;
        }
        match &self.pattern {
            CompiledPattern::Shaped {
                width,
                height,
                slots: pattern,
            } => matches_shaped(pattern, *width, *height, slots, size),
            CompiledPattern::Tagged(requirements) => {
                let pool = aggregate(slots, size);
                tagged_plan(requirements, &pool).map(|_| false)
            }
        }
    }

    /// Consumes one craft's ingredients from a matched grid.
    ///
    /// The grid must already have matched this recipe. The caller should use a
    /// clone for transactional behavior; a partial failure leaves the grid
    /// partially consumed.
    pub(crate) fn consume_grid(&self, grid: &mut CraftGrid, mirrored: bool) -> bool {
        match &self.pattern {
            CompiledPattern::Shaped {
                width,
                height,
                slots,
            } => {
                let size = grid.size();
                let Some(bounds) = grid_bounds(grid) else {
                    return false;
                };
                if bounds.width != *width || bounds.height != *height {
                    return false;
                }
                for y in 0..*height {
                    for x in 0..*width {
                        let recipe_index = usize::from(y) * usize::from(*width) + usize::from(x);
                        let grid_x = if mirrored { *width - 1 - x } else { x };
                        let grid_index = usize::from(bounds.min_y + y) * usize::from(size)
                            + usize::from(bounds.min_x + grid_x);
                        match &slots[recipe_index] {
                            None => {
                                if grid.slot(grid_index).is_some() {
                                    return false;
                                }
                            }
                            Some(ingredient) => {
                                if grid
                                    .take(grid_index, ingredient.count)
                                    .is_none_or(|stack| stack.count != ingredient.count)
                                {
                                    return false;
                                }
                            }
                        }
                    }
                }
                true
            }
            CompiledPattern::Tagged(requirements) => {
                let pool = aggregate(grid.slots(), grid.size());
                let Some(plan) = tagged_plan(requirements, &pool) else {
                    return false;
                };
                for (item, count) in plan {
                    let mut remaining = count;
                    for index in 0..usize::from(grid.size()) * usize::from(grid.size()) {
                        if remaining == 0 {
                            break;
                        }
                        let Some(stack) = grid.slot(index) else {
                            continue;
                        };
                        if stack.item != item {
                            continue;
                        }
                        let take = 1.min(stack.count);
                        if grid.take(index, take).is_none() {
                            return false;
                        }
                        remaining -= take;
                    }
                    if remaining != 0 {
                        return false;
                    }
                }
                true
            }
        }
    }
}

fn matches_shaped(
    pattern: &[Option<CompiledIngredient>],
    width: u8,
    height: u8,
    slots: &[Option<ItemStack>; MAX_CRAFT_SLOTS],
    size: u8,
) -> Option<bool> {
    let occupied: Vec<_> = slots
        .iter()
        .take(usize::from(size) * usize::from(size))
        .copied()
        .collect();
    let bounds = occupied_bounds(&occupied, size, size)?;
    if bounds.width != width || bounds.height != height {
        return None;
    }
    if matches_at(pattern, width, height, slots, size, bounds, false) {
        return Some(false);
    }
    if matches_at(pattern, width, height, slots, size, bounds, true) {
        return Some(true);
    }
    None
}

fn matches_at(
    pattern: &[Option<CompiledIngredient>],
    width: u8,
    height: u8,
    slots: &[Option<ItemStack>; MAX_CRAFT_SLOTS],
    size: u8,
    bounds: Bounds,
    mirrored: bool,
) -> bool {
    for y in 0..height {
        for x in 0..width {
            let recipe_index = usize::from(y) * usize::from(width) + usize::from(x);
            let grid_x = if mirrored { width - 1 - x } else { x };
            let grid_index = usize::from(bounds.min_y + y) * usize::from(size)
                + usize::from(bounds.min_x + grid_x);
            match (&pattern[recipe_index], slots[grid_index]) {
                (None, None) => {}
                (Some(ingredient), Some(stack))
                    if ingredient.accepts(stack.item) && stack.count >= ingredient.count => {}
                _ => return false,
            }
        }
    }
    true
}

fn grid_bounds(grid: &CraftGrid) -> Option<Bounds> {
    let size = grid.size();
    let mut occupied = Vec::with_capacity(MAX_CRAFT_SLOTS);
    for index in 0..usize::from(size) * usize::from(size) {
        occupied.push(grid.slot(index));
    }
    occupied_bounds(&occupied, size, size)
}

/// Collects the grid's occupied slots into one `(item, count)` entry per item
/// id. Each slot contributes one ingredient; extra items in a stack stay in
/// the slot and can feed later crafts.
fn aggregate(slots: &[Option<ItemStack>; MAX_CRAFT_SLOTS], size: u8) -> Vec<(ItemId, u16)> {
    let mut pool: Vec<(ItemId, u16)> = Vec::new();
    for slot in slots.iter().take(usize::from(size) * usize::from(size)) {
        let Some(stack) = slot else {
            continue;
        };
        if let Some(entry) = pool.iter_mut().find(|(item, _)| *item == stack.item) {
            entry.1 = entry.1.saturating_add(1);
        } else {
            pool.push((stack.item, 1));
        }
    }
    pool
}

/// Allocates each tagged requirement to one item type. Returns the chosen
/// `(item, count)` plan, or `None` when the occupied slots do not match.
fn tagged_plan(
    requirements: &[CompiledIngredient],
    pool: &[(ItemId, u16)],
) -> Option<Vec<(ItemId, u16)>> {
    if requirements.is_empty() {
        return None;
    }
    let total: u32 = pool.iter().map(|(_, count)| u32::from(*count)).sum();
    let needed: u32 = requirements
        .iter()
        .map(|requirement| u32::from(requirement.count))
        .sum();
    if total != needed {
        return None;
    }
    if pool.iter().any(|(item, count)| {
        *count > 0
            && !requirements
                .iter()
                .any(|requirement| requirement.accepts(*item))
    }) {
        return None;
    }
    let mut counts: Vec<u16> = pool.iter().map(|(_, count)| *count).collect();
    let mut plan = Vec::with_capacity(requirements.len());
    if plan_requirements(requirements, pool, &mut counts, 0, &mut plan) {
        Some(plan)
    } else {
        None
    }
}

fn plan_requirements(
    requirements: &[CompiledIngredient],
    pool: &[(ItemId, u16)],
    counts: &mut [u16],
    index: usize,
    plan: &mut Vec<(ItemId, u16)>,
) -> bool {
    if index == requirements.len() {
        return counts.iter().all(|count| *count == 0);
    }
    let requirement = &requirements[index];
    for slot in 0..pool.len() {
        if counts[slot] < requirement.count || !requirement.accepts(pool[slot].0) {
            continue;
        }
        counts[slot] -= requirement.count;
        plan.push((pool[slot].0, requirement.count));
        if plan_requirements(requirements, pool, counts, index + 1, plan) {
            return true;
        }
        plan.pop();
        counts[slot] += requirement.count;
    }
    false
}
