//! App-side egui glue: input forwarding and building one tessellated frame.

use egui_wgpu::ScreenDescriptor;
use winit::event::WindowEvent;

use crate::renderer::UiFrame;

use super::App;

impl App {
    /// Opens the inventory on the demo path.
    pub(super) fn open_inventory(&mut self) {
        if self.demo.is_none() || self.ui.inventory_open() {
            return;
        }
        self.end_break();
        self.release_cursor();
        self.pressed.clear();
        self.jump_requested = false;
        self.ui.open_inventory(self.game.inventory.selected_index());
    }

    /// Closes the inventory, returning a held stack first.
    pub(super) fn close_inventory(&mut self) {
        if let Some(held) = self.ui.take_held()
            && let Some(leftover) = self.game.inventory.add(held, &self.items)
        {
            self.ui.set_held(Some(leftover));
            return;
        }
        self.ui.close_inventory();
        self.capture_cursor();
    }

    /// Forwards one window event to egui and reports whether gameplay must
    /// ignore it.
    pub(super) fn handle_ui_event(&mut self, event: &WindowEvent) -> bool {
        let Some(window) = self.window.clone() else {
            return false;
        };
        if let Some(state) = &mut self.egui_state {
            let response = state.on_window_event(&window, event);
            if response.repaint {
                window.request_redraw();
            }
        }

        let pointer = matches!(
            event,
            WindowEvent::CursorMoved { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
        );
        let keyboard = matches!(event, WindowEvent::KeyboardInput { .. });
        (pointer && self.ui.captures_pointer()) || (keyboard && self.ui.captures_keyboard())
    }

    /// Runs egui for this frame and returns its tessellated output.
    pub(super) fn build_ui_frame(&mut self) -> Option<UiFrame> {
        self.demo.as_ref()?;
        let window = self.window.clone()?;
        let target = self.hud_target();
        let raw_input = {
            let state = self.egui_state.as_mut()?;
            state.take_egui_input(&window)
        };

        let inventory = &mut self.game.inventory;
        let registry = &self.items;
        let ui = &mut self.ui;
        let output = self.egui_ctx.run_pass(raw_input, |ctx| {
            ui.draw(ctx, inventory, registry, target.as_deref());
        });

        let egui::FullOutput {
            platform_output,
            textures_delta,
            shapes,
            pixels_per_point,
            ..
        } = output;
        if let Some(state) = &mut self.egui_state {
            state.handle_platform_output(&window, platform_output);
        }
        let primitives = self.egui_ctx.tessellate(shapes, pixels_per_point);
        let size = window.inner_size();
        let pixels_per_point = if pixels_per_point > 0.0 {
            pixels_per_point
        } else {
            1.0
        };
        if self.ui.take_close_requested() {
            self.close_inventory();
        }
        Some(UiFrame {
            textures_delta,
            primitives,
            screen: ScreenDescriptor {
                size_in_pixels: [size.width.max(1), size.height.max(1)],
                pixels_per_point,
            },
        })
    }

    fn hud_target(&self) -> Option<String> {
        let hit = self.target_hit()?;
        let demo = self.demo.as_ref()?;
        let id = demo.block_at(hit.block)?;
        self.pack.name(id).map(str::to_owned)
    }
}
