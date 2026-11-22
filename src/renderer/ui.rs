//! The egui render pass.
//!
//! The app owns the `egui::Context` and the winit input state; the renderer
//! owns the GPU buffers and textures. A [`UiFrame`] is the tessellated output
//! and texture delta for one frame.

use egui::{ClippedPrimitive, TexturesDelta};
use egui_wgpu::{Renderer as EguiRenderer, RendererOptions, ScreenDescriptor};

use super::Renderer;

/// One frame of tessellated UI ready for the GPU.
pub(crate) struct UiFrame {
    pub(crate) textures_delta: TexturesDelta,
    pub(crate) primitives: Vec<ClippedPrimitive>,
    pub(crate) screen: ScreenDescriptor,
}

pub(super) struct UiPass {
    renderer: EguiRenderer,
}

impl UiPass {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self {
            renderer: EguiRenderer::new(device, format, RendererOptions::default()),
        }
    }

    /// Uploads textures and vertex/index buffers, returning staging command
    /// buffers that must be submitted before the frame encoder.
    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &UiFrame,
    ) -> Vec<wgpu::CommandBuffer> {
        for (id, deltas) in &frame.textures_delta.set {
            for delta in deltas {
                self.renderer.update_texture(device, queue, *id, delta);
            }
        }
        for id in &frame.textures_delta.free {
            self.renderer.free_texture(id);
        }
        self.renderer
            .update_buffers(device, queue, encoder, &frame.primitives, &frame.screen)
    }

    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'static>, frame: &UiFrame) {
        self.renderer.render(pass, &frame.primitives, &frame.screen);
    }
}

impl Renderer {
    pub(super) fn prepare_ui(
        &mut self,
        frame: Option<&mut UiFrame>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(frame) = frame else {
            return Vec::new();
        };
        let commands = self.ui.prepare(&self.device, &self.queue, encoder, frame);
        frame.textures_delta.clear();
        commands
    }

    /// Draws the UI into a second color-only pass, loading the scene result.
    pub(super) fn draw_ui(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        frame: &UiFrame,
    ) {
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })];
        let descriptor = wgpu::RenderPassDescriptor {
            label: Some("ui pass"),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        };
        let mut pass = encoder.begin_render_pass(&descriptor).forget_lifetime();
        self.ui.draw(&mut pass, frame);
    }
}
