//! The Phase 1 recipe grammar, compiled tables, and shaped/tagged matching.
//!
//! Recipes are authored as stable names, compiled once against the item
//! registry, and then matched by dense ids. The grammar carries every field the
//! later crafting phases need even though Phase 1 uses only T1, instant costs,
//! starting knowledge, and workbench/inventory stations.

use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use super::crafting::CraftGrid;
use super::inventory::Inventory;
use super::item::{ItemId, ItemRegistry, ItemStack};

mod matching;

/// Most slots a crafting grid can have.
pub(crate) const MAX_CRAFT_SLOTS: usize = 9;

/// Where a recipe is made.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Station {
    /// The 2x2 grid in the inventory screen.
    #[default]
    Inventory,
    /// The 3x3 grid opened by a placed workbench.
    Workbench,
}

impl Station {
    /// The square grid edge this station offers.
    pub(crate) const fn grid_size(self) -> u8 {
        match self {
            Self::Inventory => 2,
            Self::Workbench => 3,
        }
    }

    /// The user-facing station name.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Inventory => "Inventory",
            Self::Workbench => "Workbench",
        }
    }

    /// Whether this station can make a recipe whose home station is `recipe`.
    pub(crate) fn can_make(self, recipe: Station) -> bool {
        recipe.grid_size() <= self.grid_size()
    }
}

/// The material tier of a recipe. Phase 1 has only T1.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Tier {
    /// The first material tier.
    #[default]
    T1,
}

impl Tier {
    /// The user-facing tier name.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::T1 => "T1",
        }
    }
}

/// A crafting discipline. Phase 1 stores this as metadata only.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Discipline {
    /// Blocks, stations, and building materials.
    #[default]
    Construction,
    /// Wooden gear and furniture.
    Woodworking,
    /// Stone and metal gear.
    Smithing,
}

impl Discipline {
    /// The user-facing discipline name.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Construction => "Construction",
            Self::Woodworking => "Woodworking",
            Self::Smithing => "Smithing",
        }
    }
}

/// How a recipe is learned. Phase 1 uses starting knowledge only.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Unlock {
    /// Known from the start of a run.
    #[default]
    Start,
}

impl Unlock {
    /// The user-facing unlock name.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
        }
    }
}

/// What a craft costs. Phase 1 is instant and free.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum CraftCost {
    /// No time, fuel, catalyst, or essence.
    #[default]
    Instant,
}

impl CraftCost {
    /// The user-facing cost name.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Instant => "Instant",
        }
    }
}

/// A Phase 1 item tag.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum Tag {
    /// Logs and other raw wood.
    Wood,
    /// Wooden planks.
    Planks,
    /// Stone.
    Stone,
    /// Anything that can fuel a torch or a later furnace.
    Fuel,
}

impl Tag {
    /// Every tag in display order.
    pub(crate) const ALL: [Self; 4] = [Self::Wood, Self::Planks, Self::Stone, Self::Fuel];

    /// The stable item names that carry this tag.
    pub(crate) const fn names(self) -> &'static [&'static str] {
        match self {
            Self::Wood => &["core:wood"],
            Self::Planks => &["core:planks"],
            Self::Stone => &["core:stone"],
            Self::Fuel => &["core:leaves", "core:planks", "core:wood"],
        }
    }

    /// The user-facing tag name.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Wood => "wood",
            Self::Planks => "planks",
            Self::Stone => "stone",
            Self::Fuel => "fuel",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Wood => 0,
            Self::Planks => 1,
            Self::Stone => 2,
            Self::Fuel => 3,
        }
    }
}

/// One ingredient slot or tagged requirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Ingredient {
    /// One exact item.
    Item(&'static str),
    /// Any item carrying a tag.
    Tag(Tag),
}

/// One counted tagged requirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct IngredientCount {
    /// The item or tag.
    pub(crate) ingredient: Ingredient,
    /// How many are needed.
    pub(crate) count: u16,
}

/// A shaped or tagged ingredient pattern.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Pattern {
    /// A rectangle whose occupied cells matter. Empty cells are holes.
    Shaped {
        /// Recipe width before trimming; the compiler trims empty borders.
        width: u8,
        /// Recipe height before trimming.
        height: u8,
        /// Row-major slots.
        slots: &'static [Option<Ingredient>],
    },
    /// A multiset of items and tags whose arrangement does not matter.
    Tagged(&'static [IngredientCount]),
}

/// What a recipe produces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecipeOutput {
    /// Stable item name.
    pub(crate) item: &'static str,
    /// Output count.
    pub(crate) count: u16,
}

/// One authored recipe. Every grammar field is present in Phase 1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Recipe {
    /// Stable recipe name.
    pub(crate) name: &'static str,
    /// Where it is made.
    pub(crate) station: Station,
    /// Minimum tier.
    pub(crate) tier: Tier,
    /// Shaped or tagged.
    pub(crate) pattern: Pattern,
    /// Item and count produced.
    pub(crate) output: RecipeOutput,
    /// Time, fuel, catalysts, and essence.
    pub(crate) cost: CraftCost,
    /// How the recipe is learned.
    pub(crate) unlock: Unlock,
    /// Crafting discipline metadata.
    pub(crate) discipline: Discipline,
}

const PLANK: Option<Ingredient> = Some(Ingredient::Item("core:planks"));
const STICK: Option<Ingredient> = Some(Ingredient::Item("core:stick"));
const STONE: Option<Ingredient> = Some(Ingredient::Item("core:stone"));
const HOLE: Option<Ingredient> = None;

const PICKAXE: [Option<Ingredient>; 9] =
    [PLANK, PLANK, PLANK, HOLE, STICK, HOLE, HOLE, STICK, HOLE];
const AXE: [Option<Ingredient>; 9] = [PLANK, PLANK, HOLE, PLANK, STICK, HOLE, HOLE, STICK, HOLE];
const SHOVEL: [Option<Ingredient>; 3] = [PLANK, STICK, STICK];
const SWORD: [Option<Ingredient>; 3] = [PLANK, PLANK, STICK];

const STONE_PICKAXE: [Option<Ingredient>; 9] =
    [STONE, STONE, STONE, HOLE, STICK, HOLE, HOLE, STICK, HOLE];
const STONE_AXE: [Option<Ingredient>; 9] =
    [STONE, STONE, HOLE, STONE, STICK, HOLE, HOLE, STICK, HOLE];
const STONE_SHOVEL: [Option<Ingredient>; 3] = [STONE, STICK, STICK];
const STONE_SWORD: [Option<Ingredient>; 3] = [STONE, STONE, STICK];

const WOOD_REQUIREMENT: [IngredientCount; 1] = [IngredientCount {
    ingredient: Ingredient::Tag(Tag::Wood),
    count: 1,
}];
const TORCH_REQUIREMENTS: [IngredientCount; 2] = [
    IngredientCount {
        ingredient: Ingredient::Item("core:stick"),
        count: 1,
    },
    IngredientCount {
        ingredient: Ingredient::Tag(Tag::Fuel),
        count: 1,
    },
];
const STICKS_PATTERN: [Option<Ingredient>; 2] = [PLANK, PLANK];
const WORKBENCH_PATTERN: [Option<Ingredient>; 4] = [PLANK, PLANK, PLANK, PLANK];

const fn recipe(
    name: &'static str,
    station: Station,
    pattern: Pattern,
    item: &'static str,
    count: u16,
    discipline: Discipline,
) -> Recipe {
    Recipe {
        name,
        station,
        tier: Tier::T1,
        pattern,
        output: RecipeOutput { item, count },
        cost: CraftCost::Instant,
        unlock: Unlock::Start,
        discipline,
    }
}

/// The twelve Phase 1 recipes, in deterministic first-match order.
pub(crate) const RECIPES: &[Recipe] = &[
    recipe(
        "core:planks_from_wood",
        Station::Inventory,
        Pattern::Tagged(&WOOD_REQUIREMENT),
        "core:planks",
        4,
        Discipline::Construction,
    ),
    recipe(
        "core:sticks_from_planks",
        Station::Inventory,
        Pattern::Shaped {
            width: 1,
            height: 2,
            slots: &STICKS_PATTERN,
        },
        "core:stick",
        4,
        Discipline::Construction,
    ),
    recipe(
        "core:workbench",
        Station::Inventory,
        Pattern::Shaped {
            width: 2,
            height: 2,
            slots: &WORKBENCH_PATTERN,
        },
        "core:workbench",
        1,
        Discipline::Construction,
    ),
    recipe(
        "core:torch",
        Station::Inventory,
        Pattern::Tagged(&TORCH_REQUIREMENTS),
        "core:torch",
        4,
        Discipline::Construction,
    ),
    recipe(
        "core:wooden_pickaxe",
        Station::Workbench,
        Pattern::Shaped {
            width: 3,
            height: 3,
            slots: &PICKAXE,
        },
        "core:wooden_pickaxe",
        1,
        Discipline::Woodworking,
    ),
    recipe(
        "core:wooden_axe",
        Station::Workbench,
        Pattern::Shaped {
            width: 3,
            height: 3,
            slots: &AXE,
        },
        "core:wooden_axe",
        1,
        Discipline::Woodworking,
    ),
    recipe(
        "core:wooden_shovel",
        Station::Workbench,
        Pattern::Shaped {
            width: 1,
            height: 3,
            slots: &SHOVEL,
        },
        "core:wooden_shovel",
        1,
        Discipline::Woodworking,
    ),
    recipe(
        "core:wooden_sword",
        Station::Workbench,
        Pattern::Shaped {
            width: 1,
            height: 3,
            slots: &SWORD,
        },
        "core:wooden_sword",
        1,
        Discipline::Woodworking,
    ),
    recipe(
        "core:stone_pickaxe",
        Station::Workbench,
        Pattern::Shaped {
            width: 3,
            height: 3,
            slots: &STONE_PICKAXE,
        },
        "core:stone_pickaxe",
        1,
        Discipline::Smithing,
    ),
    recipe(
        "core:stone_axe",
        Station::Workbench,
        Pattern::Shaped {
            width: 3,
            height: 3,
            slots: &STONE_AXE,
        },
        "core:stone_axe",
        1,
        Discipline::Smithing,
    ),
    recipe(
        "core:stone_shovel",
        Station::Workbench,
        Pattern::Shaped {
            width: 1,
            height: 3,
            slots: &STONE_SHOVEL,
        },
        "core:stone_shovel",
        1,
        Discipline::Smithing,
    ),
    recipe(
        "core:stone_sword",
        Station::Workbench,
        Pattern::Shaped {
            width: 1,
            height: 3,
            slots: &STONE_SWORD,
        },
        "core:stone_sword",
        1,
        Discipline::Smithing,
    ),
];

/// A compiled ingredient: the dense item ids that satisfy it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CompiledIngredient {
    /// Item ids accepted by this ingredient.
    pub(crate) items: Vec<ItemId>,
    /// How many are needed.
    pub(crate) count: u16,
    /// The tag this came from, for display and aggregation.
    pub(crate) tag: Option<Tag>,
}

impl CompiledIngredient {
    /// Whether one item can satisfy this ingredient.
    pub(crate) fn accepts(&self, item: ItemId) -> bool {
        self.items.contains(&item)
    }
}

/// A compiled recipe pattern.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CompiledPattern {
    /// A trimmed shaped rectangle.
    Shaped {
        /// Trimmed width.
        width: u8,
        /// Trimmed height.
        height: u8,
        /// Row-major trimmed slots.
        slots: Vec<Option<CompiledIngredient>>,
    },
    /// A tagged multiset.
    Tagged(Vec<CompiledIngredient>),
}

/// A recipe resolved against the item and tag tables.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CompiledRecipe {
    /// The authored fields.
    pub(crate) recipe: &'static Recipe,
    /// The resolved output id.
    pub(crate) output: ItemId,
    /// The output count.
    pub(crate) output_count: u16,
    /// The resolved pattern.
    pub(crate) pattern: CompiledPattern,
}

/// A recipe that matched a grid, plus the orientation that matched.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RecipeMatch<'a> {
    /// The matched recipe.
    pub(crate) recipe: &'a CompiledRecipe,
    /// Whether the shaped pattern matched horizontally mirrored.
    pub(crate) mirrored: bool,
}

/// A validation failure while compiling the built-in recipe table.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub(crate) enum RecipeError {
    #[error("duplicate recipe name: {0}")]
    DuplicateName(String),
    #[error("recipe {0} duplicates another recipe's pattern")]
    DuplicatePattern(String),
    #[error("recipe {recipe} names an unknown item: {item}")]
    UnknownItem { recipe: String, item: String },
    #[error("tag {0} resolves to no items")]
    EmptyTag(&'static str),
    #[error("recipe {0} has an empty pattern")]
    EmptyPattern(String),
    #[error("recipe {recipe} has {slots} slots for a {width}x{height} pattern")]
    SlotCount {
        recipe: String,
        width: u8,
        height: u8,
        slots: usize,
    },
    #[error("recipe {recipe} is {width}x{height}, larger than its {station} station")]
    PatternTooLarge {
        recipe: String,
        width: u8,
        height: u8,
        station: &'static str,
    },
    #[error("recipe {recipe} has a zero count")]
    ZeroCount { recipe: String },
    #[error("recipe {recipe} produces {count} items but the stack limit is {maximum}")]
    OutputOverflow {
        recipe: String,
        count: u16,
        maximum: u16,
    },
    #[error("recipe {recipe} needs {count} ingredients for a {station} grid")]
    TooManyIngredients {
        recipe: String,
        count: u16,
        station: &'static str,
    },
}

/// The compiled tag membership table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TagTable {
    items: [Vec<ItemId>; 4],
}

impl TagTable {
    fn builtin(registry: &ItemRegistry) -> Result<Self, RecipeError> {
        let mut items: [Vec<ItemId>; 4] = std::array::from_fn(|_| Vec::new());
        for tag in Tag::ALL {
            for name in tag.names() {
                let id = registry.id(name).ok_or_else(|| RecipeError::UnknownItem {
                    recipe: format!("tag {}", tag.label()),
                    item: (*name).to_owned(),
                })?;
                items[tag.index()].push(id);
            }
            if items[tag.index()].is_empty() {
                return Err(RecipeError::EmptyTag(tag.label()));
            }
        }
        Ok(Self { items })
    }

    /// The items carrying a tag.
    pub(crate) fn get(&self, tag: Tag) -> &[ItemId] {
        &self.items[tag.index()]
    }
}

/// The compiled recipe book. Every Phase 1 recipe is known at start.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecipeBook {
    recipes: Vec<CompiledRecipe>,
}

impl RecipeBook {
    /// Compiles and validates the built-in recipe table.
    pub(crate) fn builtin(registry: &ItemRegistry) -> Result<Self, RecipeError> {
        let tags = TagTable::builtin(registry)?;
        let mut names = BTreeSet::new();
        let mut patterns = BTreeSet::new();
        let mut recipes = Vec::with_capacity(RECIPES.len());
        for raw in RECIPES {
            if !names.insert(raw.name) {
                return Err(RecipeError::DuplicateName(raw.name.to_owned()));
            }
            if raw.output.count == 0 {
                return Err(RecipeError::ZeroCount {
                    recipe: raw.name.to_owned(),
                });
            }
            let output = registry
                .id(raw.output.item)
                .ok_or_else(|| RecipeError::UnknownItem {
                    recipe: raw.name.to_owned(),
                    item: raw.output.item.to_owned(),
                })?;
            let maximum = registry.def(output).max_stack;
            if raw.output.count > maximum {
                return Err(RecipeError::OutputOverflow {
                    recipe: raw.name.to_owned(),
                    count: raw.output.count,
                    maximum,
                });
            }
            let pattern = compile_pattern(raw, registry, &tags)?;
            if !patterns.insert(pattern.signature()) {
                return Err(RecipeError::DuplicatePattern(raw.name.to_owned()));
            }
            recipes.push(CompiledRecipe {
                recipe: raw,
                output,
                output_count: raw.output.count,
                pattern,
            });
        }
        Ok(Self { recipes })
    }

    /// Every recipe in deterministic order.
    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &CompiledRecipe> {
        self.recipes.iter()
    }

    /// A recipe by index.
    pub(crate) fn get(&self, index: usize) -> Option<&CompiledRecipe> {
        self.recipes.get(index)
    }

    /// The number of recipes.
    pub(crate) fn len(&self) -> usize {
        self.recipes.len()
    }

    /// Recipe indices whose name or output matches a case-insensitive query.
    pub(crate) fn search(&self, query: &str, registry: &ItemRegistry) -> Vec<usize> {
        let query = query.trim().to_lowercase();
        self.recipes
            .iter()
            .enumerate()
            .filter(|(_, recipe)| {
                query.is_empty()
                    || recipe.recipe.name.to_lowercase().contains(&query)
                    || registry.name(recipe.output).to_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// The first recipe that matches the grid at a station compatible with it.
    pub(crate) fn find(&self, grid: &CraftGrid, station: Station) -> Option<RecipeMatch<'_>> {
        let slots = grid.slots();
        let size = grid.size();
        self.recipes.iter().find_map(|recipe| {
            if !station.can_make(recipe.recipe.station) {
                return None;
            }
            recipe
                .matches(slots, size)
                .map(|mirrored| RecipeMatch { recipe, mirrored })
        })
    }
}

impl CompiledPattern {
    fn signature(&self) -> String {
        match self {
            Self::Shaped {
                width,
                height,
                slots,
            } => {
                let mut signature = format!("shaped:{width}x{height}");
                for slot in slots {
                    signature.push('|');
                    match slot {
                        None => signature.push('-'),
                        Some(ingredient) => signature.push_str(&ingredient.signature()),
                    }
                }
                signature
            }
            Self::Tagged(requirements) => {
                let mut parts: Vec<String> = requirements
                    .iter()
                    .map(CompiledIngredient::signature)
                    .collect();
                parts.sort();
                format!("tagged:{}", parts.join("|"))
            }
        }
    }
}

impl CompiledIngredient {
    fn signature(&self) -> String {
        if let Some(tag) = self.tag {
            return format!("tag:{}:{}", tag.label(), self.count);
        }
        let mut items = self.items.clone();
        items.sort_unstable();
        format!("items:{items:?}:{}", self.count)
    }
}

fn compile_pattern(
    recipe: &Recipe,
    registry: &ItemRegistry,
    tags: &TagTable,
) -> Result<CompiledPattern, RecipeError> {
    match recipe.pattern {
        Pattern::Shaped {
            width,
            height,
            slots,
        } => compile_shaped(recipe, registry, tags, width, height, slots),
        Pattern::Tagged(requirements) => {
            if requirements.is_empty() {
                return Err(RecipeError::EmptyPattern(recipe.name.to_owned()));
            }
            let grid_slots = u16::from(recipe.station.grid_size()).pow(2);
            let mut total = 0_u16;
            let mut compiled = Vec::with_capacity(requirements.len());
            for requirement in requirements {
                if requirement.count == 0 {
                    return Err(RecipeError::ZeroCount {
                        recipe: recipe.name.to_owned(),
                    });
                }
                total = total.saturating_add(requirement.count);
                compiled.push(compile_ingredient(
                    recipe,
                    registry,
                    tags,
                    requirement.ingredient,
                    requirement.count,
                )?);
            }
            if total > grid_slots {
                return Err(RecipeError::TooManyIngredients {
                    recipe: recipe.name.to_owned(),
                    count: total,
                    station: recipe.station.label(),
                });
            }
            Ok(CompiledPattern::Tagged(compiled))
        }
    }
}

fn compile_shaped(
    recipe: &Recipe,
    registry: &ItemRegistry,
    tags: &TagTable,
    width: u8,
    height: u8,
    slots: &'static [Option<Ingredient>],
) -> Result<CompiledPattern, RecipeError> {
    if width == 0 || height == 0 {
        return Err(RecipeError::EmptyPattern(recipe.name.to_owned()));
    }
    if slots.len() != usize::from(width) * usize::from(height) {
        return Err(RecipeError::SlotCount {
            recipe: recipe.name.to_owned(),
            width,
            height,
            slots: slots.len(),
        });
    }
    let mut compiled = Vec::with_capacity(slots.len());
    for slot in slots {
        compiled.push(match slot {
            Some(ingredient) => Some(compile_ingredient(recipe, registry, tags, *ingredient, 1)?),
            None => None,
        });
    }
    let Some(bounds) = occupied_bounds(&compiled, width, height) else {
        return Err(RecipeError::EmptyPattern(recipe.name.to_owned()));
    };
    if bounds.width > recipe.station.grid_size() || bounds.height > recipe.station.grid_size() {
        return Err(RecipeError::PatternTooLarge {
            recipe: recipe.name.to_owned(),
            width: bounds.width,
            height: bounds.height,
            station: recipe.station.label(),
        });
    }
    let mut trimmed = Vec::with_capacity(usize::from(bounds.width) * usize::from(bounds.height));
    for y in 0..bounds.height {
        for x in 0..bounds.width {
            let source =
                usize::from(bounds.min_y + y) * usize::from(width) + usize::from(bounds.min_x + x);
            trimmed.push(compiled[source].clone());
        }
    }
    Ok(CompiledPattern::Shaped {
        width: bounds.width,
        height: bounds.height,
        slots: trimmed,
    })
}

fn compile_ingredient(
    recipe: &Recipe,
    registry: &ItemRegistry,
    tags: &TagTable,
    ingredient: Ingredient,
    count: u16,
) -> Result<CompiledIngredient, RecipeError> {
    let (items, tag) = match ingredient {
        Ingredient::Item(name) => {
            let id = registry.id(name).ok_or_else(|| RecipeError::UnknownItem {
                recipe: recipe.name.to_owned(),
                item: name.to_owned(),
            })?;
            (vec![id], None)
        }
        Ingredient::Tag(tag) => (tags.get(tag).to_vec(), Some(tag)),
    };
    if items.is_empty() {
        return Err(RecipeError::EmptyTag(tag.map_or("ingredient", Tag::label)));
    }
    Ok(CompiledIngredient { items, count, tag })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Bounds {
    pub(super) min_x: u8,
    pub(super) min_y: u8,
    pub(super) width: u8,
    pub(super) height: u8,
}

pub(super) fn occupied_bounds<T>(slots: &[Option<T>], width: u8, height: u8) -> Option<Bounds> {
    let mut min_x = u8::MAX;
    let mut min_y = u8::MAX;
    let mut max_x = 0_u8;
    let mut max_y = 0_u8;
    let mut any = false;
    for y in 0..height {
        for x in 0..width {
            let index = usize::from(y) * usize::from(width) + usize::from(x);
            if slots.get(index).is_some_and(Option::is_some) {
                any = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    any.then(|| Bounds {
        min_x,
        min_y,
        width: max_x - min_x + 1,
        height: max_y - min_y + 1,
    })
}

impl CompiledRecipe {
    /// The authored recipe name.
    #[cfg(test)]
    pub(crate) fn name(&self) -> &'static str {
        self.recipe.name
    }

    /// The output stack at full durability.
    pub(crate) fn output_stack(&self, registry: &ItemRegistry) -> Option<ItemStack> {
        registry.stack(registry.name(self.output), self.output_count)
    }

    /// The compiled ingredients, flattened for display and direct crafting.
    pub(crate) fn requirements(&self) -> Vec<&CompiledIngredient> {
        match &self.pattern {
            CompiledPattern::Shaped { slots, .. } => slots.iter().flatten().collect(),
            CompiledPattern::Tagged(requirements) => requirements.iter().collect(),
        }
    }

    /// Whether the current inventory has every ingredient.
    pub(crate) fn craftable_from_inventory(
        &self,
        inventory: &Inventory,
        registry: &ItemRegistry,
    ) -> bool {
        self.ingredient_status(inventory, registry)
            .iter()
            .all(|status| status.have >= status.need)
    }

    /// Ingredient totals grouped by item or tag.
    pub(crate) fn ingredient_status(
        &self,
        inventory: &Inventory,
        registry: &ItemRegistry,
    ) -> Vec<IngredientStatus> {
        let mut grouped: BTreeMap<String, (u16, u16)> = BTreeMap::new();
        for ingredient in self.requirements() {
            let label = ingredient.label(registry);
            let have = if ingredient.tag.is_some() {
                count_to_u16(
                    ingredient
                        .items
                        .iter()
                        .map(|item| inventory.count(*item))
                        .sum(),
                )
            } else {
                count_to_u16(
                    ingredient
                        .items
                        .first()
                        .map_or(0, |item| inventory.count(*item)),
                )
            };
            let entry = grouped.entry(label).or_insert((0, 0));
            entry.0 = entry.0.max(have);
            entry.1 = entry.1.saturating_add(ingredient.count);
        }
        grouped
            .into_iter()
            .map(|(label, (have, need))| IngredientStatus { label, have, need })
            .collect()
    }
}

impl CompiledIngredient {
    /// The display label for this ingredient.
    pub(crate) fn label(&self, registry: &ItemRegistry) -> String {
        if let Some(tag) = self.tag {
            return tag.label().to_owned();
        }
        self.items.first().map_or_else(
            || "unknown".to_owned(),
            |item| display_name(registry.name(*item)),
        )
    }
}

/// One ingredient line in the recipe details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IngredientStatus {
    /// Item or tag label.
    pub(crate) label: String,
    /// How many the inventory has.
    pub(crate) have: u16,
    /// How many the recipe needs.
    pub(crate) need: u16,
}

/// The user-facing form of a stable item name.
pub(crate) fn display_name(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).replace('_', " ")
}

fn count_to_u16(count: u32) -> u16 {
    u16::try_from(count).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests;
