# Catch-up overlay

An in-game overlay for Elden Ring for catching up with the rest of the group:

- **During play** it shows only a compass pointing at the boss you picked, with a rough distance. With no target set, the screen stays clear.
- **A menu** holds the "Catch-up" list from [PROGRESS.md](../PROGRESS.md): the bosses and regions the others have done that you haven't, grouped by region, with who did each one. You pick the compass target there.

The list comes from this repo's JSON files (`progress/*.json`, `config.json`, `er_tables.json`) in your clone. When those files change, for example after a `git pull` or a sync, it updates within 2 seconds. The compass reads your position and the camera direction from game memory, and the overlay reads the listed bosses' kill flags there to notice a kill as it happens. It never writes to the game or your save.

It's optional and local: other players don't need it, and it doesn't change anything they see.

## Using it

Press **F9**, or on a controller **hold Y and press View** (Triangle and Create on a PlayStation controller), to open the menu. It opens in the middle of the screen over the dimmed game. With nothing to catch up on, for example before anyone else has synced, it says so and only offers Back.

| In the menu | Keyboard | Controller |
|---|---|---|
| Move | Up/Down arrows, Page Up/Down | D-pad or left stick, LB/RB to jump |
| Set the compass target | Enter | A (Cross) |
| Clear the target | Enter on the current target | A on the current target |
| Close | Esc, Backspace or F9 | B (Circle) or the combo |

Picking a target closes the menu. While the menu is open, the game doesn't see your keyboard, mouse or controller, so moving through the list doesn't switch your items or make you jump. The game keeps running, as in its own menus.

The HUD shows:
- **With a target:** a compass. The arrow points at the boss relative to the camera, so straight up means "ahead", with the distance and region under the name.
- **Without a target:** nothing, except a short hint at start, after closing the menu, or when the list changes.

Your target is saved in `catchup_overlay_target.txt` next to the DLL, so it's still set next session. Once you've beaten the boss, it drops off the list, and the overlay clears the target and says so. You don't need to sync first.

### The compass

How precise it is depends on where the boss is:

| Boss | The arrow points at | How close |
|---|---|---|
| Open world (75 bosses) | The 256 m map tile the boss is in, taken from its kill flag | Within about 128 m. Under 150 m it says "nearby" instead of pointing. |
| Caves, catacombs, tunnels and legacy dungeons such as Stormveil | Where the game's own map places that dungeon, from its `WorldMapLegacyConvParam` table | The dungeon, not the boss room |
| Underground (Siofra, Ainsel, Deeproot, Lake of Rot) | Nothing: the underground has no overworld position | "No compass in this area" |

The menu shows each boss's distance too, so you can pick the nearest.

The compass works with **Elden Ring 2.7.2.0** only. Its game-memory layouts come from [fromsoftware-rs](https://github.com/vswarte/fromsoftware-rs), pinned to a commit that supports that version. After a game patch, the compass shows that it needs a different version, and the list keeps working. To fix it, update the `rev` in `Cargo.toml` to a fromsoftware-rs commit that supports the new version, then rebuild.

If the arrow seems to point the wrong way, set `compass_debug = 1`. That adds your map, coordinates, the target's coordinates and the camera direction under the compass, which shows where the mismatch is.

### Controllers

The combo is the same physical buttons on any controller: the top face button plus the small button left of center. Hints use the names on the controller you're holding.

| Controller | Combo | How the overlay reads it |
|---|---|---|
| Xbox, or anything Windows sees as one | Y + View | XInput |
| DualSense (PS5) | Triangle + Create | XInput when Steam Input is on, otherwise straight from the controller |
| DualShock 4 (PS4) | Triangle + Share | Same as DualSense |
| Others, through Steam Input | Whatever Steam maps to Y and View | XInput |

The overlay picks the names automatically. With only a PlayStation controller plugged in, it uses PlayStation names. With an Xbox controller too, it follows whichever you pressed last. To fix the names instead, set `controller_style = xbox` or `playstation`.

**Keeping input away from the game.** Elden Ring has no Y + View action. While you hold Y, the overlay keeps View from reaching the game, so the combo doesn't open the map. While the menu is open, the game gets an idle controller and no keyboard or mouse input. After the menu closes, the game stays blocked until you let go of the keys and buttons you used, so closing with B doesn't also backstep.

This works by hooking `XInputGetState` and DirectInput's `GetDeviceState` and `GetDeviceData`, which is how the game reads the controller, keyboard and mouse. The overlay hides input only during the combo and while the menu is open. If you'd rather it didn't touch controller input at all, set `controller_combo = off` and use the key.

A PlayStation controller with Steam Input off is the one exception. The game then reads it directly, so the overlay can read it but can't hide it from the game.

**Changing the combo.** Use `controller_combo`, with Xbox or PlayStation names (`Triangle + Create` and `Y + View` are the same combo). Pick a first button that does nothing when held on its own away from interactables; Y/Triangle (Event action) works well. Avoid combos the game already uses: Y with a bumper, trigger or D-pad direction.

## Build

You need:
- [Rust](https://rustup.rs/), 1.86 or later, with the MSVC toolchain (the Windows default).
- Internet access for the first build. Cargo downloads fromsoftware-rs from GitHub at the pinned commit.

Then run:

```
cd overlay
cargo build --release
```

This builds `overlay\target\release\catchup_overlay.dll`. Only one person has to build it; the DLL works on any of our PCs.

To check what the list will show without starting the game:

```
cargo run --release --bin catchup_preview -- ..
```

## Install

Pick one of these.

### With ModEngine2 (recommended)

[ModEngine2](https://github.com/soulsmods/ModEngine2) is the usual way to load Seamless Co-op together with other DLL mods.

1. Unzip ModEngine2 somewhere.
2. Put `catchup_overlay.dll` next to `config_eldenring.toml`.
3. In `config_eldenring.toml`, list both Seamless Co-op and the overlay under `external_dlls`, with doubled backslashes:

   ```toml
   external_dlls = [
       "C:\\Program Files (x86)\\Steam\\steamapps\\common\\ELDEN RING\\Game\\SeamlessCoop\\ersc.dll",
       "catchup_overlay.dll",
   ]
   ```

4. Start the game with `launchmod_eldenring.bat` instead of `ersc_launcher.exe`.

Try Seamless Co-op alone under ModEngine2 first. If that works, add the overlay.

### As dinput8.dll

The game loads any `dinput8.dll` in its own folder, so this way you keep starting with `ersc_launcher.exe`.

1. Copy `catchup_overlay.dll` to `ELDEN RING\Game\dinput8.dll`. The overlay forwards the game's DirectInput calls to the real Windows `dinput8.dll`.
2. Start the game with `ersc_launcher.exe` as usual.

Two caveats:

- **The DLL also loads when you start the game normally from Steam, with EasyAntiCheat on.** Delete or rename `dinput8.dll` before playing online without Seamless.
- **Only one mod can be `dinput8.dll`.** If you already use one, such as Elden Mod Loader, use ModEngine2 instead.

## Settings

On first start, the overlay creates `catchup_overlay.ini` next to the DLL and shows where it is. Set `repo`, then restart the game. If the ini has a mistake, the overlay lists the bad lines in the corner until you fix them.

| Setting | Default | What it does |
|---|---|---|
| `repo` | (empty) | Full path to your clone of this repo. Required. |
| `player` | from `config.json` | Your name as it appears in `PROGRESS.md`. Only needed if the clone has no `config.json`. |
| `toggle_key` | `F9` | Key that opens and closes the menu: `F1` to `F12`, or a single letter or digit. |
| `controller_combo` | `Y + View` | Opens and closes the menu: hold the first button, press the second. Xbox names: `A B X Y LB RB L3 R3 View Menu Up Down Left Right`. PlayStation names: `Cross Circle Square Triangle L1 R1 L3 R3 Create Share Options`. `off` turns it off. |
| `controller_style` | `auto` | Button names in hints: `auto` (the controller you're using), `xbox` or `playstation`. |
| `corner` | `top-right` | Where the compass and menu go: `top-left`, `top-right`, `bottom-left` or `bottom-right`. |
| `opacity` | `0.75` | Background opacity, from `0.2` (see-through) to `1` (solid). |
| `font_size` | `0` | Text height in pixels. `0` scales with the game's resolution. |
| `compass_debug` | `0` | `1` shows map and coordinates under the compass. |

## Troubleshooting

The overlay writes `catchup_overlay.log` next to the DLL each time the game starts. The log records:
- whether it hooked the game's drawing and input
- whether the compass can work with this game version
- each time it loads progress
- the first frame it drew, and resolution changes after that
- a frozen game: "Frame stuck for N s while <step>" means the overlay's own code hung in that step; "No frame for N s; the last one finished normally" means the game stopped drawing with the overlay idle, so the stall is in the game, the driver or another hook (the Steam or NVIDIA overlay). Minimizing the game also logs the second one, followed by "Frames resumed" when it comes back.

If nothing shows up, press F9 first: with no compass target, the overlay only shows a short hint at start and when you load in. If F9 does nothing either, the log says how far the overlay got.

If the log stops after the "Keyboard and mouse blocking hooked" line and the game closed within a few seconds, the overlay didn't fail: its next line, "Compass ready", only comes once the game has finished starting. The game was shut down before that, almost always by Seamless Co-op because Steam wasn't running. See "If the game won't start" in the [main README](../README.md).

## Keeping it current

- **Other players' progress** is only as fresh as your clone. `er_sync.py` pulls when you sync; otherwise run `git pull` before playing.
- **Your own progress** comes from your last sync, plus the bosses the overlay has seen you beat since. It sees a kill two ways, both from the game's kill flags:
  - A listed boss dies while you're playing. It comes off the list within a second.
  - A listed boss is already dead in the first world you load after starting the game, which is always your own. This covers kills from an earlier session that you haven't synced.

  In a friend's world the flags are the friend's, so a boss that was already dead when you joined stays on your list. The overlay forgets what it saw when the game closes, and `PROGRESS.md` and the other players only change when you run `er_sync.py`. Regions you've reached still wait for a sync.

The menu shows when each player last synced, so you can tell when the list is old.

## How it works

- `src/lib.rs`: hooks the game's DirectX 12 rendering with [hudhook](https://github.com/veeenu/hudhook), draws the HUD and menu with Dear ImGui, handles menu input, and keeps the compass target.
- `src/compass.rs`: where bosses and the player are on the overworld (from kill flags, map tiles and the dungeon table), and the arrow's angle.
- `src/game.rs`: checks the game version, then reads the player's map and position, the camera and the dungeon table from game memory. It copies values out with `ReadProcessMemory`, so a stale pointer during a loading screen gives an error instead of a crash. Kill flags are the exception: they go through fromsoftware-rs's own lookup, and only while a character is loaded.
- `src/kills.rs`: decides which set kill flags are your kills, since in a friend's world the flags are the friend's.
- `src/pad.rs`: reads controllers through XInput and, for PlayStation controllers, straight from their HID reports. Works out which kind is in use, and hooks `XInputGetState` to hide the combo, and the whole controller while the menu is open.
- `src/dinput.rs`: hides keyboard and mouse from the game while the menu is open, and forwards `DirectInput8Create` for the `dinput8.dll` install.
- `src/catchup.rs`: loads the JSON files and works out the catch-up list, using the same rules as `er_sync.py render`.
- `src/view.rs`: turns the result into the menu's lines and the one-line hint.
- `src/settings.rs`: reads `catchup_overlay.ini`.
- `src/bin/catchup_preview.rs`: prints the hint line and the menu's list in a terminal.

See [docs/overlay-plan.md](../docs/overlay-plan.md) for the original design notes.
