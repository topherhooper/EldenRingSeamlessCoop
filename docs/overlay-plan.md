# In-game catch-up overlay: feasibility

Goal: an in-game overlay that lists the bosses and regions the others have done that I haven't, the same thing `PROGRESS.md`'s "Catch-up" section shows, so I can catch up without alt-tabbing. Later, possibly a live pointer to the next target.

**Status:** v1 is built. See [overlay/README.md](../overlay/README.md).

Short answer: v1, a toggleable list, is feasible and low risk, and it doesn't need to read game memory at all. A pointer is realistic for overworld bosses, roughly, using only data we already have. Dungeon bosses and exact positions need more data work.

## Hooking

Elden Ring renders only with DirectX 12. Overlay mods for it are DLLs loaded into the game process. They hook `IDXGISwapChain::Present`, plus `ID3D12CommandQueue::ExecuteCommandLists` to capture the game's command queue, and draw Dear ImGui each frame. Two existing ways to build one:

- **[hudhook](https://github.com/veeenu/hudhook)** (Rust, MIT). You implement `ImguiRenderLoop::render` and pass it to the `hudhook!` macro with `ImguiDx12Hooks`. It's the overlay layer of the [Elden Ring practice tool](https://github.com/soarqin/eldenring-practice-tool), so it's proven on this game. Recommended.
- **C++**: Dear ImGui's DX12 backend plus MinHook or kiero for the vtable hooks. Same result with more boilerplate. Choose it only if we'd rather not use Rust.

For game memory (needed only for v2), [fromsoftware-rs](https://github.com/vswarte/fromsoftware-rs) has an `eldenring` crate with typed game structures. It finds singletons through RTTI instead of hard-coded addresses, so it survives patches better. Its license is MIT/Apache with an added clause that bars using the code to train machine-learning models. Using it in a mod isn't affected.

### Getting the DLL loaded

We currently start the game with `ersc_launcher.exe`, which injects `SeamlessCoop/ersc.dll`. There's no ModEngine2 in the install. Options:

| Loader | How | Notes |
|---|---|---|
| **ModEngine2** | List both `SeamlessCoop\\ersc.dll` and `catchup_overlay.dll` in `external_dlls` in `config_eldenring.toml`, then launch through ME2 instead of `ersc_launcher.exe`. | The documented way to combine Seamless with DLL mods ([guide](https://www.nexusmods.com/eldenring/articles/94)). [GG Player List Overlay](https://www.nexusmods.com/eldenring/mods/8088), an overlay built for Seamless, installs this way. Recommended. |
| **`dinput8.dll` proxy** (Elden Mod Loader, or the overlay itself acting as the proxy) | Put the DLL in `Game\`. The game loads it whatever launcher starts it, so `ersc_launcher.exe` keeps working. | Least change to how we launch. Clashes with any other mod that also uses `dinput8.dll`. |
| **me3** (ModEngine2's successor) | `[[natives]]` entries in a profile. | Users have reported trouble getting Seamless to load under it ([discussion](https://github.com/garyttierney/me3/discussions/118)). Skip for now. |

## Compatibility with Seamless Co-op

- **Anti-cheat:** EasyAntiCheat doesn't run when the game is started through Seamless, and play goes over Seamless's own servers, not FromSoftware's. A read-only overlay doesn't change anything other players see.
- **Other overlays:** Steam's overlay, RTSS, and ReShade also hook `Present`. Hook libraries chain these hooks, and the practice tool coexists with Steam's overlay. ReShade or Special K are the likeliest to conflict, so test with whatever each of us runs.
- **Game and mod updates:** v1 only hooks DXGI and reads JSON files, so game patches shouldn't break it. v2 reads game structures, so a game patch can break it until fromsoftware-rs is updated.
- **Seamless's own guarantees:** Seamless doesn't promise compatibility with any other mod. Test Seamless alone under the new loader first, then add the overlay.
- **Per player:** The overlay is optional and local. Players who don't install it are unaffected. It doesn't touch `regulation.bin` or the save.

## Data

### v1: the repo's JSON files

The overlay gets the clone's path from a small `catchup_overlay.ini` next to the DLL, then reads:

- `progress/*.json`: every player's last synced progress.
- `config.json`: the `player` field says which file is "me".
- `er_tables.json`: boss order, so the list follows the same order as `PROGRESS.md`.

It reimplements `render()`'s catch-up logic from `er_sync.py`, which is about 15 lines:
- Missing bosses are every boss someone else has beaten, minus mine.
- Missing regions are every region someone else has reached, minus mine.
- Each entry says who has it.

It also picks the "furthest" player, the one with the most bosses, and shows "N bosses behind Arya". It reloads when a file's modified time changes.

Two limitations, both fine for v1:

1. **Others' progress is only as fresh as my last `git pull`.** `er_sync.py` pulls when you sync. A `git pull` before launching is enough. The overlay won't run git itself.
2. **My own progress is only as fresh as my last sync.** A boss I beat this session stays on the list until I sync. v2 fixes this with live flags, described next.

### v1.5 (optional): my progress, live

The game keeps event flags in memory, and fromsoftware-rs's `CSFD4VirtualMemoryFlag::get_flag(flag)` reads them. `er_tables.json` already has the flag ID for every boss and grace. So the overlay could check my own flags every few seconds and cross bosses off the moment I beat them, with no save read and no sync. This is the cheapest and most useful game-memory feature, so it should come before the pointer.

### v2: a live pointer

**Player position: available.** The `eldenring` crate exposes `PlayerIns::local_player()`. That has:
- `current_block_id`, the map block I'm in
- `block_position`, my position inside that block
- `ChrIns::chunk_position`, documented as "position in global map chunk coordinates"

My facing comes from the physics orientation, so a 2D arrow is doable.

**Boss coordinates: no public dataset, but the flag IDs give an approximate location.** I found no open dataset of boss world positions. Fan APIs, the Kaggle dataset, and Fextralife only have place names. [Elden Ring Compass](https://github.com/EthanShoeDev/elden-ring-compass) pins bosses on a map by extracting MSB map files from a game install. Its repo has no license, so we can't copy its data, but we could copy the method.

What we can use right away: Elden Ring boss flag IDs encode the map they belong to.

| Flag pattern | Map | Example | Count in `er_tables.json` |
|---|---|---|---|
| `10XXYY0800` | overworld small tile `m60_XX_YY_00` | Tree Sentinel `1042360800` → `m60_42_36`, the Church of Elleh tile | 75 of 157 |
| `AABB0800` | dungeon `mAA_BB` (m30 catacombs, m31 caves, m32 tunnels, m10–m19 legacy dungeons, m12 underground) | Margit `10000850` → `m10_00` (Stormveil) | 81 |
| `12XXYY0800` | unknown, probably an overworld tile at a different scale | Night's Cavalry Duo `1248550800` (Consecrated Snowfield) | 1 |

Small overworld tiles are 256 × 256 units, with each tile's local origin at its center. So for the 75 overworld bosses we already know where each one is to within about 128 m. That's enough for "Tree Sentinel: 600 m north-east" whenever you aren't standing on top of it, with no extra data.

What it would take beyond that:

- **Dungeon bosses (81):** point at the dungeon entrance instead. That needs about 60 entrance positions, either from `WorldMapLegacyConvParam` in `regulation.bin`, which maps dungeon coordinates onto the overworld, or a hand-made table from the in-game map. The underground (`m12`) is its own coordinate space, so its arrow only works while you're underground.
- **Exact positions:** unpack the game with UXM, read the boss enemies' positions from the MSBs with SoulsFormats or Smithbox, and convert them to overworld coordinates. That's real work, and tile accuracy is probably enough.
- **Drawing it:** an ImGui compass arrow plus a distance, relative to the player's facing. Rotating it with the camera instead needs the camera's rotation, another game structure. fromsoftware-rs's `debug-line` example shows the game can also draw lines in the world. Placing a marker on the game's own map means writing game memory, so leave that out.

Verdict: a tile-accuracy pointer for overworld bosses is realistic, at moderate effort, once v1.5 exists. Dungeon bosses need the entrance table first.

## Recommendation

### v1: a toggleable list (small)

- A Rust `cdylib` using `hudhook` with `ImguiDx12Hooks`, about 300 to 400 lines.
- Loaded through ModEngine2 `external_dlls` next to `ersc.dll`, or as a `dinput8.dll` proxy if we'd rather keep `ersc_launcher.exe`.
- A small semi-transparent window in a corner, hidden by default, toggled with a key set in the ini (default F9). It doesn't grab the mouse or keyboard, so it never takes input from the game.
- Content:
  - a header: "Toozy: 4 bosses behind Arya, 2 regions behind"
  - missing bosses grouped by region, in table order, each with who beat it
  - missing regions at the end
  - the time of each player's last sync, so it's obvious when the data is old
- It reads only the files described under Data, and makes no game-memory reads or writes. It doesn't touch the save, `er_sync.py`, or `er_progress.py`.
- Ships with a README section on building it (`cargo build --release`), the loader setup, and the ini.

### v2

1. **Live flags for me (v1.5):** bosses I beat drop off the list right away.
2. **Pointer for overworld targets:** take the boss tile from the flag ID and my position from `PlayerIns`, then draw a compass arrow and distance to the nearest or chosen target. Default target: the closest boss that the furthest player has beaten.
3. **Dungeon entrances:** a table so dungeon bosses get a pointer to their entrance.
4. **Nice to have:** a controller toggle, and showing the furthest player's next three targets instead of the whole list.

## Side finding: duplicate boss flags

`er_tables.json` (from ER-Save-Editor) gives three pairs of bosses the same flag. Beating one of a pair marks both as beaten, and the catch-up list would ask you to "catch up" on a boss you never fought:

| Flag | Bosses |
|---|---|
| `1036500800` | Alabaster Lord (Evergaol) [Limgrave] and Onyx Lord (Royal Grave Evergaol) [Liurnia] |
| `1042380850` | Bell Bearing Hunter (Warmaster's Shack) [Limgrave] and Deathbird (Warmaster's Shack) [Capital Outskirts] |
| `1050570800` | Death Rite Bird and Erdtree Avatar [Mountaintops of the Giants] |

This affects `PROGRESS.md` today, not just the overlay. Worth fixing in `tools/build_tables.py` or upstream.
