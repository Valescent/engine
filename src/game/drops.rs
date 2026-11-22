//! Block drop tables, resolved through the content pack and the item registry.

use crate::content;

use super::item::{ItemRegistry, ItemStack};
use super::tools::{self, Tool};

/// The item stacks a broken block yields for the held tool.
///
/// The content pack owns the drop names and counts. A tool below the block's
/// minimum tier yields nothing.
pub(crate) fn drops_for(
    pack: &content::ContentPack,
    registry: &ItemRegistry,
    block: &str,
    tool: Option<Tool>,
) -> Vec<ItemStack> {
    if !tools::can_drop(block, tool) {
        return Vec::new();
    }
    let Some(id) = pack.id(block) else {
        return Vec::new();
    };
    let Some(record) = pack.records().get(usize::from(id)) else {
        return Vec::new();
    };
    let mut drops = Vec::new();
    for (name, count) in &record.drops {
        if let Some(stack) = registry.stack(name, u16::from(*count)) {
            drops.push(stack);
        }
    }
    drops
}

#[cfg(test)]
mod tests {
    use crate::content;

    use super::super::item::{ItemKind, ItemRegistry};
    use super::super::tools::{Tool, ToolClass};
    use super::*;

    fn tables() -> (content::ContentPack, ItemRegistry) {
        let pack = content::v1_default();
        let registry = ItemRegistry::builtin(&pack);
        (pack, registry)
    }

    fn pickaxe() -> Tool {
        Tool {
            class: ToolClass::Pickaxe,
            tier: 1,
        }
    }

    #[test]
    fn stone_drops_stone_with_a_wooden_pickaxe() {
        let (pack, registry) = tables();
        let drops = drops_for(&pack, &registry, "core:stone", Some(pickaxe()));

        assert_eq!(drops.len(), 1);
        assert_eq!(registry.name(drops[0].item), "core:stone");
        assert_eq!(drops[0].count, 1);
    }

    #[test]
    fn stone_drops_nothing_by_hand() {
        let (pack, registry) = tables();

        assert_eq!(
            drops_for(&pack, &registry, "core:stone", None),
            Vec::<ItemStack>::new()
        );
    }

    #[test]
    fn every_drop_resolves_to_a_block_item() {
        let (pack, registry) = tables();

        for record in pack.records() {
            for name in record.drops.keys() {
                let id = registry.id(name).expect("every drop has an item");
                assert!(matches!(registry.def(id).kind, ItemKind::Block { .. }));
            }
        }
    }

    #[test]
    fn unknown_blocks_drop_nothing() {
        let (pack, registry) = tables();

        assert_eq!(
            drops_for(&pack, &registry, "missing:block", None),
            Vec::<ItemStack>::new()
        );
    }
}
