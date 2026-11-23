# Controls

Keyboard and mouse bindings for the current engine and the scoped milestones. Sections marked
**planned** come from scope pages and are not implemented yet. Camera angle conventions are in
[Coordinates](coordinates.md#camera-and-direction).

| Area | Status |
|---|---|
| Fly-camera bindings | Historical (v0.0.2) |
| Chunk boundary demo toggle | Historical (v0.0.3) |
| Walking, interaction, hotbar, save | Current (v0.0.4, legacy) |
| Basic mechanics: items, tools, drops, player save | Current (0.0.10.1, demo) |
| HUD: crosshair, hotbar, interaction prompt | Current (0.0.10.2, demo) |
| Inventory screen | Current (0.0.10.2, demo) |
| Vsync toggle | Current (v0.0.4) |
| FPS/frame-time overlay | Current (v0.0.4) |
| Day/night debug key (`T`) | Current (v0.0.6) |
| Unlit debug shading (`F4`) | Current (v0.0.6) |
| Debug fly (`F`) | Current (debug) |
| Remappable bindings | Planned (`0.0.10.5`) |

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

## Interaction (legacy, v0.0.4)

The default legacy chunk world still uses this path. The v1 demo replaces it with the 0.0.10
interaction below.

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
| Hold left mouse | Break the targeted block after its hardness time | Current (demo) |
| Right mouse | Place the selected item in the adjacent cell | Current (demo) |
| `1`-`9` | Select a hotbar slot | Current (demo) |
| `E` | Open or close the inventory | Current (demo) |
| `Escape` | Close the inventory if open; otherwise open the pause screen | Current (demo) |
| `Shift` while moving | Sprint while stamina remains | Current (demo) |
| `Space` | Jump; costs 5 stamina | Current (demo) |
| `R` | Respawn after death | Current (demo) |
| Arrow keys | Move the inventory focus | Current (demo) |
| `Enter` / `Space` | Activate the focused inventory slot | Current (demo) |
| Left click | Pick up, place, or swap a stack while the inventory is open | Current (demo) |
| Right click | Take half a stack, or place one item | Current (demo) |
| Shift-click | Move a stack between the hotbar and the main grid | Current (demo) |
| `Q` | Drop one item from the selected slot | Planned (`0.0.10.2`/`0.0.10.3`) |
| `Ctrl+S` | Save the world and player state | Current |

### Breaking

- The first left click after the cursor is released only captures the cursor. A later press starts
  a break.
- Keep holding: the outline fills from pale yellow to hot orange as progress reaches the block's
  hardness time. Release, lose focus, press `Escape`, or change the target to cancel.
- The ray reaches `6 m` and only targets resident, solid cells. The demo's bottom layer at `y = 0`
  is the world floor and cannot be broken.
- A completed break applies one durability point to the selected tool, removes the block, spawns
  its drops at the block centre, and lets you walk over them to pick them up.
- A drop below the block's minimum tool tier breaks without yielding an item. A wrong tool class
  does not satisfy the tier even when the tier number is high enough.

### Placing

- Right click places the selected block item in the cell on the entered face.
- The new block must not intersect the player, must be in a resident column, and must be air or a
  replaceable block. A non-air replaceable block is cleared first.
- `core:plant` needs a solid block directly below. A tool, material, or food item cannot be placed.
- One item is consumed only after the world edit succeeds.

### Items and hotbar

- New demo runs start with an empty inventory. Punch wood to begin the crafting chain.
- `1`-`9` select a hotbar slot. The HUD shows the nine slots, the item icon, stack count, and tool
  durability. The selected slot has a white border.
- Pickup radius is `1.5 m`. A full inventory leaves the remainder on the ground.
- There is no drop key, health, stamina, or audio yet.

## HUD (0.0.10.2, demo)

- A small crosshair sits at the centre of the screen.
- The nine-slot hotbar is anchored at the bottom centre. Empty slots show their number; occupied
  slots show a flat-colour placeholder icon, the stack count, and a green durability bar for tools.
- When a demo block is targeted, a prompt above the survival bars reads `Hold left: break the
  targeted block | Right: place`.
- The existing `F3` bitmap overlay still shows FPS and frame time. The HUD does not duplicate it.
- Health, stamina, and air bars are drawn above the hotbar, with a short red damage flash. Health and
  stamina persist in `VPLY` version 2.
- The HUD is non-interactive. It does not capture clicks or keys, so the gameplay bindings above
  continue to work.

## Inventory (0.0.10.2, demo)

- `E` opens the inventory, releases the cursor, and clears held movement keys. The HUD is hidden.
- `E`, `Escape`, or the `Close [E]` button closes it. The first `E` press only opens it; the next
  press closes it. Closing returns a held stack to the inventory and recaptures the cursor.
- Layout: three rows of nine main slots, a nine-slot hotbar row, and the offhand slot.
- Left click picks up a whole stack, places it, merges it, or swaps it. Right click takes half a
  stack, or places one item.
- Shift-click moves a stack between the hotbar and the main grid.
- Drag a stack to another slot and release to place it; release outside the grid to return it.
- Arrow keys move the focus. `Enter` or `Space` activates the focused slot. `Shift` with activation
  quick-moves.
- Hovering a slot shows its name, kind, count, and durability.
- If the inventory is full, a held stack keeps the screen open rather than being destroyed.

## Survival (0.0.10.4, demo)

- Holding `Shift` while moving sprints at `6.3 m/s` and drains stamina at 20 per second.
- Jumping costs 5 stamina. A jump or sprint is refused at zero stamina.
- Falling more than three blocks deals one damage per extra block. Water landings are safe.
- The eye cell is probed for water. Air lasts 15 seconds, then the player takes one damage per
  second. Lava and fire hooks use the same environment probe.
- At zero health gameplay freezes, the cursor releases, and `R` respawns at the nearest safe cell.
  The inventory is kept.
- The HUD shows health, stamina, and air below full, with a short damage flash.
- `Escape` opens the pause screen with Resume and Save & Quit. Pause freezes movement, the fixed
  tick, and the sky; `Escape` or Resume returns to play.
- The death screen offers Respawn (`R`/`Enter`) and Save & Quit.

## Crafting (0.0.10.3)

- The inventory screen gains a 2x2 crafting grid and output slot.
- Right-click a placed `core:workbench` opens the 3x3 workbench screen instead of placing a block.
- Click a craft slot to pick up or place; right-click splits or places one; shift-click moves a
  craft slot back to the inventory.
- Click the output to craft one; shift-click to craft as many as the grid and inventory allow.
- The known-recipe book is a panel with search, details, `Craft`, and `Craft x10`; there is no
  separate recipe-book key yet. The search field owns the keyboard while focused.
- `Salvage held` breaks one item from the cursor stack into its static returns.
- `E`/`Escape`/the close button close the station and return the grid and cursor to the inventory.
- Quality, rarity, disciplines, queues, and automation are not bound because they are not in
  Phase 1.

### Saving

- `Ctrl+S` flushes the terrain journal and writes `world/terrain-demo/player.dat` without stopping
  the save worker, so later edits still persist.
- Closing the window flushes and joins the terrain worker, then writes the player file.
- The player file stores position, view, inventory, selected hotbar slot, and dropped items. A
  corrupt file is moved to `player.bad`; a different seed is ignored. Stress and headless runs do
  not use it.

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
| Sprint | `Shift` while moving; drains stamina. |
| Crouch / sneak | No binding. |
| Mouse-wheel hotbar | No binding. `1`-`9` select in the demo; `1`-`5` in the legacy path. |
| Inventory (`E`), drop (`Q`) | `E` opens the inventory. `Q` is planned; no drop binding yet. |
| Pause / menu | `Escape` opens the pause screen; Resume or `Escape` returns to play. |
| Gamepad | No input support today; not scheduled. |

## Terrain demo

Start with `TERRAIN_DEMO=1` (and `ENGINE_STRESS_SCENE=1` when `world/seed` is absent). The
movement, interaction, hotbar, and save bindings are the 0.0.10 table above. The demo also has the
world-repair prompt:

| Input | Effect |
|---|---|
| `R` / `Q` / `S` | Restore backup / quarantine / salvage the corrupt section shown in the repair prompt |
| Left / middle / right click | The same three repair actions, split across the window width |
| Left/middle/right click in the repair prompt | Same three repair actions |

`Ctrl+S` flushes the save worker; closing the window does the same. Tile paging, LOD selection, and
light/mesh rebuilds are automatic. See [Terrain v1 runtime](terrain-v1.md).

## Keybindings

- Current bindings are hardcoded to physical key codes and mouse buttons. Physical key codes
  keep WASD in the same positions across keyboard layouts.
- Planned for `0.0.10.5`: an action map and config layer will make bindings remappable. See
  [the 0.0.10.4-0.0.11 plan](plan-0.0.10.4-0.0.11.md) and [Configuration](configuration.md) for
  the planned file.
- Until the action map lands, changing a binding means changing code. The tables above are the
  supported defaults.

## Later milestones

- `0.0.10.4`: health and stamina bars, sprint and jump stamina costs, pause on `Escape`, the death
  overlay, fall damage, drowning, and safe respawn are implemented.
- `0.0.10.5`: kira audio, the settings screen, and remappable bindings.
- v0.0.6 onward: `Lamp` and `Water` are not on the hotbar. Water has its own transparent mesh
  pass and cannot be placed from the hotbar.
