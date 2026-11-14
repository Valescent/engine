# Why I Chose winit for Windowing

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted  
**Date:** 2026-11-13  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None  
**Scope:** Window creation, the event loop, and platform input events. The graphics API is decided separately (wgpu), and input mapping, gamepad support, and UI are out of scope here.

---

## Summary

This document records why the engine uses winit for windowing and platform events. It is both an official decision record and a devlog entry: the decision is final for the current scope, and I am publishing the reasoning so future decisions can be traced back to it.

The short version: winit is the standard Rust windowing crate, it is the crate wgpu's own examples build surfaces against, and it does nothing except windows and events. That matches what I need: a thin, cross-platform platform layer under a renderer I own, with the event loop in my control so chunk uploads and frame pacing are my decisions. The tradeoff is that winit has a demanding event-loop model, still changes between releases, and gives me no input abstraction, no gamepad support, and no UI.

---

## 1. Context

### 1.1 The project

The engine needs a platform layer that does very little and stays out of the way:

- **A surface for wgpu.** wgpu does not create windows; it renders into a surface built from a native window handle. The windowing crate has to produce that handle cleanly on every platform.
- **A loop I control.** Chunk meshing and GPU upload work has to happen at points I choose. I do not want a framework deciding when frames happen.
- **Raw input.** A first-person camera needs mouse motion, cursor locking, and focus-change handling. I want the raw events, not a keybinding abstraction chosen for me.
- **Resize, minimize, DPI changes.** A resizable window means surface reconfiguration, and high-DPI monitors mean scale-factor handling. These are correctness concerns, not polish.
- **Multi-platform reach.** Windows first, but a Linux or macOS build and a possible browser build should not require replacing the platform layer.
- **No framework lock-in.** I am building the engine, so the platform layer must not carry an application framework, a UI toolkit, or a rendering opinion with it.

These requirements point toward a minimal, cross-platform, Rust-native windowing and event crate — not a full application framework and not hand-written platform code.

### 1.2 The constraints

My windowing choice is constrained by the following:

- It must integrate with wgpu's surface creation through a native window handle.
- It must support more than one desktop platform without per-platform code in the engine.
- It must expose platform events directly rather than through an opinionated input model.
- It must leave frame pacing, render order, and application structure to me.
- It must be usable from Rust without a large unsafe FFI surface.

---

## 2. Decision

**winit is the windowing and platform-event library for the engine.**

This covers window creation, the event loop, monitor and scale-factor information, and raw keyboard, mouse, and focus events. Surface creation and presentation remain wgpu's responsibility. Input mapping, gamepad support, and in-engine UI are out of scope for this decision and will be decided separately.

I accept this decision for the current scope. I will revisit it only if the assumptions in this document stop holding.

---

## 3. Why I Chose winit

### 3.1 It is the standard pairing for wgpu

wgpu's own examples and most Rust renderers create their surfaces from a winit window via `raw-window-handle`. Choosing winit means the integration path is the well-trodden one: create a window, build a surface from its handle, reconfigure on resize. I am not inventing an integration layer, and when I hit a problem, the reference implementations use the same two crates I do.

### 3.2 Minimal scope, no framework lock-in

winit does windows and events. It has no renderer, no ECS, no scene graph, and no UI toolkit. That is exactly the boundary I want: the engine owns its architecture, and the platform layer stays a platform layer. A framework like Bevy or eframe would hand me a working window and then expect me to build inside its structure, which conflicts with the point of writing an engine.

### 3.3 Cross-platform reach that matches the renderer

winit supports Windows, macOS, Linux (X11 and Wayland), Android, iOS, and the web. That is the same reach wgpu gives me. The two decisions reinforce each other: if the renderer can run on a platform, the window layer can too, so portability is a build concern rather than a rewrite.

### 3.4 An event loop I control

winit hands me the loop. Frames happen when I request a redraw, not on a hidden timer. That matters for a voxel engine, where a block change should rebuild one chunk mesh and a frame should be presented when the work is actually done. winit's `ApplicationHandler` model makes ownership of engine state explicit, which is awkward at first but keeps frame timing honest.

### 3.5 Raw input for a first-person camera

winit delivers raw mouse motion, cursor position, cursor grab and confinement, focus loss, and keyboard state as events. A first-person controller needs relative motion and needs to release the cursor when the window loses focus; those are exactly the primitives winit exposes. It does not decide my keybindings, sensitivity curves, or input buffering, which is the correct division of responsibility for an engine.

### 3.6 Resize, DPI, and monitor handling are already solved

Window resizing, scale-factor changes, monitor enumeration, and fullscreen transitions are platform-specific minefields, and winit already handles them. I get scale factors and monitor information as data and can respond by reconfiguring the wgpu surface, instead of writing per-OS code for display changes.

### 3.7 Honest about maintenance risk

winit is actively developed and tracks platform changes, including Wayland and WebGPU-era browser support. It is also the crate the rest of the Rust ecosystem depends on, so its health is not tied to my project alone. That does not make it stable — it still breaks between releases — but it means the breakage is visible, documented, and shared with everyone else, rather than a private platform layer rotting in my repository.

### 3.8 Why not SDL, GLFW, or raw platform APIs?

- **SDL2/SDL3** is mature and battle-tested, but it is a C library behind FFI bindings, it brings audio, threading, and input abstractions I did not ask for, and its types do not compose with Rust ownership. It also offers its own graphics API, which is a competing decision I do not want bundled into my windowing decision.
- **GLFW** is small and proven, but it is C, it is OpenGL-shaped, and it has no first-class web story. It also does less for modern high-DPI and Wayland behavior.
- **Raw Win32, Cocoa, and X11/Wayland** would give me total control at the cost of writing and maintaining three platform layers before I render a cube. That is the wrong order of work for a solo project.
- **Bevy or `eframe`** would give me a window inside a larger framework with its own application model. I am not adopting that model.

---

## 4. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **winit** | Rust-native, minimal, multi-platform, standard wgpu pairing, raw events, explicit event loop, active maintenance. | **Selected** |
| **SDL2 / SDL3** | Very mature, wide platform support, bundled input and audio; C FFI surface, larger scope, opinions I do not need, competing graphics API. | Not selected |
| **GLFW** | Small and proven, simple C API; OpenGL-shaped, weaker modern platform support, no web path. | Not selected |
| **Raw platform APIs** | Maximum control; three separate implementations, high unsafe cost, slow to first window. | Not selected |
| **Bevy / `eframe`** | Working window and loop immediately; framework and application-model lock-in. | Not selected |
| **`minifb` / `pixels`** | Very simple; too limited for a first-person 3D engine (input, DPI, surface control). | Not selected |

---

## 5. Counterarguments and Disposition

This decision has real weaknesses. I am recording them here rather than omitting them, so the record stays honest.

| Counterargument | Disposition |
|---|---|
| **winit's event-loop model is awkward.** In recent versions the loop owns the application state through `ApplicationHandler`, which forces a particular structure and fights straightforward "main loop" code. | Accepted. I will structure the engine around an explicit application type from the start rather than retrofit it. The model is awkward but it is also the current standard, and fighting it costs more than adapting to it. |
| **winit changes between releases.** Breaking changes are common, and examples written for an older version often do not apply. | Accepted. I will pin versions, upgrade deliberately between milestones, and keep all winit usage inside a platform module so an upgrade touches one layer. |
| **It gives me no input abstraction.** Keybindings, action mapping, dead zones, and buffering are my problem. | Accepted. That is the intended scope. I want a first-person controller that I control; a thin wrapper over winit events is cheaper than adopting someone else's input model. |
| **No gamepad support.** | Accepted for now. Gamepads are not on the roadmap. If they become needed, `gilrs` is a separate decision layered on top, not a reason to replace the window layer. |
| **No UI.** Debug overlays and menus need another crate. | Accepted. Debug UI is not required by any current milestone. `egui` with its winit integration is the obvious later addition. |
| **Wayland and X11 behave differently.** Cursor locking, window positioning, and some events are not identical across them. | Accepted. Development is Windows-first; Linux support will be validated when it is actually targeted, with Wayland behavior checked explicitly rather than assumed. |
| **It does not do frame pacing or vsync.** Presentation timing is wgpu's and mine. | Accepted. I want control over when frames are presented. The cost is that I must implement pacing consciously instead of inheriting it. |
| **Documentation is thin and scattered.** | Accepted with qualification. The winit examples repository is the primary reference and is reasonably complete. This is a learning cost, not a blocker. |

---

## 6. Consequences

### 6.1 Positive

- One platform layer serves Windows, macOS, Linux, and potentially the web.
- The wgpu surface integration follows the standard, well-documented path.
- The event loop stays under my control, so frame pacing and chunk upload timing are engine decisions.
- Raw input events support a first-person camera directly, including cursor lock and focus loss.
- Resize, DPI, and monitor handling come from a maintained crate instead of per-OS code.
- The engine keeps its own architecture with no framework dependency.

### 6.2 Negative

- The `ApplicationHandler` event-loop model constrains how engine state is structured.
- winit upgrades will cost migration work.
- Input abstraction, gamepad support, and UI all require additional crates and decisions.
- Platform behavior differs, so cross-platform correctness is tested rather than assumed.
- Frame pacing must be implemented by me.
- Heavy use of winit types in engine code would couple the engine to it, so a boundary module is required.

### 6.3 Neutral

- Surface creation belongs to wgpu, so windowing and rendering stay separate decisions.
- `raw-window-handle` is the shared interface between the two, and it is already a dependency of both crates.
- Native window handles remain reachable if a platform-specific feature is ever needed.

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| winit API churn breaks the platform layer on upgrade. | Pin the version; upgrade deliberately between milestones; keep winit usage isolated in one module; check the changelog and examples before upgrading. |
| The event-loop model forces a painful engine structure. | Design the application type and state ownership at v0.0.1, before rendering complexity exists; follow the current documented pattern rather than old tutorials. |
| Platform-specific bugs appear outside Windows. | Develop Windows-first; validate Linux and macOS only when targeted; test Wayland and X11 separately when Linux becomes a goal. |
| Input state is lost or stuck on focus changes. | Reset input state on focus loss; release cursor grab on focus loss; clear held keys and motion deltas; test Alt-Tab explicitly. |
| DPI and resize handling produces stretched or blurry output. | Reconfigure the wgpu surface on scale-factor and size events; test on a high-DPI display and on a scaled secondary monitor. |
| Frame pacing is missing, causing tearing or wasted CPU. | Implement present mode selection explicitly through wgpu; measure frame time; add a frame limiter only if measurement shows a need. |
| Engine code becomes coupled to winit types. | Keep a thin platform module that translates winit events into engine events; the rest of the engine never imports winit. |

---

## 8. Conclusion

winit is my deliberate, scope-driven, portability-aware choice for the engine's windowing layer. I am selecting it because it is the standard Rust windowing crate, it pairs directly with wgpu, it stays out of the engine's architecture, and it exposes the raw platform events a first-person voxel engine needs. I acknowledge that its event-loop model is awkward, that it changes between releases, and that it gives me no input abstraction, gamepad support, or UI. I manage that through version pinning, an isolated platform module, deliberate upgrades, and the separate decisions those gaps will require. I therefore accept this decision as the windowing and platform-event layer for the engine, with rendering left to wgpu and with its limits recorded above.

---

*This entry is part of the Valescent Engine decision log. Future entries should reference this document when they depend on the platform layer or revise its assumptions.*
