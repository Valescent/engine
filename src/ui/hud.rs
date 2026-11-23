//! The non-interactive HUD: crosshair, hotbar, and interaction prompt.

use egui::{
    Align2, Color32, Context, FontId, Id, LayerId, Order, Rect, Sense, Stroke, StrokeKind, vec2,
};

use super::item_icon::draw_stack;
use crate::game::{
    HOTBAR_SLOTS, Inventory, ItemRegistry, MAX_AIR, MAX_HEALTH, MAX_STAMINA, Survival,
};

/// The data one HUD frame reads.
#[derive(Clone, Copy)]
pub(crate) struct HudData<'a> {
    pub(crate) inventory: &'a Inventory,
    pub(crate) registry: &'a ItemRegistry,
    pub(crate) target: Option<&'a str>,
    pub(crate) survival: &'a Survival,
    pub(crate) damage_flash: f32,
}

pub(super) fn draw(ctx: &Context, data: HudData<'_>) {
    crosshair(ctx);
    hotbar(ctx, &data);
    survival_bars(ctx, data.survival);
    prompt(ctx, data.target);
    flash(ctx, data.damage_flash);
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

fn survival_bars(ctx: &Context, survival: &Survival) {
    egui::Area::new(Id::unique("survival-bars"))
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -68.0))
        .order(Order::Foreground)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
            bar(
                ui,
                survival.health(),
                MAX_HEALTH,
                Color32::from_rgb(205, 62, 62),
            );
            bar(
                ui,
                survival.stamina(),
                MAX_STAMINA,
                Color32::from_rgb(220, 190, 74),
            );
            if survival.air() < MAX_AIR - 0.1 {
                bar_f32(ui, survival.air(), MAX_AIR, Color32::from_rgb(92, 166, 225));
            }
        });
}

fn bar(ui: &mut egui::Ui, value: u16, maximum: u16, color: Color32) {
    bar_f32(ui, f32::from(value), f32::from(maximum.max(1)), color);
}

fn bar_f32(ui: &mut egui::Ui, value: f32, maximum: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(220.0, 9.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, Color32::from_rgba_unmultiplied(0, 0, 0, 165));
    let fraction = (value / maximum.max(0.001)).clamp(0.0, 1.0);
    let inner = Rect::from_min_size(
        rect.min + vec2(1.0, 1.0),
        vec2((rect.width() - 2.0) * fraction, rect.height() - 2.0),
    );
    painter.rect_filled(inner, 2.0, color);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        format!("{value:.0}/{maximum:.0}"),
        FontId::proportional(8.0),
        Color32::WHITE,
    );
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn flash(ctx: &Context, strength: f32) {
    if strength <= 0.0 {
        return;
    }
    let screen = ctx.input(egui::InputState::viewport_rect);
    let alpha = u8::try_from((strength.clamp(0.0, 1.0) * 70.0) as u32).unwrap_or(0);
    ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("damage-flash")))
        .rect_filled(
            screen,
            0.0,
            Color32::from_rgba_unmultiplied(190, 20, 20, alpha),
        );
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
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -112.0))
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
