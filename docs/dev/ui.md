# UI

**Status:** HUD and inventory implemented in `0.0.10.2`; the crafting, pause, death, and settings
screens remain.

**Scope:** The egui integration, input routing, the HUD, the inventory screen, and the render pass.
Gameplay state lives in [Gameplay core](gameplay.md); this page covers presentation and event
ownership.

| Area | Status |
|---|---|
| HUD foundation | Current (`0.0.10.2`) |
| Inventory screen | Current (`0.0.10.2`) |
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
| `src/ui/mod.rs` | `UiState`, screen ownership, the HUD/inventory draw entry point, and the pointer/keyboard capture queries. |
| `src/ui/hud.rs` | Crosshair, hotbar slots, and interaction prompt. |
| `src/ui/inventory.rs` | The inventory panel, grid, cursor stack, drag/drop, shift-click, tooltips, keyboard navigation, and close handling. |
| `src/ui/item_icon.rs` | Shared slot icon, stack count, and tool durability drawing. |
| `src/app/ui.rs` | `egui_winit` forwarding, `take_egui_input`, `run_pass`, platform output, tessellation, `UiFrame` construction, and inventory open/close. |
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
  event. Both return `false` for the HUD and `true` while the inventory is open.
- `E` opens the inventory from gameplay. The opening frame suppresses the same key press so the
  screen does not close immediately.
- While the inventory is open, `E`, `Escape`, the close button, or the screen losing a held stack
  to a full inventory determine whether it closes. `Escape` is handled through `egui` input
  because keyboard events are skipped before `App::handle_key`.
- Opening releases the cursor and clears held movement keys. Closing returns any cursor stack to
  the inventory and recaptures the cursor.
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

## Inventory

`E` opens the inventory on the demo path. It dims the world, releases the cursor, and draws a
centred panel with the same 37 slots as `Inventory`:

| Area | Slots |
|---|---|
| Main grid, three rows of nine | `9..36` |
| Hotbar row | `0..9` |
| Offhand | `36` |

The panel header shows `E / Esc close   Shift-click move   Arrows + Enter`. The footer has a
`Close [E]` button and the slot count. The HUD is hidden while the inventory is open.

Slot borders encode state:

| State | Border |
|---|---|
| Keyboard focus | Blue |
| Hovered | Pale blue |
| Selected hotbar slot | White |
| Valid drop target while holding a stack | Green |

Each occupied slot uses the shared item icon: a flat colour, the stack count when above one, and a
green durability bar for tools. Empty slots show their 1-based number.

### Mouse interactions

- **Left click, empty cursor:** pick up the whole stack.
- **Left click, holding a stack:** place into an empty slot, merge into a matching stack up to the
  limit, or swap with a different stack.
- **Right click, empty cursor:** take half a stack, rounded up.
- **Right click, holding a stack:** place one item.
- **Shift-click:** move the stack between the hotbar and the main grid. Hotbar stacks go to
  `9..36`; main and offhand stacks go to `0..9`. The destination range is merged first, then
  filled. A full range leaves the remainder in the source slot.
- **Drag:** start dragging a slot to pick up its stack. The held stack follows the pointer. Release
  over a slot to place it; release elsewhere to return it to the first free slot. A held stack from
  a previous click can also be dragged and dropped.
- **Close:** a held stack is returned to the inventory first. If the inventory is full, the screen
  stays open with the stack still on the cursor, so closing can never destroy an item.

### Keyboard

- Arrow keys move the focus by one slot left or right, or by a row up or down. The focus is clamped
  to `0..=36`.
- `Enter` or `Space` activates the focused slot with the same left-click rules.
- Holding `Shift` while activating uses the shift-click quick move.
- `E` or `Escape` closes the screen. The opening `E` press is suppressed so it does not close the
  screen in its first frame.

### Tooltips

Hovering a slot shows its display name, kind, stack count, and tool durability. It also shows
`Click to pick up · Shift-click to move`, or `Click to place` while holding a stack.

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
- `ui::tests::the_inventory_draws_and_captures_input` opens the screen and pins the pointer and
  keyboard capture policy.
- `ui::tests::click_picks_up_and_places_a_stack`, `right_click_splits_and_places_one`, and
  `shift_click_moves_between_the_hotbar_and_main_inventory` cover the interaction rules without a
  window.
- `ui::tests::the_opening_key_is_suppressed_and_the_next_press_closes` pins the two-frame `E`
  behavior.
- The windowed smoke run captured a frame with the nine-slot hotbar, selected border, wooden
  pickaxe icon, durability bar, crosshair, and the existing FPS overlay.
- `ENGINE_UI_INVENTORY=1` opens the inventory on the first frame for repeatable captures. The
  captured inventory shows the main grid, hotbar row, offhand, selected slot, durability bar,
  close button, and dimmed world.
- GPU validation is exercised by `Renderer::new` and the windowed run; the default test suite
  remains headless.

## Current limitations

- No crafting, pause, death, or settings screen.
- No container-to-container transfer beyond the hotbar/main quick move.
- No gamepad navigation or accessibility tree.
- No UI scale control; egui follows the window scale factor.
- No text input, clipboard, or link handling; those features are disabled.
- Item icons are flat colours, not the block texture array.
- The HUD and inventory are not captured in golden terrain images; captures are manual QA, not
  comparison tests.

## Next

The next slice is crafting in `0.0.10.3`:

- A data-driven recipe table with shaped and shapeless recipes.
- A 2x2 grid in the inventory and a 3x3 workbench screen.
- A recipe book, search, and one-click craft.
- Tool durability repair and the wood-to-stone progression chain.
