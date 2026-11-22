//! The inventory screen: grid, cursor stack, drag/drop, shift-click, and tooltips.

use egui::{
    Align2, Color32, Context, FontId, Id, LayerId, Order, Rect, RichText, Sense, Stroke,
    StrokeKind, vec2,
};

use crate::game::{
    HOTBAR_SLOTS, Inventory, ItemKind, ItemRegistry, ItemStack, OFFHAND_SLOT, TOTAL_SLOTS,
};

use super::UiState;
use super::item_icon::draw_stack;

const SLOT_SIZE: f32 = 48.0;

#[allow(
    clippy::too_many_lines,
    reason = "the inventory layout and interaction pass are one immediate-mode frame"
)]
pub(super) fn draw(
    ctx: &Context,
    state: &mut UiState,
    inventory: &mut Inventory,
    registry: &ItemRegistry,
) {
    let screen = ctx.input(egui::InputState::viewport_rect);
    ctx.layer_painter(LayerId::new(Order::Middle, Id::unique("inventory-dim")))
        .rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 140));

    let mut rects: Vec<(usize, Rect)> = Vec::with_capacity(TOTAL_SLOTS);
    let mut drag_start = None;
    let mut click = None;

    egui::Area::new(Id::unique("inventory"))
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .order(Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(12, 14, 18, 235))
                .stroke(Stroke::new(
                    1.0,
                    Color32::from_rgba_unmultiplied(255, 255, 255, 45),
                ))
                .corner_radius(10.0)
                .inner_margin(18.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Inventory")
                                .size(20.0)
                                .strong()
                                .color(Color32::WHITE),
                        );
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new("E / Esc close   Shift-click move   Arrows + Enter")
                                .size(11.0)
                                .color(Color32::from_rgba_unmultiplied(200, 200, 200, 180)),
                        );
                    });
                    ui.add_space(10.0);

                    for row in 0..3 {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                            for column in 0..9 {
                                let index = HOTBAR_SLOTS + row * 9 + column;
                                view_slot(
                                    ui,
                                    index,
                                    state,
                                    inventory,
                                    registry,
                                    &mut rects,
                                    &mut drag_start,
                                    &mut click,
                                );
                            }
                        });
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                        for index in 0..HOTBAR_SLOTS {
                            view_slot(
                                ui,
                                index,
                                state,
                                inventory,
                                registry,
                                &mut rects,
                                &mut drag_start,
                                &mut click,
                            );
                        }
                    });

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Offhand")
                                .size(12.0)
                                .color(Color32::from_rgba_unmultiplied(210, 210, 210, 200)),
                        );
                        view_slot(
                            ui,
                            OFFHAND_SLOT,
                            state,
                            inventory,
                            registry,
                            &mut rects,
                            &mut drag_start,
                            &mut click,
                        );
                    });

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Close [E]").clicked() {
                            state.close_requested = true;
                        }
                        ui.label(
                            RichText::new(format!("{TOTAL_SLOTS} slots"))
                                .size(11.0)
                                .color(Color32::from_rgba_unmultiplied(180, 180, 180, 170)),
                        );
                    });
                });
        });

    let (left, right, up, down, enter, shift) = ctx.input(|input| {
        (
            input.key_pressed(egui::Key::ArrowLeft),
            input.key_pressed(egui::Key::ArrowRight),
            input.key_pressed(egui::Key::ArrowUp),
            input.key_pressed(egui::Key::ArrowDown),
            input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space),
            input.modifiers.shift,
        )
    });
    if left {
        state.focused_slot = state.focused_slot.saturating_sub(1);
    }
    if right && state.focused_slot < OFFHAND_SLOT {
        state.focused_slot += 1;
    }
    if up && state.focused_slot >= HOTBAR_SLOTS {
        state.focused_slot -= 9;
    }
    if down && state.focused_slot + 9 <= OFFHAND_SLOT {
        state.focused_slot += 9;
    }
    if enter {
        state.activate(state.focused_slot, inventory, registry, false, shift);
    }

    if let Some(index) = drag_start {
        if state.held.is_some() {
            state.dragging = true;
        } else {
            state.pick_up(index, inventory);
            state.dragging = state.held.is_some();
        }
    } else if let Some((index, secondary)) = click {
        state.activate(index, inventory, registry, secondary, shift);
    }

    let pointer = ctx.input(|input| input.pointer.hover_pos());
    let released = ctx.input(|input| input.pointer.any_released());
    if state.dragging && released {
        let target = pointer.and_then(|position| {
            rects
                .iter()
                .find(|(_, rect)| rect.contains(position))
                .map(|(index, _)| *index)
        });
        if let Some(index) = target {
            state.place_held(index, inventory, registry, false);
        } else {
            state.return_held(inventory, registry);
        }
        state.dragging = false;
    }

    if let Some(position) = pointer
        && let Some((_, rect)) = rects.iter().find(|(_, rect)| rect.contains(position))
        && state.held.is_some()
    {
        ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("drop-target")))
            .rect_stroke(
                *rect,
                5.0,
                Stroke::new(2.0, Color32::from_rgb(120, 230, 140)),
                StrokeKind::Inside,
            );
    }

    if let (Some(stack), Some(position)) = (state.held, pointer) {
        let held_painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::unique("held-item")));
        let rect = Rect::from_center_size(position, vec2(36.0, 36.0));
        held_painter.rect_filled(rect, 4.0, Color32::from_rgba_unmultiplied(0, 0, 0, 190));
        draw_stack(&held_painter, rect, stack, registry);
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the slot view needs the UI, state, inventory, registry, and three collectors"
)]
fn view_slot(
    ui: &mut egui::Ui,
    index: usize,
    state: &UiState,
    inventory: &Inventory,
    registry: &ItemRegistry,
    rects: &mut Vec<(usize, Rect)>,
    drag_start: &mut Option<usize>,
    click: &mut Option<(usize, bool)>,
) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(SLOT_SIZE, SLOT_SIZE), Sense::click_and_drag());
    rects.push((index, rect));

    let painter = ui.painter();
    painter.rect_filled(rect, 5.0, Color32::from_rgba_unmultiplied(30, 32, 38, 255));
    let mut border = if index == state.focused_slot {
        Stroke::new(2.0, Color32::from_rgb(90, 160, 255))
    } else {
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 45))
    };
    if index < HOTBAR_SLOTS && inventory.selected_index() == index {
        border = Stroke::new(2.0, Color32::WHITE);
    }
    if response.hovered() {
        border = Stroke::new(2.0, Color32::from_rgb(180, 210, 255));
    }
    painter.rect_stroke(rect, 5.0, border, StrokeKind::Inside);

    if let Some(stack) = inventory.slot(index) {
        draw_stack(painter, rect, stack, registry);
    } else {
        painter.text(
            rect.left_top() + vec2(4.0, 3.0),
            Align2::LEFT_TOP,
            (index + 1).to_string(),
            FontId::monospace(10.0),
            Color32::from_rgba_unmultiplied(255, 255, 255, 80),
        );
    }

    if response.drag_started() {
        *drag_start = Some(index);
    }
    if response.clicked() {
        *click = Some((index, false));
    }
    if response.secondary_clicked() {
        *click = Some((index, true));
    }

    let stack = inventory.slot(index);
    let holding = state.held.is_some();
    response.on_hover_ui(|ui| tooltip(ui, stack, registry, holding));
}

fn tooltip(ui: &mut egui::Ui, stack: Option<ItemStack>, registry: &ItemRegistry, holding: bool) {
    if let Some(stack) = stack {
        ui.label(RichText::new(item_label(registry.name(stack.item))).strong());
        match registry.def(stack.item).kind {
            ItemKind::Block { .. } => {
                ui.label("Block");
            }
            ItemKind::Tool {
                class,
                tier,
                durability: maximum,
            } => {
                ui.label(format!("{class:?} · tier {tier}"));
                ui.label(format!("Durability {}/{maximum}", stack.durability));
            }
            ItemKind::Material => {
                ui.label("Material");
            }
            ItemKind::Food { nutrition } => {
                ui.label(format!("Food · {nutrition}"));
            }
        }
        if stack.count > 1 {
            ui.label(format!("x{}", stack.count));
        }
        if holding {
            ui.label("Click to place");
        } else {
            ui.label("Click to pick up · Shift-click to move");
        }
    } else {
        ui.label("Empty slot");
        if holding {
            ui.label("Click to place");
        }
    }
}

fn item_label(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).replace('_', " ")
}
