"""One-time: turn ER-Save-Editor's Rust tables (MIT/Apache) into er_tables.json."""
import json, re, sys

src, out = sys.argv[1], sys.argv[2]

def table_section(text):
    return text[text.index("HashMap::from(["):]

def parse(text, pattern):
    region, rows = None, []
    for line in table_section(text).splitlines():
        s = line.strip()
        if s.startswith("//"):
            region = s.lstrip("/").strip().replace("Weeping Weeping", "Weeping").replace("Leyendell", "Leyndell")
            continue
        m = re.search(pattern, s)
        if m:
            rows.append((region, m))
    return rows

bosses = [
    {"region": r, "flag": int(m.group(1)), "name": m.group(2)}
    for r, m in parse(open(f"{src}/bosses.rs", encoding="utf-8").read(), r'\(Boss::\w+,\s*\((\d+),\s*"([^"]+)"\)\)')
]
graces = [
    {"region": r, "flag": int(m.group(1)), "name": m.group(2)}
    for r, m in parse(open(f"{src}/graces.rs", encoding="utf-8").read(), r'\(Grace::\w+,\s*\(MapName::\w+,\s*(\d+),\s*"([^"]+)"\)\)')
]
offsets = {
    int(m.group(1)): [int(m.group(2), 16), int(m.group(3))]
    for m in re.finditer(r"\((\d+),\(0x([0-9a-f]+),(\d)\)\)", open(f"{src}/event_flags.rs", encoding="utf-8").read())
}
needed = {b["flag"] for b in bosses} | {g["flag"] for g in graces}
missing = needed - offsets.keys()
print(f"{len(bosses)} bosses, {len(graces)} graces, {len(offsets)} offsets, missing {sorted(missing)}")
json.dump({
    "source": "https://github.com/ClayAmore/ER-Save-Editor (MIT/Apache-2.0), base game only",
    "bosses": bosses,
    "graces": graces,
    "flag_offsets": {str(k): offsets[k] for k in sorted(needed - missing)},
}, open(out, "w", encoding="utf-8"), indent=1)
