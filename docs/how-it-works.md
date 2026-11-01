# How it works

## Reading the save

Elden Ring's PC save, and Seamless Co-op's `.co2` copy of it, is an unencrypted BND4 container:

- Entries 0 to 9 are character slots. Each one is a 16-byte MD5 checksum followed by 0x280000 bytes of slot data.
- Entry 10 (`USER_DATA010`) holds the profile. Byte 0x1954 starts 10 one-byte "slot in use" flags.

Inside a slot, `er_progress.py` walks the same sequence of sections that ER-Save-Editor's `SaveSlot::read` does. Most sections have a fixed size, but a few don't:

- the item table at the start, where each entry is 8, 16, or 21 bytes depending on item type
- the equipped projectiles and unlocked regions, which are count-prefixed lists

The walk ends at the event-flag block (0x1BF99F bytes), and that block is what we read.

After the event flags, the reader keeps walking to a value the editor expects to be 2 for a live slot. If that value isn't 2, the layout has changed (usually a game or mod update), and the reader stops with "slot layout mismatch" instead of reporting wrong progress.

## Flags

The game records each boss kill and each discovered Site of Grace as one bit in the event-flag block. `er_tables.json` has:

- `bosses` and `graces`: flag ID, name, and region for each one
- `flag_offsets`: for each flag ID, the byte offset and bit number in the block (bit `n` means `1 << n`)

A region counts as reached if any of its graces is set. Regions are listed in the order the grace table uses, which roughly follows the game's progression.

## Updating the tables

The tables come from ER-Save-Editor's `src/db/bosses.rs`, `graces.rs`, and `event_flags.rs`. To rebuild them, for example once the editor adds Shadow of the Erdtree:

1. Download those three files from the repository into an empty folder.
2. Run `python -I tools/build_tables.py <that folder> er_tables.json`.

The script prints the counts and any flag IDs that have no offset. A flag with no offset is left out of `flag_offsets`, and the reader would fail on it, so fix the source data before using the new tables.

## Sync

`er_sync.py` does these steps in order:

1. **Read the save** into a snapshot: player, character, level, bosses (name and region), regions, grace count, and the save's modified time.
2. **Pull** with `git pull --rebase --autostash`, after discarding any local changes to `PROGRESS.md` and `LOG.md`, since those files are always regenerated.
3. **Compare** the snapshot with `progress/<player>.json` as of the last commit (not the working copy). The committed file is the record of the last sync, so there is no separate state to keep. If the level, bosses, and regions all match, it stops here.
4. **Write** `progress/<player>.json`, and write `log/<player>.jsonl` as its committed version plus one new entry. **Rebuild** `PROGRESS.md` from every file in `progress/` and `LOG.md` from every file in `log/`, then commit all four.
5. **Push.** If the push is rejected, it undoes the commit, keeping the player's own files, pulls again, rebuilds, and recommits. It tries up to three times.

Only a player's own sync writes their progress and log files, so rebases never conflict on those files. `PROGRESS.md` and `LOG.md` are the only shared files, and they're rebuilt after every pull instead of merged. That's why the joint log is generated: if everyone appended to one `LOG.md`, two syncs at the same moment would conflict.

Building the new files from the committed versions means a sync that fails partway, for example when every push is rejected, leaves nothing the next sync can trip over. The next sync starts again from the last commit and adds exactly one log entry.

A log entry holds the time (the save's modified time, in that player's local time), the player, the level before and after, the new bosses and regions, the running totals, and two flags: `first_sync`, and `lost` for a save that has less progress than the last sync.

Without an upstream branch, or with `--no-push`, it skips steps 2 and 5 and commits locally.

## Recovery

**PROGRESS.md or LOG.md looks wrong or has a merge conflict.** Run `python -I er_sync.py render` and commit the result. They're built entirely from `progress/*.json` and `log/*.jsonl`.

**A save from a different PC, a restored backup, or a new character.** A sync compares against the last committed progress file:
- Bosses missing from the save don't count as "new", but the progress file still gets overwritten with the save's real state, so the counts can go down.
- To make the next sync count as a first sync, delete `progress/<player>.json`, commit, and sync again. The log keeps its history, and the next entry says "First sync".

**A rename.** Changing `player` in `config.json` starts new progress and log files. Delete the old progress file in the same commit. To keep the old log entries, rename the old log file to the new name and change `player` in each of its lines.
