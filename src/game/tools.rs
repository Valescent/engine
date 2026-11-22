//! Tool classes and hardness-based break rules.
//!
//! Block hardness is content data. The class and minimum tier are game tables
//! for now; they move into the authored content pack when it lands.

/// What kind of tool a block prefers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolClass {
    Hand,
    Pickaxe,
    Axe,
    Shovel,
    Sword,
}

/// The tool view a break calculation needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Tool {
    pub(crate) class: ToolClass,
    pub(crate) tier: u8,
}

/// A block's preferred tool and the minimum tier that yields a drop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BreakRule {
    pub(crate) class: ToolClass,
    pub(crate) min_tier: u8,
}

/// The break rule for a stable block name.
///
/// Unknown blocks fall back to hand mining at tier zero, so a new block is
/// always breakable and never blocks the loop.
pub(crate) fn break_rule(block: &str) -> BreakRule {
    let (class, min_tier) = match block {
        "core:stone" | "core:ore_copper" | "core:ice" | "core:boulder" => (ToolClass::Pickaxe, 1),
        "core:ore_iron" | "core:furnace" => (ToolClass::Pickaxe, 2),
        "core:bedrock" => (ToolClass::Pickaxe, 4),
        "core:wood" | "core:planks" | "core:workbench" | "core:leaves" | "core:plant" => {
            (ToolClass::Axe, 0)
        }
        "core:dirt" | "core:grass" | "core:sand" | "core:gravel" | "core:clay" | "core:snow" => {
            (ToolClass::Shovel, 0)
        }
        _ => (ToolClass::Hand, 0),
    };
    BreakRule { class, min_tier }
}

/// Speed multiplier for the held tool against the block's preferred class.
fn speed(rule: BreakRule, tool: Option<Tool>) -> f32 {
    match tool {
        Some(tool) if tool.class == rule.class => match tool.tier {
            0 => 1.0,
            1 => 2.0,
            2 => 4.0,
            _ => 6.0,
        },
        _ => 1.0,
    }
}

/// Seconds needed to break a block, with a floor of one fixed tick.
pub(crate) fn break_seconds(hardness: f32, block: &str, tool: Option<Tool>) -> f32 {
    let rule = break_rule(block);
    (hardness / speed(rule, tool)).max(1.0 / 60.0)
}

/// Whether the held tool meets the block's minimum tier and can yield a drop.
pub(crate) fn can_drop(block: &str, tool: Option<Tool>) -> bool {
    let rule = break_rule(block);
    let tier = tool.map_or(0, |tool| tool.tier);
    tier >= rule.min_tier
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_matching_tool_is_faster() {
        let hand = break_seconds(1.5, "core:stone", None);
        let wood = break_seconds(
            1.5,
            "core:stone",
            Some(Tool {
                class: ToolClass::Pickaxe,
                tier: 1,
            }),
        );

        assert!(wood < hand);
        assert!((wood - 0.75).abs() < 1e-6);
    }

    #[test]
    fn a_wrong_tool_gets_no_bonus() {
        let hand = break_seconds(2.0, "core:wood", None);
        let pickaxe = break_seconds(
            2.0,
            "core:wood",
            Some(Tool {
                class: ToolClass::Pickaxe,
                tier: 3,
            }),
        );

        assert!((hand - pickaxe).abs() < 1e-6);
    }

    #[test]
    fn stone_needs_a_wooden_pickaxe_for_a_drop() {
        let hand = None;
        let wood = Some(Tool {
            class: ToolClass::Pickaxe,
            tier: 1,
        });

        assert!(!can_drop("core:stone", hand));
        assert!(can_drop("core:stone", wood));
        assert!(!can_drop("core:ore_iron", wood));
    }

    #[test]
    fn bedrock_never_drops() {
        assert!(!can_drop("core:bedrock", None));
        assert!(!can_drop(
            "core:bedrock",
            Some(Tool {
                class: ToolClass::Pickaxe,
                tier: 3,
            })
        ));
    }
}
