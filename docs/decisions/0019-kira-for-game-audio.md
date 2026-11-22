# Why I Chose kira for Game Audio

**Document type:** Architecture Decision Record / Devlog Entry
**Status:** Accepted; scoped for v0.0.10
**Date:** 2026-11-22
**Project:** Valescent Engine
**Owner:** Project Owner
**Supersedes:** None.
**Scope:** The audio backend, mixer, buses, and placeholder sound source. It does not decide the
final sound set, music licensing, or a plugin format.

---

## Summary

This document records why the engine uses kira 0.12 for audio and synthesises placeholder sounds at
startup instead of committing binary assets. The short version: kira is built for games, gives the
project mixer tracks and positional sounds without a custom graph, and a procedural placeholder set
keeps the repository free of audio provenance questions until the art pass.

---

## 1. Context

### 1.1 The audio work in v0.0.10

The release needs interaction sounds (break, place, pickup, step, damage), a UI click, and a quiet
ambience bed. Later releases add positional creature and water sounds, music, weather, and caves.

### 1.2 The constraints

- The engine has no audio assets and no asset pipeline. The texture pipeline is procedural today.
- Windows, Linux, and macOS are in CI. The backend must build on all three; Linux needs ALSA
  headers at build time.
- Audio must degrade gracefully when no output device exists, which is common in headless CI and
  on remote machines.
- The game loop must never block on the audio device.
- The licence set is restricted by `deny.toml`; MIT, Apache-2.0, BSD, ISC, Zlib, Unicode, and MPL
  are allowed.
- The 1,000-line gate applies to the wrapper, so audio lives in its own module.

---

## 2. Decision

**kira 0.12 is the audio backend, with `cpal` and WAV decoding enabled and every other codec
disabled.**

- A `cpal` output stream feeds kira's mixer on a dedicated audio thread.
- Buses are `music`, `ambience`, `ui`, and `world`; the world bus carries positional sounds.
- Placeholder sounds are generated as PCM at startup and wrapped in `StaticSoundData`. No WAV,
  OGG, or MP3 files are committed in 0.0.10.
- The Ubuntu CI job installs `libasound2-dev` so cpal links.
- If no device opens, the audio system becomes a no-op and logs once. Gameplay continues.

---

## 3. Why kira

### 3.1 It is a game mixer, not a media player

kira exposes tracks, buses, effects, and positional sounds as first-class concepts. That matches
the roadmap: ambience, UI, world, and music need independent volume and later need effects.

### 3.2 The wrapper stays small

The app pushes `play(sound, bus)` and `set_volume(bus, value)` calls. The mixer, resampling, and
device handling stay inside kira, so the engine module stays well under the size gate.

### 3.3 Procedural placeholders fit the repository

The texture array is procedural placeholder art. Synthesised clicks, thumps, and noise beds are the
audio equivalent: they exercise the backend and the buses without binary assets or licence
metadata. They are explicitly placeholders, replaced by the 0.0.11 art pass.

### 3.4 Graceful failure

Opening a device is fallible and may fail in CI. kira's typed errors let the wrapper downgrade to a
silent backend with one warning instead of failing startup.

### 3.5 Licence fit

kira, cpal, and the enabled dependencies are MIT or Apache-2.0. The `cargo deny` licence allow-list
already covers them.

---

## 4. Alternatives considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **kira 0.12** | Game-oriented mixer, buses, positional sounds, small wrapper | **Selected** |
| rodio | Simple playback, but no game mixer or bus model; positional and effects work would be custom | Not selected |
| cpal directly | Maximum control, but rebuilds mixing, resampling, and scheduling | Not selected |
| Platform APIs | Three implementations and no shared mixer | Not selected |
| No audio in 0.0.10 | Fails the audio half of the definition of done | Not selected |

---

## 5. Counterarguments and disposition

| Counterargument | Disposition |
|---|---|
| kira pulls cpal and platform audio code | Accepted. It is the standard cost of cross-platform audio; the codec features are trimmed to WAV. |
| Linux CI needs `libasound2-dev` | Accepted. One apt package in the Ubuntu job is cheaper than a custom backend. |
| Procedural sounds are not good art | Accepted with qualification. They are placeholders that prove the buses and interaction hooks; 0.0.11 replaces them. |
| Audio can fail in headless runs | Mitigated by the no-op fallback and a single warning. |
| A second thread can complicate shutdown | Mitigated by owning the audio system in `App` and dropping it before the event loop exits. |

---

## 6. Consequences

### 6.1 Positive

- One mixer with independent music, ambience, UI, and world volume.
- Positional sounds are available for creatures and water without a rewrite.
- No binary audio assets or provenance work in 0.0.10.

### 6.2 Negative

- A new native dependency and a Linux build package.
- Placeholder synthesis is extra code that the art pass will replace.
- Audio cannot be verified by the headless test; it needs a manual run.

### 6.3 Neutral

- The game starts silently when no device is available.
- The sound set is deliberately small until the art pass.

---

## 7. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Linux CI fails to link ALSA | Install `libasound2-dev` in the Ubuntu test job and keep the audio module unconditional. |
| A device open blocks startup | Open the device off the main thread or fail to the no-op backend; never wait indefinitely. |
| Audio thread panics on shutdown | Own the system in `App`, stop sources, and drop it before the event loop returns. |
| Placeholder sounds are too loud or harsh | Apply a master limiter and conservative default volumes; expose per-bus settings. |
| Codec features creep in | Keep `default-features = false` with only `cpal` and `wav`. |

---

## 8. Conclusion

kira is the deliberate choice for v0.0.10 audio because it provides a game mixer, buses, and
positional playback without a custom audio graph, and because it fits the existing licence and
platform constraints. I accept the cpal/ALSA dependency and the placeholder sound quality; both are
bounded by feature trimming, CI setup, and the 0.0.11 art pass.

---

*This entry is part of the Valescent Engine decision log. Future audio work should reference
ADR-0019 when it depends on the bus layout or the procedural placeholder policy.*
