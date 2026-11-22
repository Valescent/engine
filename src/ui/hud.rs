//! The non-interactive HUD: crosshair, hotbar, and interaction prompt.

use egui::{Align2, Color32, Context, FontId, Id, LayerId, Order, Sense, Stroke, StrokeKind, vec2};

use super::item_icon::draw_stack;
use crate::game::{HOTBAR_SLOTS, Inventory, ItemRegistry};

/// The data one HUD frame reads.
#[derive(Clone, Copy)]
pub(crate) struct HudData<'a> {
    pub(crate) inventory: &'a Inventory,
    pub(crate) registry: &'a ItemRegistry,
    pub(crate) target: Option<&'a str>,
}

pub(super) fn draw(ctx: &Context, data: HudData<'_>) {
    crosshair(ctx);
    hotbar(ctx, &data);
    prompt(ctx, data.target);
}

fn crosshair(ctx: &Context) {
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("crosshair")));
    let center = ctx.input(egui::InputState::viewport_rect).center();
    let shadow = Color32::from_rgba_unmultiplied(0, 0, 0, 140);
    let colour = Color32::from_rgba_unmultiplied(255, 255, 255, 210);
    for (from, to) in [
        (vec2(-9.0, 0.0), vec2(-3.0, 0.0)),
        (vec2(3.0, 0.0), vec2(9.0, 0.0)),
        (vec2(0.0, -9.0), vec2(0.0, -3.0)),
        (vec2(0.0, 3.0), vec2(0.0, 9.0)),
    ] {
        painter.line_segment([center + from, center + to], Stroke::new(3.0, shadow));
        painter.line_segment([center + from, center + to], Stroke::new(1.5, colour));
    }
}

fn hotbar(ctx: &Context, data: &HudData<'_>) {
    egui::Area::new(Id::unique("hotbar"))
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -14.0))
        .order(Order::Foreground)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
            ui.horizontal(|ui| {
                for index in 0..HOTBAR_SLOTS {
                    slot(ui, index, data.inventory, data.registry);
                }
            });
        });
}

fn slot(ui: &mut egui::Ui, index: usize, inventory: &Inventory, registry: &ItemRegistry) {
    let (rect, _) = ui.allocate_exact_size(vec2(46.0, 46.0), Sense::hover());
    let painter = ui.painter();
    let selected = inventory.selected_index() == index;
    painter.rect_filled(rect, 4.0, Color32::from_rgba_unmultiplied(0, 0, 0, 150));
    let border = if selected {
        Stroke::new(2.0, Color32::WHITE)
    } else {
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 70))
    };
    painter.rect_stroke(rect, 4.0, border, StrokeKind::Inside);

    let Some(stack) = inventory.slot(index) else {
        painter.text(
            rect.left_top() + vec2(4.0, 3.0),
            Align2::LEFT_TOP,
            (index + 1).to_string(),
            FontId::monospace(10.0),
            Color32::from_rgba_unmultiplied(255, 255, 255, 90),
        );
        return;
    };

    draw_stack(painter, rect, stack, registry);
}

fn prompt(ctx: &Context, target: Option<&str>) {
    let Some(target) = target else {
        return;
    };
    let label = target
        .rsplit(':')
        .next()
        .unwrap_or(target)
        .replace('_', " ");
    egui::Area::new(Id::unique("interaction-prompt"))
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -72.0))
        .order(Order::Foreground)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(format!("Hold left: break {label}    |    Right: place"))
                    .color(Color32::WHITE)
                    .background_color(Color32::from_rgba_unmultiplied(0, 0, 0, 150))
                    .size(14.0),
            );
        });
}
