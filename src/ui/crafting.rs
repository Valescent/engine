//! The shared crafting panel: 2x2/3x3 grid, output slot, known recipe book,
//! direct craft buttons, and salvage.

use egui::{Color32, RichText, Sense, Stroke, StrokeKind, vec2};

use crate::game::{Inventory, ItemRegistry, RecipeBook, display_name};

use super::UiState;
use super::inventory::{SLOT_SIZE, item_label, tooltip};
use super::item_icon::draw_stack;

const PANEL_WIDTH: f32 = 360.0;
const LIST_HEIGHT: f32 = 170.0;

pub(super) fn draw_panel(
    ui: &mut egui::Ui,
    state: &mut UiState,
    inventory: &mut Inventory,
    registry: &ItemRegistry,
    recipes: &RecipeBook,
) {
    ui.set_width(PANEL_WIDTH);
    ui.label(RichText::new("Crafting").size(16.0).strong());
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            let size = usize::from(state.station().grid_size());
            for y in 0..size {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    for x in 0..size {
                        craft_slot(ui, state, y * size + x, inventory, registry);
                    }
                });
            }
        });
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.label(
                RichText::new("Output")
                    .size(11.0)
                    .color(Color32::from_rgba_unmultiplied(210, 210, 210, 190)),
            );
            output_slot(ui, state, inventory, registry, recipes);
        });
    });

    if let Some(error) = state.craft_error {
        ui.add_space(4.0);
        ui.colored_label(Color32::from_rgb(235, 110, 100), error.to_string());
    }

    ui.add_space(7.0);
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Recipe book").strong());
        ui.label(
            RichText::new(format!("{} known", recipes.len()))
                .size(10.0)
                .color(Color32::from_rgba_unmultiplied(180, 180, 180, 170)),
        );
    });
    let search = ui.text_edit_singleline(&mut state.recipe_search);
    state.search_focused = search.has_focus();

    let indices = recipes.search(&state.recipe_search, registry);
    if state.selected_recipe.is_none() {
        state.selected_recipe = indices.first().copied();
    }
    if indices.is_empty() {
        ui.label(
            RichText::new("No recipes match.")
                .size(11.0)
                .color(Color32::from_rgba_unmultiplied(190, 190, 190, 180)),
        );
    } else {
        egui::ScrollArea::vertical()
            .max_height(LIST_HEIGHT)
            .show(ui, |ui| {
                for &index in &indices {
                    let Some(recipe) = recipes.get(index) else {
                        continue;
                    };
                    let available = state.station().can_make(recipe.recipe.station);
                    let craftable =
                        available && recipe.craftable_from_inventory(inventory, registry);
                    let marker = if !available {
                        "WB"
                    } else if craftable {
                        "OK"
                    } else {
                        "--"
                    };
                    let label = format!(
                        "{marker:>2}  {} x{}",
                        display_name(registry.name(recipe.output)),
                        recipe.output_count
                    );
                    if ui
                        .selectable_label(state.selected_recipe == Some(index), label)
                        .clicked()
                    {
                        state.selected_recipe = Some(index);
                        state.craft_error = None;
                    }
                }
            });
    }

    ui.add_space(6.0);
    if let Some(index) = state.selected_recipe
        && let Some(recipe) = recipes.get(index)
    {
        recipe_details(ui, state, inventory, registry, recipes, index, recipe);
    }

    ui.add_space(6.0);
    ui.separator();
    salvage_row(ui, state, inventory, registry);
}

fn craft_slot(
    ui: &mut egui::Ui,
    state: &mut UiState,
    index: usize,
    inventory: &mut Inventory,
    registry: &ItemRegistry,
) {
    let (rect, response) = ui.allocate_exact_size(vec2(SLOT_SIZE, SLOT_SIZE), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 5.0, Color32::from_rgba_unmultiplied(30, 32, 38, 255));
    let border = if response.hovered() {
        Stroke::new(2.0, Color32::from_rgb(180, 210, 255))
    } else {
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 45))
    };
    painter.rect_stroke(rect, 5.0, border, StrokeKind::Inside);

    if let Some(stack) = state.craft.slot(index) {
        draw_stack(painter, rect, stack, registry);
    }

    let shift = ui.input(|input| input.modifiers.shift);
    if response.clicked() {
        state.click_craft_slot(index, inventory, registry, false, shift);
    }
    if response.secondary_clicked() {
        state.click_craft_slot(index, inventory, registry, true, shift);
    }

    let stack = state.craft.slot(index);
    let holding = state.held.is_some();
    response.on_hover_ui(|ui| tooltip(ui, stack, registry, holding));
}

fn output_slot(
    ui: &mut egui::Ui,
    state: &mut UiState,
    inventory: &mut Inventory,
    registry: &ItemRegistry,
    recipes: &RecipeBook,
) {
    let preview = recipes
        .find(&state.craft, state.station())
        .and_then(|matched| matched.recipe.output_stack(registry));
    let (rect, response) = ui.allocate_exact_size(vec2(SLOT_SIZE, SLOT_SIZE), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 5.0, Color32::from_rgba_unmultiplied(24, 30, 24, 255));
    let border = if preview.is_some() {
        Stroke::new(2.0, Color32::from_rgb(120, 220, 140))
    } else {
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 45))
    };
    painter.rect_stroke(rect, 5.0, border, StrokeKind::Inside);
    if let Some(stack) = preview {
        draw_stack(painter, rect, stack, registry);
    }

    let shift = ui.input(|input| input.modifiers.shift);
    if response.clicked() {
        state.click_craft_output(inventory, recipes, registry, shift);
    }
    response.on_hover_ui(|ui| {
        if let Some(stack) = preview {
            ui.label(
                RichText::new(format!(
                    "{} x{}",
                    display_name(registry.name(stack.item)),
                    stack.count
                ))
                .strong(),
            );
            ui.label("Click to craft one - Shift-click to craft all");
        } else {
            ui.label("No matching recipe");
        }
    });
}

fn recipe_details(
    ui: &mut egui::Ui,
    state: &mut UiState,
    inventory: &mut Inventory,
    registry: &ItemRegistry,
    recipes: &RecipeBook,
    index: usize,
    recipe: &crate::game::CompiledRecipe,
) {
    ui.separator();
    ui.label(
        RichText::new(format!(
            "{} x{}",
            display_name(registry.name(recipe.output)),
            recipe.output_count
        ))
        .strong(),
    );
    ui.label(
        RichText::new(format!(
            "{} - {} - {} - {} - {}",
            recipe.recipe.station.label(),
            recipe.recipe.tier.label(),
            recipe.recipe.discipline.label(),
            recipe.recipe.unlock.label(),
            recipe.recipe.cost.label(),
        ))
        .size(11.0)
        .color(Color32::from_rgba_unmultiplied(205, 205, 205, 190)),
    );

    let status = recipe.ingredient_status(inventory, registry);
    let craftable = recipe.craftable_from_inventory(inventory, registry);
    for line in &status {
        let color = if line.have >= line.need {
            Color32::from_rgb(140, 220, 145)
        } else {
            Color32::from_rgb(235, 140, 120)
        };
        ui.colored_label(color, format!("{} {}/{}", line.label, line.have, line.need));
    }

    let available = state.station().can_make(recipe.recipe.station);
    let can_craft = available && craftable;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(can_craft, egui::Button::new("Craft"))
            .clicked()
        {
            state.craft_direct_recipe(index, inventory, recipes, registry, 1);
        }
        if ui
            .add_enabled(can_craft, egui::Button::new("Craft x10"))
            .clicked()
        {
            state.craft_direct_recipe(index, inventory, recipes, registry, 10);
        }
    });
    if !available {
        ui.label(
            RichText::new("Requires a workbench.")
                .size(11.0)
                .color(Color32::from_rgb(235, 190, 110)),
        );
    }
}

fn salvage_row(
    ui: &mut egui::Ui,
    state: &mut UiState,
    inventory: &mut Inventory,
    registry: &ItemRegistry,
) {
    let held = state.held;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(held.is_some(), egui::Button::new("Salvage held"))
            .clicked()
        {
            state.salvage_held_item(inventory, registry);
        }
        if let Some(stack) = held {
            ui.label(
                RichText::new(format!(
                    "{} x{}",
                    item_label(registry.name(stack.item)),
                    stack.count
                ))
                .size(11.0)
                .color(Color32::from_rgba_unmultiplied(205, 205, 205, 190)),
            );
        } else {
            ui.label(
                RichText::new("Pick up an item to salvage it.")
                    .size(11.0)
                    .color(Color32::from_rgba_unmultiplied(180, 180, 180, 170)),
            );
        }
    });
}
