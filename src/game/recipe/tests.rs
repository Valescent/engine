use crate::content;

use super::super::crafting::CraftGrid;
use super::*;

fn tables() -> (ItemRegistry, RecipeBook) {
    let registry = ItemRegistry::builtin(&content::v1_default());
    let book = RecipeBook::builtin(&registry).expect("the built-in recipes are valid");
    (registry, book)
}

fn grid(size: u8, items: &[(usize, &str, u16)]) -> (ItemRegistry, CraftGrid) {
    let registry = ItemRegistry::builtin(&content::v1_default());
    let mut grid = CraftGrid::new(size);
    for &(index, name, count) in items {
        grid.set_slot(index, registry.stack(name, count));
    }
    (registry, grid)
}

fn found<'a>(
    book: &'a RecipeBook,
    grid: &CraftGrid,
    station: Station,
) -> Option<&'a CompiledRecipe> {
    book.find(grid, station).map(|matched| matched.recipe)
}

#[test]
fn the_builtin_table_validates_and_has_twelve_recipes() {
    let (_, book) = tables();
    assert_eq!(book.len(), 12);
    assert_eq!(RECIPES.len(), 12);
}

#[test]
fn duplicate_recipe_names_are_rejected() {
    let registry = ItemRegistry::builtin(&content::v1_default());
    let mut names = BTreeSet::new();
    for recipe in RECIPES {
        assert!(names.insert(recipe.name), "duplicate {}", recipe.name);
    }
    assert!(RecipeBook::builtin(&registry).is_ok());
}

#[test]
fn planks_use_a_tagged_wood_requirement() {
    let (registry, grid) = grid(2, &[(0, "core:wood", 1)]);
    let (_, book) = tables();
    let recipe = found(&book, &grid, Station::Inventory).expect("wood makes planks");
    assert_eq!(recipe.name(), "core:planks_from_wood");
    assert_eq!(recipe.output_count, 4);
    assert_eq!(recipe.output, registry.id("core:planks").unwrap());
}

#[test]
fn sticks_need_two_planks_in_a_vertical_shaped_pattern() {
    let (_, book) = tables();
    let (_, vertical) = grid(2, &[(0, "core:planks", 1), (2, "core:planks", 1)]);
    assert_eq!(
        found(&book, &vertical, Station::Inventory).map(CompiledRecipe::name),
        Some("core:sticks_from_planks")
    );

    let (_, horizontal) = grid(2, &[(0, "core:planks", 1), (1, "core:planks", 1)]);
    assert!(found(&book, &horizontal, Station::Inventory).is_none());
}

#[test]
fn a_three_by_three_recipe_is_rejected_by_the_inventory_grid() {
    let (_, book) = tables();
    let (_, grid) = grid(
        3,
        &[
            (0, "core:planks", 1),
            (1, "core:planks", 1),
            (2, "core:planks", 1),
            (4, "core:stick", 1),
            (7, "core:stick", 1),
        ],
    );
    assert!(found(&book, &grid, Station::Inventory).is_none());
    assert_eq!(
        found(&book, &grid, Station::Workbench).map(CompiledRecipe::name),
        Some("core:wooden_pickaxe")
    );
}

#[test]
fn the_workbench_can_make_inventory_recipes() {
    let (_, book) = tables();
    let (_, grid) = grid(3, &[(4, "core:wood", 1)]);
    assert_eq!(
        found(&book, &grid, Station::Workbench).map(CompiledRecipe::name),
        Some("core:planks_from_wood")
    );
}

#[test]
fn shaped_matching_trims_the_grid_and_accepts_offsets() {
    let (_, book) = tables();
    let (_, grid) = grid(
        3,
        &[
            (1, "core:planks", 1),
            (4, "core:stick", 1),
            (7, "core:stick", 1),
        ],
    );
    assert_eq!(
        found(&book, &grid, Station::Workbench).map(CompiledRecipe::name),
        Some("core:wooden_shovel")
    );
}

#[test]
fn a_mirrored_axe_pattern_matches() {
    let (_, book) = tables();
    let (_, grid) = grid(
        3,
        &[
            (1, "core:planks", 1),
            (2, "core:planks", 1),
            (4, "core:stick", 1),
            (5, "core:planks", 1),
            (7, "core:stick", 1),
        ],
    );
    let matched = book
        .find(&grid, Station::Workbench)
        .expect("mirrored axe matches");
    assert_eq!(matched.recipe.name(), "core:wooden_axe");
    assert!(matched.mirrored);
}

#[test]
fn tagged_matching_is_a_multiset_and_rejects_extras() {
    let (_, book) = tables();
    let (_, matched_grid) = grid(2, &[(0, "core:stick", 1), (1, "core:leaves", 1)]);
    assert_eq!(
        found(&book, &matched_grid, Station::Inventory).map(CompiledRecipe::name),
        Some("core:torch")
    );

    let (_, extra) = grid(
        2,
        &[
            (0, "core:stick", 1),
            (1, "core:leaves", 1),
            (2, "core:stone", 1),
        ],
    );
    assert!(found(&book, &extra, Station::Inventory).is_none());
}

#[test]
fn tag_alternatives_are_accepted() {
    let (_, book) = tables();
    for fuel in ["core:leaves", "core:planks", "core:wood"] {
        let (_, grid) = grid(2, &[(0, "core:stick", 1), (1, fuel, 1)]);
        assert_eq!(
            found(&book, &grid, Station::Inventory).map(CompiledRecipe::name),
            Some("core:torch"),
            "{fuel} should satisfy the fuel tag"
        );
    }
}

#[test]
fn first_match_order_is_deterministic() {
    let (_, book) = tables();
    let (_, grid) = grid(2, &[(0, "core:wood", 1)]);
    let first = found(&book, &grid, Station::Inventory).map(CompiledRecipe::name);
    for _ in 0..10 {
        assert_eq!(
            found(&book, &grid, Station::Inventory).map(CompiledRecipe::name),
            first
        );
    }
}

#[test]
fn every_output_resolves_and_fits_a_stack() {
    let (registry, book) = tables();
    for recipe in book.iter() {
        assert!(recipe.output_stack(&registry).is_some());
        assert!(recipe.output_count <= registry.def(recipe.output).max_stack);
    }
}

#[test]
fn search_matches_recipe_and_output_names() {
    let (registry, book) = tables();
    assert_eq!(book.search("pickaxe", &registry).len(), 2);
    assert_eq!(book.search("planks", &registry).len(), 2);
    assert_eq!(book.search("torch", &registry).len(), 1);
    assert_eq!(book.search("", &registry).len(), 12);
}

#[test]
fn ingredient_status_aggregates_repeated_slots() {
    let (registry, book) = tables();
    let recipe = book
        .iter()
        .find(|recipe| recipe.name() == "core:wooden_pickaxe")
        .unwrap();
    let mut inventory = Inventory::new();
    inventory.set_slot(0, registry.stack("core:planks", 1));
    inventory.set_slot(1, registry.stack("core:stick", 2));

    let status = recipe.ingredient_status(&inventory, &registry);
    assert!(
        status
            .iter()
            .any(|line| line.label == "planks" && line.need == 3 && line.have == 1)
    );
    assert!(
        status
            .iter()
            .any(|line| line.label == "stick" && line.need == 2 && line.have == 2)
    );
    assert!(!recipe.craftable_from_inventory(&inventory, &registry));
}
