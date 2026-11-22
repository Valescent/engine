# UI

**Status:** HUD foundation implemented in `0.0.10.2`; the inventory, crafting, pause, death, and
settings screens remain.

**Scope:** The egui integration, input routing, the HUD, and the render pass. Gameplay state lives
in [Gameplay core](gameplay.md); this page covers presentation and event ownership.

| Area | Status |
|---|---|
| HUD foundation | Current (`0.0.10.2`) |
| Inventory screen | Planned (`0.0.10.2`) |
| Crafting screen | Planned (`0.0.10.3`) |
| Pause, death, settings | Planned (`0.0.10.4`-`0.0.10.5`) |

## Stack

| Crate | Version | Role |
|---|---|---|
| `egui` | 0.37 | Immediate-mode widgets, layout, text, and painting |
| `egui-wgpu` | 0.37 | Tessellated UI meshes and textures on the engine's `wgpu` 30 device |
| `egui-winit` | 0.37 | Window events, scale factor, and cursor state on the engine's `winit` 0.30 |

The dependencies use `default-features = false`; `egui` keeps `default_fonts`. Clipboard, link
opening, accesskit, and platform menu integration are not enabled yet. The engine's own `winit`
dependency still enables X11, Wayland, and raw-window-handle 0.6, so the unified winit build has the
platform backends the engine already needs.

`egui-wgpu` and `egui-winit` are not optional features. They compile in the normal
`--no-default-features` CI build, which keeps the UI path from drifting untested.

## Module map

| Path | Responsibility |
|---|---|
| `src/ui/mod.rs` | `UiState`, the HUD draw entry point, and the pointer/keyboard ownership queries. |
| `src/ui/hud.rs` | Crosshair, hotbar slots, item colour, count, durability, and interaction prompt. |
| `src/app/ui.rs` | `egui_winit` forwarding, `take_egui_input`, `run_pass`, platform output, tessellation, and `UiFrame` construction. |
| `src/renderer/ui.rs` | `UiPass`, `UiFrame`, texture and buffer upload, the second color-only render pass, and texture-delta cleanup. |

`egui` types appear only in these modules plus `App`'s context/state fields. `src/game/` remains
headless and names no UI or GPU type.

## Ownership

| Owner | State |
|---|---|
| `App` | `egui::Context`, `Option<egui_winit::State>`, and `UiState` |
| `Renderer` | `UiPass`, which owns the `egui_wgpu::Renderer`, its textures, and its buffers |
| `UiFrame` | One frame's tessellated primitives, texture delta, and screen descriptor |

`App::resumed` creates the winit adapter after the window exists. The context is created in
`App::with_world`, so headless tests can use it without a window. `Renderer::new` creates the GPU
renderer with the surface format.

## Frame flow

1. `App::window_event` forwards the event to `egui_winit::State` first.
2. `App::handle_ui_event` returns `true` only when an open screen owns that event class. The HUD
   owns nothing, so gameplay continues normally.
3. `App::build_ui_frame` takes egui input, runs `egui::Context::run_pass`, handles platform output,
   tessellates the shapes, and builds a `UiFrame`.
4. `Renderer::render` calls `prepare_ui` before the scene pass. It uploads changed textures, frees
   removed textures, and updates the UI vertex/index buffers. The returned staging command buffers
   are submitted before the frame encoder.
5. The world, highlight, and debug overlay draw in the scene pass.
6. A second color-only pass loads the scene result and draws the egui primitives with no depth
   attachment.
7. The frame is captured after the UI pass, so `ENGINE_CAPTURE` includes the HUD.
8. `textures_delta.clear()` runs after the upload; `epaint` asserts if a delta is dropped without
   being applied.

The UI frame is built even when the window is unfocused, so the HUD does not flicker on focus
changes. Gameplay input remains focus-gated.

## Input routing

The policy is **UI first, gameplay second**:

- Every window event goes to `egui_winit::State` before the engine match.
- `UiState::captures_pointer` and `captures_keyboard` decide whether gameplay must ignore the
  event. Both return `false` for the HUD.
- The inventory screen will return `true` while it is open. The screen must handle `Escape` itself
  through `egui` input, because the event is skipped before `App::handle_key`.
- Opening a screen must release the cursor. The HUD does not change capture.
- The first left click after a release still only captures; it does not break or place.
- The cursor grab, focus loss, break cancellation, and `Ctrl+S` paths are unchanged.

## HUD

The HUD is drawn with `egui` layers and anchored areas, not a panel, so the world remains visible.

| Element | Placement | Content |
|---|---|---|
| Crosshair | Screen centre | Four short white segments with a dark shadow and a 3 px gap |
| Hotbar | Bottom centre, 14 px above the edge | Nine 46 px slots, 4 px apart |
| Interaction prompt | Above the hotbar | `Hold left: break the targeted block | Right: place` when a demo target exists |

Each hotbar slot shows:

- A dark translucent background and a border. The selected slot has a white 2 px border.
- The slot number in the top-left corner when empty.
- An item icon as a flat colour from the renderer's placeholder palette.
- The stack count at the bottom-right when it is above one.
- A green durability bar at the bottom for a tool.

The HUD does not show health, stamina, hunger, or oxygen yet. Those arrive with survival in
`0.0.10.4`. The existing bitmap debug overlay (`F3`) remains separate and is drawn in the scene
pass before the egui pass.

## Rendering details

`UiPass` uses `egui_wgpu::Renderer::new` with `RendererOptions::default()`:

- No depth-stencil attachment. The UI pass is color-only.
- No MSAA. The surface and scene pass are single-sample.
- Dithering is on by default.

`egui` warns once that the engine's surface is `Bgra8UnormSrgb` and it prefers a non-sRGB target.
The warning is expected: egui converts its colors into the sRGB target. Changing the surface format
would affect the whole renderer and is deferred.

The UI pass uses `LoadOp::Load` and `StoreOp::Store`; it never clears the scene. A frame with no
primitives produces no UI draws, and the headless path builds no `UiFrame` at all.

## Testing

- `ui::tests::the_hud_draws_without_a_window` runs `egui::Context::run_pass` with a 1280x720 raw
  input, draws an empty inventory with a stone target, and asserts that shapes were produced.
- `ui::tests::the_hud_does_not_capture_input` pins the current ownership policy.
- The windowed smoke run captured a frame with the nine-slot hotbar, selected border, wooden
  pickaxe icon, durability bar, crosshair, and the existing FPS overlay.
- GPU validation is exercised by `Renderer::new` and the windowed run; the default test suite
  remains headless.

## Current limitations

- No inventory or crafting screen; `E` is not bound yet.
- No pause, death, or settings screen.
- No gamepad navigation or accessibility tree.
- No UI scale control; egui follows the window scale factor.
- No text input, clipboard, or link handling; those features are disabled.
- Item icons are flat colours, not the block texture array.
- The HUD is not captured in golden terrain images; captures are manual QA, not comparison tests.

## Next

The next slice adds the inventory screen:

- `E` opens and `Escape` closes; the cursor is released while it is open.
- 9 hotbar, 27 main, and one offhand slot, using the existing `Inventory` operations.
- Drag and drop, shift-click, tooltips, keyboard focus, and a selected-item cursor stack.
- UI-first routing becomes real: `captures_pointer` and `captures_keyboard` return `true` while
  the screen is open.

Crafting, the recipe book, and the workbench follow in `0.0.10.3`.
