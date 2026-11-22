//! Item identity, definitions, and stacks.
//!
//! Stable `namespace:name` strings are the on-disk authority. Numeric ids are
//! derived by sorting names at load, following the content-pack rule
//! (`A5.1`): adding an item changes ids but never invalidates a save.

use std::collections::BTreeMap;

use crate::content;

use super::tools::ToolClass;

/// A stable item name, interned to a dense index at load.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct ItemId(u16);

impl ItemId {
    const fn index(self) -> usize {
        self.0 as usize
    }
}

/// What an item is used for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ItemKind {
    /// A block that can be placed.
    Block { block: content::BlockId },
    /// A tool with a class, tier, and maximum durability.
    Tool {
        class: ToolClass,
        tier: u8,
        durability: u16,
    },
    /// A crafting material with no direct use yet.
    Material,
    /// A food item with a nutrition value.
    Food { nutrition: u8 },
}

/// One item definition.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ItemDef {
    pub(crate) name: String,
    pub(crate) max_stack: u16,
    pub(crate) kind: ItemKind,
}

impl ItemDef {
    /// The tool view of this definition, or `None` for a non-tool.
    pub(crate) fn tool(&self) -> Option<super::tools::Tool> {
        match self.kind {
            ItemKind::Tool { class, tier, .. } => Some(super::tools::Tool { class, tier }),
            _ => None,
        }
    }
}

/// A stack in an inventory slot or on the ground.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ItemStack {
    pub(crate) item: ItemId,
    pub(crate) count: u16,
    /// Remaining durability for tools; zero for everything else.
    pub(crate) durability: u16,
}

impl ItemStack {
    pub(crate) const fn new(item: ItemId, count: u16, durability: u16) -> Self {
        Self {
            item,
            count,
            durability,
        }
    }
}

/// The built-in item table.
#[derive(Clone, Debug)]
pub(crate) struct ItemRegistry {
    defs: Vec<ItemDef>,
    ids: BTreeMap<String, ItemId>,
}

/// Built-in tools: name, class, tier, durability.
const TOOLS: &[(&str, ToolClass, u8, u16)] = &[
    ("core:iron_pickaxe", ToolClass::Pickaxe, 3, 251),
    ("core:stone_axe", ToolClass::Axe, 2, 132),
    ("core:stone_pickaxe", ToolClass::Pickaxe, 2, 132),
    ("core:stone_shovel", ToolClass::Shovel, 2, 132),
    ("core:stone_sword", ToolClass::Sword, 2, 132),
    ("core:wooden_axe", ToolClass::Axe, 1, 60),
    ("core:wooden_pickaxe", ToolClass::Pickaxe, 1, 60),
    ("core:wooden_shovel", ToolClass::Shovel, 1, 60),
    ("core:wooden_sword", ToolClass::Sword, 1, 60),
];

/// Built-in non-block items: name, max stack, kind.
const MATERIALS: &[(&str, u16, ItemKind)] = &[
    ("core:apple", 64, ItemKind::Food { nutrition: 4 }),
    ("core:stick", 64, ItemKind::Material),
];

impl ItemRegistry {
    /// Builds the item table from a content pack plus the built-in tools and
    /// materials.
    ///
    /// Every non-air, non-liquid block becomes a placeable item. The pack is
    /// the authority for which blocks exist; this table adds only the items
    /// that have no block record yet.
    pub(crate) fn builtin(pack: &content::ContentPack) -> Self {
        let mut defs = Vec::new();
        for record in pack.records() {
            if record.name == "core:air" || record.liquid {
                continue;
            }
            let Some(block) = pack.id(&record.name) else {
                continue;
            };
            defs.push(ItemDef {
                name: record.name.clone(),
                max_stack: 64,
                kind: ItemKind::Block { block },
            });
        }
        for &(name, class, tier, durability) in TOOLS {
            defs.push(ItemDef {
                name: name.to_owned(),
                max_stack: 1,
                kind: ItemKind::Tool {
                    class,
                    tier,
                    durability,
                },
            });
        }
        for &(name, max_stack, kind) in MATERIALS {
            defs.push(ItemDef {
                name: name.to_owned(),
                max_stack,
                kind,
            });
        }

        defs.sort_by(|left, right| left.name.cmp(&right.name));
        let mut ids = BTreeMap::new();
        for (index, def) in defs.iter().enumerate() {
            let id = u16::try_from(index).expect("the built-in item table fits in u16");
            ids.insert(def.name.clone(), ItemId(id));
        }
        Self { defs, ids }
    }

    /// Resolves a stable name to an id.
    pub(crate) fn id(&self, name: &str) -> Option<ItemId> {
        self.ids.get(name).copied()
    }

    /// The definition for an id.
    pub(crate) fn def(&self, id: ItemId) -> &ItemDef {
        &self.defs[id.index()]
    }

    /// The stable name for an id.
    pub(crate) fn name(&self, id: ItemId) -> &str {
        &self.defs[id.index()].name
    }

    /// The tool view for an id, or `None` for a non-tool.
    pub(crate) fn tool(&self, id: ItemId) -> Option<super::tools::Tool> {
        self.def(id).tool()
    }

    /// Creates a full-durability stack for a name.
    pub(crate) fn stack(&self, name: &str, count: u16) -> Option<ItemStack> {
        let id = self.id(name)?;
        let def = self.def(id);
        let durability = match def.kind {
            ItemKind::Tool { durability, .. } => durability,
            _ => 0,
        };
        Some(ItemStack::new(
            id,
            count.min(def.max_stack).max(1),
            durability,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> ItemRegistry {
        ItemRegistry::builtin(&content::v1_default())
    }

    #[test]
    fn ids_are_derived_from_sorted_names() {
        let registry = registry();
        let mut names: Vec<&str> = registry
            .defs
            .iter()
            .map(|definition| definition.name.as_str())
            .collect();
        let sorted = names.clone();
        names.sort_unstable();
        assert_eq!(names, sorted);

        for (index, definition) in registry.defs.iter().enumerate() {
            let id = registry
                .id(&definition.name)
                .expect("every definition has an id");
            assert_eq!(id.index(), index);
            assert_eq!(registry.name(id), definition.name);
        }
    }

    #[test]
    fn every_placeable_block_has_an_item() {
        let pack = content::v1_default();
        let registry = ItemRegistry::builtin(&pack);

        for record in pack.records() {
            if record.name == "core:air" || record.liquid {
                continue;
            }
            let id = registry.id(&record.name).expect("block item exists");
            assert!(matches!(registry.def(id).kind, ItemKind::Block { .. }));
        }
    }

    #[test]
    fn tools_start_at_full_durability_and_do_not_stack() {
        let registry = registry();
        let stack = registry.stack("core:wooden_pickaxe", 9).unwrap();

        assert_eq!(stack.count, 1);
        assert_eq!(stack.durability, 60);
        assert_eq!(
            registry.tool(stack.item),
            Some(super::super::tools::Tool {
                class: ToolClass::Pickaxe,
                tier: 1,
            })
        );
    }

    #[test]
    fn liquids_and_air_are_not_items() {
        let registry = registry();

        assert!(registry.id("core:air").is_none());
        assert!(registry.id("core:water").is_none());
        assert!(registry.id("core:stone").is_some());
    }
}
