"""Read Elden Ring (Seamless Co-op) progress from a PC save, read-only.

Prints JSON: per character, the bosses defeated and Sites of Grace found.
Slot layout and flag tables follow ClayAmore/ER-Save-Editor (MIT/Apache-2.0).

    python er_progress.py                 # auto-detect %APPDATA%\\EldenRing\\<id>\\ER0000.co2
    python er_progress.py path\\to\\ER0000.co2
"""
import glob, json, os, struct, sys

HERE = os.path.dirname(os.path.abspath(__file__))
SLOT_COUNT = 10
SLOT_SIZE = 0x280000
EVENT_FLAGS_SIZE = 0x1BF99F


def find_save():
    hits = glob.glob(os.path.join(os.environ["APPDATA"], "EldenRing", "*", "ER0000.co2"))
    hits += glob.glob(os.path.join(os.environ["APPDATA"], "EldenRing", "*", "ER0000.sl2"))
    if not hits:
        sys.exit("No Elden Ring save found under %APPDATA%\\EldenRing")
    return max(hits, key=os.path.getmtime)


class Reader:
    def __init__(self, data, pos):
        self.d, self.pos = data, pos

    def skip(self, n):
        self.pos += n

    def u32(self):
        v = struct.unpack_from("<I", self.d, self.pos)[0]
        self.pos += 4
        return v

    def i32(self):
        v = struct.unpack_from("<i", self.d, self.pos)[0]
        self.pos += 4
        return v


def bnd4_entries(data):
    if data[:4] != b"BND4":
        sys.exit("Not a BND4 save file")
    count = struct.unpack_from("<I", data, 0xC)[0]
    return [struct.unpack_from("<I", data, 0x40 + i * 0x20 + 0x10)[0] for i in range(count)]


def active_slots(data, entries):
    # USER_DATA010: active-slot bools at 0x1954, then 0x24C-byte profile summaries
    base = entries[10] + 0x10
    active = data[base + 0x1954: base + 0x1954 + SLOT_COUNT]
    return [i for i in range(SLOT_COUNT) if active[i]]


def read_slot(data, slot_start):
    """Walk a save slot up to its event flags; returns (name, level, flags bytes)."""
    r = Reader(data, slot_start)
    r.skip(4 + 4 + 0x18)  # ver, map id, unknown

    for _ in range(0x1400):  # GaItems: size depends on item type
        r.skip(4)
        item_id = r.u32()
        kind = item_id & 0xF0000000
        if item_id != 0 and kind == 0:
            r.skip(13)  # weapon
        elif item_id != 0 and kind == 0x10000000:
            r.skip(8)  # armor

    pgd = r.pos  # PlayerGameData (0x1B0)
    level = struct.unpack_from("<I", data, pgd + 0x60)[0]
    name = data[pgd + 0x94: pgd + 0xB4].decode("utf-16-le").split("\0")[0]
    r.skip(0x1B0)

    r.skip(0xD0)
    r.skip(0x58)  # EquipData
    r.skip(0x74)  # ChrAsm
    r.skip(0x58)  # ChrAsm2
    r.skip(4 + 0xA80 * 12 + 4 + 0x180 * 12 + 8)  # inventory
    r.skip(0xC * 8 + 0x10 + 4)  # EquipMagicData
    r.skip(0xA * 8 + 4 + 6 * 8 + 8)  # EquipItemData
    r.skip(6 * 4)  # equipped gestures
    r.skip(r.i32() * 8)  # equipped projectiles
    r.skip(39 * 4)  # EquippedItems
    r.skip(8)  # EquipPhysicsData
    r.skip(4 + 0x12F)  # unknown, face data
    r.skip(4 + 0x780 * 12 + 4 + 0x80 * 12 + 8)  # storage box
    r.skip(0x40 * 4)  # gestures
    r.skip(r.u32() * 4)  # unlocked regions
    r.skip(40)  # horse
    r.skip(1 + 0x40 + 12 + 0x1008 + 0x34)
    r.skip(8 + 0x1B58 * 16)  # GaItemData
    r.skip(0x408 + 0x1D)

    flags = data[r.pos: r.pos + EVENT_FLAGS_SIZE]
    r.skip(EVENT_FLAGS_SIZE + 1)

    # Sanity check: the editor asserts this value is 2 for a live slot
    for _ in range(5):
        r.skip(r.i32())
    r.skip(12 + 4 + 0x11 + 12 + 0x10 + 0xF)
    if r.u32() != 2:
        raise ValueError("slot layout mismatch (game or mod update?)")
    return name, level, flags


def read_progress(path):
    with open(path, "rb") as f:
        data = f.read()
    tables = json.load(open(os.path.join(HERE, "er_tables.json"), encoding="utf-8"))
    offsets = tables["flag_offsets"]

    def is_set(flags, flag_id):
        byte, bit = offsets[str(flag_id)]
        return bool(flags[byte] & (1 << bit))

    entries = bnd4_entries(data)
    characters = []
    for slot in active_slots(data, entries):
        name, level, flags = read_slot(data, entries[slot] + 0x10)  # skip MD5
        bosses = [b for b in tables["bosses"] if is_set(flags, b["flag"])]
        graces = [g for g in tables["graces"] if is_set(flags, g["flag"])]
        # Grace list order follows the game's progression, so keep first-seen order
        regions = list(dict.fromkeys(g["region"] for g in graces if not g["region"].startswith("Table of Lost Grace")))
        characters.append({
            "slot": slot,
            "name": name,
            "level": level,
            "bosses_defeated": [{"region": b["region"], "name": b["name"]} for b in bosses],
            "regions_reached": regions,
            "graces_found": len(graces),
        })
    return {"save": path, "modified": os.path.getmtime(path), "characters": characters}


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else find_save()
    sys.stdout.reconfigure(encoding="utf-8")
    json.dump(read_progress(path), sys.stdout, indent=1, ensure_ascii=False)


if __name__ == "__main__":
    main()
