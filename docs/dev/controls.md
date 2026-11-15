# Controls

Keyboard and mouse bindings for the current engine and the scoped milestones. Sections marked
**planned** come from scope pages and are not implemented yet. Camera angle conventions are in
[Coordinates](coordinates.md#camera-and-direction).

| Area | Status |
|---|---|
| Fly-camera bindings | Current (v0.0.2) |
| Chunk boundary demo toggle | Planned (v0.0.3) |
| Walking, interaction, hotbar, save | Planned (v0.0.4) |
| Day/night debug key | Planned (v0.0.6) |
| Remappable bindings | Planned, no milestone |

## Bindings by milestone

| Input | v0.0.2 (current) | v0.0.3 (planned) | v0.0.4 (planned) |
|---|---|---|---|
| `W` / `S` | Fly forward / back | Fly forward / back | Walk forward / back |
| `A` / `D` | Strafe left / right | Strafe left / right | Walk left / right |
| Mouse move | Look | Look | Look |
| Left click | Capture the cursor | Capture the cursor | Capture first; break when captured |
| Right click | — | — | Place the selected block |
| `Space` | Toggle the demo voxel | Toggle the boundary demo voxel | Jump |
| `1`–`5` | — | — | Select stone, dirt, grass, wood, leaves |
| `Ctrl+S` | — | — | Save all non-empty chunks |
| `Escape` | Release the cursor | Release the cursor | Release the cursor |

Mouse look requires capture and window focus in every milestone.

In v0.0.2 and v0.0.3, `W` / `S` follow the full view direction, including pitch, and `A` / `D`
strafe horizontally. v0.0.4 replaces flight with walking.

v0.0.4 also saves on exit. The v0.0.3 demo toggle is not carried forward on `Space`, and the
scope assigns no replacement key.

## Current capture and focus (v0.0.2)

- Gameplay input is handled only while the window is focused.
- Mouse look requires both capture and focus. It reads raw mouse motion; sensitivity is
  `0.0025 rad/px`.
- Left click requests capture and hides the cursor when the grab succeeds.
- Capture prefers a locked grab, falls back to a confined grab, and logs a warning if neither
  works.
- `Escape` releases the grab and shows the cursor. It does not close the window, pause the
  simulation, or clear held movement keys.
- Losing focus releases capture and clears held movement keys. Regaining focus does not
  recapture automatically.
- WASD moves the camera while the window is focused, with or without capture.
- Key repeat is ignored. Holding `Space` toggles the demo voxel once.
- Movement uses a frame delta clamped to `100 ms`, so a stalled frame cannot teleport the
  camera.

## Interaction (planned, v0.0.4)

- Block interaction requires the cursor to be captured.
- The first left click after the cursor is released captures only; it does not break or place.
- Once captured, left click breaks the targeted block and right click places the selected block
  in the adjacent cell.
- Capture is acquired by left click only. Right click is not a capture button.
- Break and place are click actions; the scope does not specify hold-to-repeat.
- There is no pause menu. `Escape` still only releases the cursor.

## Tuning

| Parameter | Value | Status |
|---|---|---|
| Fly speed | `8 m/s` | Current (v0.0.2) |
| Mouse sensitivity | `0.0025 rad/px` | Current (v0.0.2) |
| Walk speed | `4.5 m/s` | Planned (v0.0.4) |
| Jump velocity | `8 m/s` | Planned (v0.0.4) |
| Gravity | `24 m/s²` | Planned (v0.0.4) |
| Interaction range | `6 m` | Planned (v0.0.4) |
| Eye height | `1.62 m` | Planned (v0.0.4) |
| Player AABB | `0.6 m` wide, `1.8 m` tall | Planned (v0.0.4) |

## Not bound

| Action | Status |
|---|---|
| Sprint | No binding. Fly and walk speeds are fixed. |
| Crouch | No binding. |
| Mouse-wheel hotbar | No binding. v0.0.4 selects with `1`–`5`. |
| Pause / menu | No binding. `Escape` only releases the cursor. |
| Gamepad | No input support today; not scheduled. |

## Keybindings

- Current bindings are hardcoded to physical key codes and mouse buttons. Physical key codes
  keep WASD in the same positions across keyboard layouts.
- Planned: an action map and config layer will make bindings remappable. No milestone is
  assigned yet; see [Configuration](configuration.md) for the planned file.
- Until the action map lands, changing a binding means changing code. The tables above are the
  supported defaults.

## Later milestones

- v0.0.6: `T` advances the day/night clock by one hour.
