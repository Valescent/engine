//! The pause overlay: resume or save and quit.

use egui::{Align2, Color32, Context, Id, Key, LayerId, Order, RichText, Stroke};

use super::UiState;

pub(super) fn draw(ctx: &Context, state: &mut UiState) {
    let screen = ctx.input(egui::InputState::viewport_rect);
    ctx.layer_painter(LayerId::new(Order::Middle, Id::unique("pause-dim")))
        .rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 150));

    let mut resume = false;
    let mut quit = false;
    egui::Area::new(Id::unique("pause"))
        .anchor(Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .order(Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(12, 14, 18, 240))
                .stroke(Stroke::new(
                    1.0,
                    Color32::from_rgba_unmultiplied(255, 255, 255, 55),
                ))
                .corner_radius(10.0)
                .inner_margin(24.0)
                .show(ui, |ui| {
                    ui.label(RichText::new("Paused").size(24.0).strong());
                    ui.add_space(10.0);
                    if ui.button("Resume [Esc]").clicked() {
                        resume = true;
                    }
                    if ui.button("Save & Quit").clicked() {
                        quit = true;
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Settings and keybinds arrive in 0.0.10.5.")
                            .size(11.0)
                            .color(Color32::from_rgba_unmultiplied(190, 190, 190, 180)),
                    );
                });
        });

    let escape = ctx.input(|input| input.key_pressed(Key::Escape));
    if state.suppress_close_key {
        state.suppress_close_key = false;
    } else if escape {
        resume = true;
    }
    if resume {
        state.request_resume();
    }
    if quit {
        state.quit_requested = true;
    }
}
