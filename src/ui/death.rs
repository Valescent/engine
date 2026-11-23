//! The death overlay: respawn or save and quit.

use egui::{Align2, Color32, Context, Id, Key, LayerId, Order, RichText, Stroke};

use super::UiState;

pub(super) fn draw(ctx: &Context, state: &mut UiState) {
    let screen = ctx.input(egui::InputState::viewport_rect);
    ctx.layer_painter(LayerId::new(Order::Middle, Id::unique("death-dim")))
        .rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(40, 0, 0, 170));

    let mut respawn = false;
    let mut quit = false;
    egui::Area::new(Id::unique("death"))
        .anchor(Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .order(Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(18, 10, 10, 240))
                .stroke(Stroke::new(
                    1.0,
                    Color32::from_rgba_unmultiplied(255, 120, 110, 90),
                ))
                .corner_radius(10.0)
                .inner_margin(24.0)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("You died")
                            .size(24.0)
                            .strong()
                            .color(Color32::from_rgb(240, 150, 140)),
                    );
                    ui.add_space(8.0);
                    if ui.button("Respawn [R]").clicked() {
                        respawn = true;
                    }
                    if ui.button("Save & Quit").clicked() {
                        quit = true;
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("The inventory is kept on respawn.")
                            .size(11.0)
                            .color(Color32::from_rgba_unmultiplied(200, 190, 190, 180)),
                    );
                });
        });

    if ctx.input(|input| input.key_pressed(Key::R) || input.key_pressed(Key::Enter)) {
        respawn = true;
    }
    if respawn {
        state.request_respawn();
    }
    if quit {
        state.quit_requested = true;
    }
}
