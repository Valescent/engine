# Controls

Keyboard and mouse bindings for the current engine and the scoped milestones. Sections marked
**planned** come from scope pages and are not implemented yet. Camera angle conventions are in
[Coordinates](coordinates.md#camera-and-direction).

| Area | Status |
|---|---|
| Fly-camera bindings | Historical (v0.0.2) |
| Chunk boundary demo toggle | Historical (v0.0.3) |
| Walking, interaction, hotbar, save | Current (v0.0.4) |
| Vsync toggle | Current (v0.0.4) |
| FPS/frame-time overlay | Current (v0.0.4) |
| Day/night debug key (`T`) | Current (v0.0.6) |
| Unlit debug shading (`F4`) | Current (v0.0.6) |
| Debug fly (`F`) | Current (debug) |
| Remappable bindings | Planned, no milestone |

## Bindings by milestone

| Input | v0.0.2 | v0.0.3 | v0.0.4 (current) |
|---|---|---|---|
| `W` / `S` | Fly forward / back | Fly forward / back | Walk forward / back |
| `A` / `D` | Strafe left / right | Strafe left / right | Walk left / right |
| Mouse move | Look | Look | Look |
| Left click | Capture the cursor | Capture the cursor | Capture first; break when captured |
| Right click | — | — | Place the selected block |
| `Space` | Toggle the demo voxel | Toggle the boundary demo voxel | Jump |
| `1`–`5` | — | — | Select stone, dirt, grass, wood, leaves |
| `Ctrl+S` | - | - | Save all non-empty chunks; also flushes `world/terrain-demo` |
| `V` | — | — | Toggle vsync |
| `F3` | — | — | Toggle the FPS/frame-time overlay |
| `Escape` | Release the cursor | Release the cursor | Release the cursor |

Mouse look requires capture and window focus in every milestone.

v0.0.4 replaces flight with walking. `W` / `S` use the camera yaw only, so looking up or down does
not change the walk direction. The v0.0.3 demo toggle is not carried forward on `Space`.

## Debug fly

`F` toggles a debug flight mode for inspecting terrain: gravity and collision are off, `Space`
ascends, `Ctrl` descends, and `WASD` moves horizontally at `12 m/s`. Leaving the mode restores
walking immediately (you will fall). The toggle is logged at `info` level. It is a debug facility,
not a gameplay mechanic, and it is not part of any milestone's Definition of Done.

## Current capture and focus (v0.0.4)

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
- WASD walks the player while the window is focused, with or without capture.
- Key repeat is ignored. Holding `Space` does not repeat the jump.
- `V` switches between FIFO presentation (vsync) and immediate or mailbox presentation when the
  surface supports it, falling back to FIFO otherwise. The engine starts with vsync on.
- `F3` toggles the debug overlay. It shows FPS and frame time from the previous one-second sample.
- `T` adds one hour to the day/night cycle (`1/24` of a 600-second day). `F4` toggles the v0.0.4 unlit debug shading path.
- Movement uses a frame delta clamped to `100 ms`. Collision is swept along each axis, so a
  stalled frame cannot tunnel through terrain.

## Interaction (current, v0.0.4)

- Block interaction requires the cursor to be captured.
- The first left click after the cursor is released captures only; it does not break or place.
- Once captured, left click breaks the targeted block and right click places the selected block
  in the adjacent cell.
- Capture is acquired by left click only. Right click is not a capture button.
- Break and place are click actions; hold-to-repeat is not implemented.
- Placement is rejected when the cell is occupied or the new block would intersect the player.
- There is no pause menu. `Escape` still only releases the cursor.

## Interaction (0.0.10, in progress)

The v1 terrain demo (`TERRAIN_DEMO=1`) uses the new interaction path. The legacy chunk world keeps
the v0.0.4 click-to-break behaviour until the demo becomes the default.

| Input | Effect | Status |
|---|---|---|
| Hold left mouse | Break the targeted block after its hardness time | In progress |
| Right mouse | Place the selected item in the adjacent cell | In progress |
| `1`-`9` | Select a hotbar slot | In progress |
| `E` | Open or close the inventory | Planned (`0.0.10.2`) |
| `Q` | Drop one item from the selected slot | Planned (`0.0.10.2`) |
| `Escape` | Close the open screen, otherwise release the cursor | Planned (`0.0.10.2`) |
| `Ctrl+S` | Save the world and player state | In progress |

Breaking requires the cursor to stay captured. Releasing the mouse cancels progress. A block whose
minimum tool tier is above the held tool breaks without a drop.

## Tuning

| Parameter | Value | Status |
|---|---|---|
| Mouse sensitivity | `0.0025 rad/px` | Current |
| Walk speed | `4.5 m/s` | Current (v0.0.4) |
| Jump velocity | `8 m/s` | Current (v0.0.4) |
| Gravity | `24 m/s²` | Current (v0.0.4) |
| Interaction range | `6 m` | Current (v0.0.4) |
| Eye height | `1.62 m` | Current (v0.0.4) |
| Player AABB | `0.6 m` wide, `1.8 m` tall | Current (v0.0.4) |

## Not bound

| Action | Status |
|---|---|
| Sprint | No binding. Walk speed is fixed. |
| Crouch | No binding. |
| Mouse-wheel hotbar | No binding. v0.0.4 selects with `1`–`5`. |
| Pause / menu | No binding. `Escape` only releases the cursor. |
| Gamepad | No input support today; not scheduled. |

## Terrain demo

Start with `TERRAIN_DEMO=1` (and `ENGINE_STRESS_SCENE=1` when `world/seed` is absent). The demo
keeps the normal movement bindings and adds:

| Input | Effect |
|---|---|
| Left click | Break the targeted block |
| Right click | Place the selected block |
| `R` / `Q` / `S` | Restore backup / quarantine / salvage the corrupt section shown in the repair prompt |
| Left/middle/right click in the repair prompt | Same three repair actions |

`Ctrl+S` flushes the save worker; closing the window does the same. Tile paging, LOD selection, and
light/mesh rebuilds are automatic. See [Terrain v1 runtime](terrain-v1.md).

## Keybindings

- Current bindings are hardcoded to physical key codes and mouse buttons. Physical key codes
  keep WASD in the same positions across keyboard layouts.
- Planned: an action map and config layer will make bindings remappable. No milestone is
  assigned yet; see [Configuration](configuration.md) for the planned file.
- Until the action map lands, changing a binding means changing code. The tables above are the
  supported defaults.

## Later milestones

- v0.0.6 onward: `Lamp` and `Water` are not on the hotbar. Water has its own transparent mesh
  pass and cannot be placed from the hotbar.
