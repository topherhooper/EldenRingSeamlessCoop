# Elden Ring Seamless Co-op progress sync

Tracks each player's progress in our Seamless Co-op run, so we can see who is behind before playing together.

**[PROGRESS.md](PROGRESS.md)** has:
- every player's level and boss and region counts
- a catch-up list of what each player is missing compared with the others
- a boss checklist by region

**[LOG.md](LOG.md)** is the joint log: one line per sync from every player, newest first, grouped by day. For example, "**9:40 PM · Arya**: Beat Margit, the Fell Omen and Godrick the Grafted. Reached Stormveil Castle. Level 24 → 31."

Progress is read straight from each player's save file. The scripts only read the save; they never write to it.

## What you need

- Windows PC with Elden Ring and the Seamless Co-op mod (the save is `%APPDATA%\EldenRing\<steam id>\ER0000.co2`).
- Python 3.8 or later. No extra packages.
- Git, with push access to this repo.

## Setup

1. Clone the repo.
2. Copy `config.example.json` to `config.json` (it's gitignored) and fill it in:

   | Field | What to put |
   |---|---|
   | `player` | Your name as it should appear in `PROGRESS.md`. It also names your file in `progress/`. |
   | `character` | Only if your save has more than one character: the character name to sync. Otherwise `null`. |
   | `save` | Only if your save isn't in the usual place: its full path. Otherwise `null`. |

3. Check what a sync would record:

   ```
   python -I er_sync.py --dry-run
   ```

## Starting the game

Run `play.bat` (double-click it, or make a desktop shortcut to it). It starts Steam if it isn't running, waits until Steam is logged in, then starts the game through Seamless Co-op's `ersc_launcher.exe`. Starting `ersc_launcher.exe` directly works too, as long as Steam is already running.

The game is found through your Steam libraries. If it's somewhere else:

```
powershell -ExecutionPolicy Bypass -File tools\play.ps1 -Game "D:\Games\ELDEN RING\Game"
```

### If the game won't start

- **The game closes a few seconds after launch, with no message.** Steam isn't running, or hasn't finished logging in; typical after a reboot when Steam isn't set to start with Windows. Seamless Co-op quits the game when it can't reach Steam. It leaves a small `.dmp` file in `ELDEN RING\Game\SeamlessCoop\crashdumps\reports` and nothing else. Start Steam first, or use `play.bat`.
- **With the overlay installed, `catchup_overlay.log` stops after the "Keyboard and mouse blocking hooked" line.** Same cause. The overlay's next line only appears once the game has finished starting, so it isn't a sign the overlay failed.

## Syncing

After a session, quit to the main menu or close the game so the save is current, then run:

```
python -I er_sync.py
```

It pulls, writes `progress/<you>.json`, adds an entry to `log/<you>.jsonl`, rebuilds `PROGRESS.md` and `LOG.md`, commits with a message like `Toozy: Lv 18 -> 25; beat Margit, the Fell Omen`, and pushes. If nothing changed since your last sync, it says so and does nothing. If someone else pushed at the same moment, it pulls and retries.

Other commands:

| Command | What it does |
|---|---|
| `python -I er_sync.py --dry-run` | Shows what's new since your last sync. Changes nothing. |
| `python -I er_sync.py --no-push` | Commits locally without pulling or pushing. |
| `python -I er_sync.py render` | Rebuilds `PROGRESS.md` and `LOG.md` from `progress/` and `log/`. |
| `python -I er_progress.py` | Prints every character in your save as JSON. |

## Files

| File | Purpose |
|---|---|
| `PROGRESS.md` | Generated summary. Don't edit by hand; the next sync overwrites it. |
| `LOG.md` | Generated joint log. Don't edit by hand. |
| `progress/<player>.json` | Each player's last synced progress. Only that player's sync writes it. |
| `log/<player>.jsonl` | Each player's log entries, one JSON object per sync. Only that player's sync writes it. |
| `er_progress.py` | Reads bosses, Sites of Grace, and level from a save. |
| `er_sync.py` | Compares the save with your last sync, updates the files, commits, and pushes. |
| `play.bat`, `tools/play.ps1` | Starts Steam if needed, then the game through Seamless Co-op's launcher. |
| `er_tables.json` | Boss and grace event-flag tables (base game). |
| `tools/build_tables.py` | Rebuilds `er_tables.json` from ER-Save-Editor's source files. |
| `docs/how-it-works.md` | Save format, sync details, recovery, and how to update the tables. |
| `overlay/` | Optional in-game overlay that shows your catch-up list. See [overlay/README.md](overlay/README.md). |

## Limits

- Base game only. Shadow of the Erdtree bosses and regions aren't in the tables yet.
- "Bosses" means the 157 bosses in the table, including field and dungeon bosses, not only main story bosses.
- A region counts as reached when you've found at least one Site of Grace in it.
- A game or mod update that changes the save layout makes the reader stop with "slot layout mismatch" instead of reporting wrong progress.

## Credits

The save layout and the flag tables come from [ClayAmore/ER-Save-Editor](https://github.com/ClayAmore/ER-Save-Editor) (MIT / Apache-2.0).
