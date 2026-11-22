//! Shared slot icon, count, and durability drawing for the HUD and inventory.

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, vec2};

use crate::game::{ItemKind, ItemRegistry, ItemStack};
use crate::renderer::item_color;

/// Draws one stack inside `rect`. The slot background and border are the
/// caller's responsibility.
pub(super) fn draw_stack(painter: &Painter, rect: Rect, stack: ItemStack, registry: &ItemRegistry) {
    let icon = Rect::from_min_max(rect.min + vec2(7.0, 7.0), rect.max - vec2(7.0, 7.0));
    let [red, green, blue] = item_color(registry.name(stack.item));
    painter.rect_filled(
        icon,
        2.0,
        Color32::from_rgb(channel(red), channel(green), channel(blue)),
    );
    painter.rect_stroke(
        icon,
        2.0,
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 0, 0, 90)),
        StrokeKind::Inside,
    );

    if stack.count > 1 {
        painter.text(
            rect.right_bottom() - vec2(4.0, 2.0),
            Align2::RIGHT_BOTTOM,
            stack.count.to_string(),
            FontId::proportional(13.0),
            Color32::WHITE,
        );
    }

    if let ItemKind::Tool {
        durability: maximum,
        ..
    } = registry.def(stack.item).kind
    {
        let fraction = f32::from(stack.durability) / f32::from(maximum.max(1));
        let bar = Rect::from_min_size(
            Pos2::new(rect.left() + 4.0, rect.bottom() - 6.0),
            vec2((rect.width() - 8.0) * fraction, 3.0),
        );
        painter.rect_filled(bar, 1.0, Color32::from_rgb(80, 220, 90));
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn channel(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}
