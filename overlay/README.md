# Catch-up overlay

An in-game overlay for Elden Ring that shows the "Catch-up" list from [PROGRESS.md](../PROGRESS.md): the bosses and regions the others have done that you haven't, grouped by region, with who did each one. Press F9 to show or hide it.

It reads only this repo's JSON files (`progress/*.json`, `config.json`, `er_tables.json`) from your clone. It doesn't read or write game memory or your save, so game patches shouldn't break it. When the files change, for example after a `git pull` or a sync, it updates within 2 seconds.

It's optional and local: other players don't need it, and it doesn't change anything they see.

## Build

You need [Rust](https://rustup.rs/), 1.86 or later, with the MSVC toolchain (the Windows default). Then run:

```
cd overlay
cargo build --release
```

This builds `overlay\target\release\catchup_overlay.dll`. Only one person has to build it; the DLL works on any of our PCs.

To check what it will show without starting the game:

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

On first start, the overlay creates `catchup_overlay.ini` next to the DLL and shows where it is. Set `repo`, then restart the game.

| Setting | Default | What it does |
|---|---|---|
| `repo` | (empty) | Full path to your clone of this repo. Required. |
| `player` | from `config.json` | Your name as it appears in `PROGRESS.md`. Only needed if the clone has no `config.json`. |
| `toggle_key` | `F9` | Key that shows and hides the overlay: `F1` to `F12`, or a single letter or digit. |
| `start_visible` | `0` | `1` shows the overlay as soon as the game starts. |
| `corner` | `top-right` | `top-left`, `top-right`, `bottom-left` or `bottom-right`. |
| `font_size` | `0` | Text height in pixels. `0` scales with the game's resolution. |
| `max_lines` | `25` | Most boss and region lines to list before "and N more". |

## Keeping it current

- **Other players' progress** is only as fresh as your clone. `er_sync.py` pulls when you sync; otherwise run `git pull` before playing.
- **Your own progress** is only as fresh as your last sync. A boss you beat this session stays on the list until you quit to the menu and run `er_sync.py`.

The footer shows when each player last synced, so you can tell when the list is old.

## How it works

- `src/lib.rs`: hooks the game's DirectX 12 rendering with [hudhook](https://github.com/veeenu/hudhook), draws the window with Dear ImGui, polls the toggle key while the game has focus, and exports `DirectInput8Create` for the `dinput8.dll` install.
- `src/catchup.rs`: loads the JSON files and works out the catch-up list, using the same rules as `er_sync.py render`.
- `src/view.rs`: turns the result into the lines the overlay draws.
- `src/settings.rs`: reads `catchup_overlay.ini`.
- `src/bin/catchup_preview.rs`: prints the same lines in a terminal.

The overlay ignores the mouse and keyboard apart from the toggle key, so it never takes input away from the game.

See [docs/overlay-plan.md](../docs/overlay-plan.md) for the design and what v2 could add (your own bosses crossed off live, a pointer to the next boss).
