# Why I Chose egui for the Game UI

**Document type:** Architecture Decision Record / Devlog Entry
**Status:** Accepted; HUD and inventory screen implemented in `0.0.10.2` (crafting and settings pending)
**Date:** 2026-11-22
**Project:** Valescent Engine
**Owner:** Project Owner
**Supersedes:** None.
**Scope:** The gameplay UI layer: HUD, inventory, crafting, pause, death, settings, and debug
screens. It does not decide fonts, the final visual style, or in-world diegetic UI.

---

## Summary

This document records why the engine uses egui, rendered through the existing wgpu device, for all
gameplay screens. The short version: egui 0.37 already targets wgpu 30 and winit 0.30, which is the
exact stack the engine ships, and an immediate-mode layer gets the inventory, crafting, and
settings screens out of the way without building a widget toolkit first.

---

## 1. Context

### 1.1 The UI work in v0.0.10

The release needs a hotbar, a crosshair, health and stamina bars, an inventory grid with drag and
drop, a 2x2 and 3x3 crafting surface, a recipe book, a pause menu, a death screen, and a settings
screen. All of it is ordinary 2D UI over a 3D scene.

### 1.2 The constraints

- The renderer already owns a `wgpu::Device`, `Queue`, surface configuration, depth texture, and a
  render pass. A UI library must share that device, not create a second GPU stack.
- The engine is pinned to `wgpu 30` and `winit 0.30`. A UI crate that pulls a different wgpu major
  would require type conversion or a duplicate device.
- Input must be routed before gameplay: when the inventory is open, the mouse must not look or
  break blocks, and `Escape` must close the screen before it releases capture.
- The crate uses `unsafe_code = "deny"` and clippy pedantic. A UI layer with a large unsafe surface
  is unacceptable.
- The project supports Windows, Linux, and macOS in CI. The UI must build on all three without a
  system toolkit.
- The 1,000-line hard file gate applies to the integration, so the UI renderer lives in its own
  module.

---

## 2. Decision

**egui 0.37 is the gameplay UI layer, with `egui-wgpu` 0.37 and `egui-winit` 0.37.**

- `egui-wgpu` depends on `wgpu ^30`, matching the engine.
- `egui-winit` depends on `winit ^0.30.13`, matching the engine.
- The app owns one `egui::Context`, one `egui_winit::State`, and one `egui_wgpu::Renderer`.
- UI draw commands are rendered after the world pass and before presentation.
- `egui-winit` receives every window event first. Its `consumed` result decides whether gameplay
  input is skipped.
- The cursor is released whenever a screen that needs the pointer opens.

---

## 3. Why egui

### 3.1 Stack alignment

The versions line up without a second wgpu or winit. That is the deciding technical fact: a UI
layer that forced a wgpu upgrade or a parallel device would cost more than the UI itself.

### 3.2 Immediate mode matches the data

The inventory, crafting grid, and recipe book are projections of `GameState`. Immediate mode
rebuilds them from state each frame, so there is no retained widget tree to synchronise with
inventory changes, recipe unlocks, or durability updates.

### 3.3 Input routing is explicit

`egui_winit::State` returns whether it consumed an event. The app can keep one policy: UI first,
gameplay second, and only when the pointer is captured. This is exactly the routing rule the
release needs.

### 3.4 No system toolkit

egui renders through wgpu and does not require GTK, Qt, or a platform webview. That keeps the CI
matrix and the release binary unchanged.

### 3.5 Debug screens come for free

The same layer can host the profiling overlay, configuration toggles, and future terrain debug
views, replacing hand-built text and quads.

---

## 4. Alternatives considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **egui 0.37** | Immediate mode, wgpu 30, winit 0.30, MIT/Apache-2.0, no system toolkit | **Selected** |
| Hand-rolled wgpu UI | Full control and no dependency, but needs text shaping, fonts, layout, focus, and widgets; would add well over 1,000 lines before the first screen | Not selected |
| iced | Retained and pleasant, but brings its own runtime and renderer integration; heavier than the screens need | Not selected |
| Platform-native menus | Good for settings, wrong for an inventory grid and a HUD | Not selected |
| No UI, debug text only | Fails the definition of done: inventory, crafting, health, and settings cannot be used | Not selected |

---

## 5. Counterarguments and disposition

| Counterargument | Disposition |
|---|---|
| egui adds a large dependency tree | Accepted. The tree is permissive, widely used, and already aligned with wgpu and winit. `cargo deny` will gate licences and duplicates. |
| Immediate mode redraws every widget every frame | Accepted. The screens are small and only visible when open; the HUD is a handful of widgets. A hidden UI skips the pass. |
| egui's look is generic | Accepted with qualification. The default theme is a placeholder; palette and spacing are art work in 0.0.11. |
| A custom UI would be more "engine-like" | Rejected for 0.0.10. Building a toolkit is not on the critical path to a playable loop. |
| Input ordering bugs can leak clicks into the world | Mitigated by the single UI-first routing rule, a capture release on open, and tests for the event policy. |

---

## 6. Consequences

### 6.1 Positive

- Inventory, crafting, HUD, and settings share one layer and one input policy.
- No second GPU device or wgpu version.
- The debug overlay can migrate to real widgets instead of bitmap text.

### 6.2 Negative

- A new dependency family enters the build.
- The renderer gains a second pipeline and bind group for UI textures.
- Immediate mode needs discipline to avoid rebuilding expensive data every frame.

### 6.3 Neutral

- UI textures and fonts come from egui defaults until the art pass.
- The UI pass can be disabled for headless and capture runs.

---

## 7. Risks and mitigations

| Risk | Mitigation |
|---|---|
| egui and wgpu drift apart on upgrade | Pin the matching minor versions and upgrade them together; the dependency job catches duplicates. |
| UI steals gameplay input incorrectly | Route all events through `egui_winit`, test the consumed policy, and release capture when a screen opens. |
| UI draw order breaks the capture path | Render UI last, after the world and highlight, and exclude it from golden terrain captures. |
| A hidden UI still costs frame time | Skip `egui::Context::run` and the UI pass when no screen or HUD element is visible. |
| Fonts or texture formats differ on a platform | Use egui defaults and the surface format already selected by the renderer. |

---

## 8. Conclusion

egui is the deliberate choice for the v0.0.10 gameplay UI because it matches the engine's wgpu and
winit versions, keeps input routing explicit, and avoids building a widget toolkit before the game
is playable. I accept the dependency cost and the generic default look; both are contained by
version pinning, the licence gate, and the 0.0.11 art pass.

---

*This entry is part of the Valescent Engine decision log. Future UI work should reference ADR-0018
when it depends on the immediate-mode layer or the UI-first input policy.*
