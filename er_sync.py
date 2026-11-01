"""Sync this player's Elden Ring Seamless Co-op progress to the shared repo.

    python er_sync.py            # read save, update progress/ and log/, rebuild PROGRESS.md + LOG.md, commit, push
    python er_sync.py --dry-run  # show what's new since the last sync; change nothing
    python er_sync.py --no-push  # commit locally, don't pull or push
    python er_sync.py render     # rebuild PROGRESS.md and LOG.md from progress/ and log/
"""
import datetime, json, os, re, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import er_progress  # noqa: E402

CONFIG = os.path.join(HERE, "config.json")
PROGRESS_DIR = os.path.join(HERE, "progress")
LOG_DIR = os.path.join(HERE, "log")
PROGRESS_MD = os.path.join(HERE, "PROGRESS.md")
LOG_MD = os.path.join(HERE, "LOG.md")
GENERATED = ["PROGRESS.md", "LOG.md"]
PUSH_ATTEMPTS = 3


def load_config():
    if not os.path.exists(CONFIG):
        sys.exit("Missing config.json; copy config.example.json and fill it in")
    return json.load(open(CONFIG, encoding="utf-8"))


def slug(name):
    return re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")


def progress_path(player):
    return f"progress/{slug(player)}.json"


def log_path(player):
    return f"log/{slug(player)}.jsonl"


def date_text(iso):
    dt = datetime.datetime.fromisoformat(iso)
    return f"{dt:%A}, {dt:%b} {dt.day}, {dt.year}"


def time_text(iso):
    dt = datetime.datetime.fromisoformat(iso)
    return f"{dt.hour % 12 or 12}:{dt:%M} {dt:%p}"


def when_text(iso):
    dt = datetime.datetime.fromisoformat(iso)
    return f"{dt:%b} {dt.day}, {dt.year}, {time_text(iso)}"


# --- Git -----------------------------------------------------------------------

def git(*args, check=True):
    result = subprocess.run(["git", *args], cwd=HERE, capture_output=True, text=True, encoding="utf-8")
    if check and result.returncode != 0:
        sys.exit(f"git {' '.join(args)} failed:\n{result.stderr.strip()}")
    return result


def has_upstream():
    return git("rev-parse", "--abbrev-ref", "@{u}", check=False).returncode == 0


def pull():
    # Each player only writes their own progress and log files, so a rebase never conflicts on them.
    # The generated files are rebuilt after every pull, so local copies of them are thrown away.
    for name in GENERATED:
        git("checkout", "--", name, check=False)
    git("pull", "--rebase", "--autostash")


def committed(path):
    """A file's text as of HEAD, or None. Comparing against HEAD rather than the working copy
    means a sync that failed to push leaves nothing behind that the next sync trips over."""
    shown = git("show", f"HEAD:{path}", check=False)
    return shown.stdout if shown.returncode == 0 else None


# --- Snapshot and diff ---------------------------------------------------------

def read_snapshot(cfg):
    save = cfg.get("save") or er_progress.find_save()
    progress = er_progress.read_progress(save)
    chars = progress["characters"]
    if cfg.get("character"):
        chars = [c for c in chars if c["name"] == cfg["character"]]
    if not chars:
        sys.exit(f"No character named {cfg['character']!r} in the save" if cfg.get("character")
                 else "No characters in the save")
    char = chars[0]
    return {
        "player": cfg["player"],
        "character": char["name"],
        "level": char["level"],
        "bosses": char["bosses_defeated"],
        "regions": char["regions_reached"],
        "graces_found": char["graces_found"],
        "save_modified": datetime.datetime.fromtimestamp(progress["modified"]).isoformat(timespec="seconds"),
    }


def diff(prev, snap):
    old_bosses = {b["name"] for b in prev["bosses"]} if prev else set()
    old_regions = set(prev["regions"]) if prev else set()
    new_bosses = {b["name"] for b in snap["bosses"]}
    return {
        "first_sync": prev is None,
        "new_bosses": [b["name"] for b in snap["bosses"] if b["name"] not in old_bosses],
        "new_regions": [r for r in snap["regions"] if r not in old_regions],
        # Only after a restored backup or a different PC's save
        "lost": bool(old_bosses - new_bosses or old_regions - set(snap["regions"])),
        "previous_level": prev["level"] if prev else None,
        "level": snap["level"],
    }


def is_change(d):
    return (d["first_sync"] or d["lost"] or bool(d["new_bosses"] or d["new_regions"])
            or d["level"] != d["previous_level"])


def commit_message(player, d):
    parts = []
    if d["previous_level"] is None or d["level"] != d["previous_level"]:
        parts.append(f"Lv {d['level']}" if d["previous_level"] is None else f"Lv {d['previous_level']} -> {d['level']}")
    if d["new_bosses"]:
        parts.append("beat " + ", ".join(d["new_bosses"]))
    if d["new_regions"]:
        parts.append("reached " + ", ".join(d["new_regions"]))
    subject = f"{player}: " + ("first sync, " if d["first_sync"] else "") + "; ".join(parts)
    if len(subject) <= 72:
        return subject
    body = [f"Level {d['level']}"]
    if d["new_bosses"]:
        body += ["", "New bosses:"] + [f"- {b}" for b in d["new_bosses"]]
    if d["new_regions"]:
        body += ["", "New regions:"] + [f"- {r}" for r in d["new_regions"]]
    counts = plural(len(d["new_bosses"]), "new boss") + ", " + plural(len(d["new_regions"]), "new region")
    return f"{player}: " + ("first sync, " if d["first_sync"] else "") + counts + "\n\n" + "\n".join(body)


# --- PROGRESS.md ---------------------------------------------------------------

def load_all():
    if not os.path.isdir(PROGRESS_DIR):
        return []
    players = [json.load(open(os.path.join(PROGRESS_DIR, f), encoding="utf-8"))
               for f in sorted(os.listdir(PROGRESS_DIR)) if f.endswith(".json")]
    return sorted(players, key=lambda p: p["player"].lower())


def render():
    render_progress()
    render_log()


def render_progress():
    players = load_all()
    tables = json.load(open(os.path.join(HERE, "er_tables.json"), encoding="utf-8"))
    boss_order = {b["name"]: i for i, b in enumerate(tables["bosses"])}
    region_order = list(dict.fromkeys(b["region"] for b in tables["bosses"]))
    beaten = {p["player"]: {b["name"] for b in p["bosses"]} for p in players}
    reached = {p["player"]: set(p["regions"]) for p in players}
    boss_region = {b["name"]: b["region"] for p in players for b in p["bosses"]}

    out = ["# Elden Ring Seamless Co-op Progress", "",
           "Generated by `er_sync.py` from `progress/*.json`. Don't edit by hand. "
           "[LOG.md](LOG.md) has what everyone did, newest first.", "",
           "| Player | Level | Bosses | Regions | Last synced |", "|---|---|---|---|---|"]
    for p in players:
        out.append(f"| {p['player']} | {p['level']} | {len(p['bosses'])} | {len(p['regions'])} | "
                   f"{when_text(p['save_modified'])} |")
    if not players:
        out.append("| No one has synced yet | | | | |")

    out += ["", "## Catch-up", ""]
    if len(players) < 2:
        out += ["Shows up once two or more players have synced.", ""]
    for p in players:
        name, others = p["player"], [o for o in players if o["player"] != p["player"]]
        if not others:
            continue
        missing_bosses = sorted({b for o in others for b in beaten[o["player"]]} - beaten[name],
                                key=lambda b: boss_order.get(b, 1 << 30))
        missing_regions = [r for o in others for r in o["regions"] if r not in reached[name]]
        missing_regions = list(dict.fromkeys(missing_regions))
        if not missing_bosses and not missing_regions:
            out += [f"**{name}** is caught up with everyone.", ""]
            continue
        out += [f"**{name}** is behind on:", ""]
        for b in missing_bosses:
            who = ", ".join(o["player"] for o in others if b in beaten[o["player"]])
            out.append(f"- {b}, {boss_region[b]} (beaten by {who})")
        for r in missing_regions:
            who = ", ".join(o["player"] for o in others if r in reached[o["player"]])
            out.append(f"- Region: {r} (reached by {who})")
        out.append("")

    any_beaten = set().union(*beaten.values()) if beaten else set()
    if any_beaten:
        names = [p["player"] for p in players]
        out += ["## Bosses by region", ""]
        for region in region_order:
            rows = sorted((b for b in any_beaten if boss_region[b] == region), key=lambda b: boss_order.get(b, 1 << 30))
            if not rows:
                continue
            out += [f"### {region}", "", "| Boss | " + " | ".join(names) + " |",
                    "|---|" + "---|" * len(names)]
            for b in rows:
                out.append(f"| {b} | " + " | ".join("✓" if b in beaten[n] else "" for n in names) + " |")
            out.append("")

    with open(PROGRESS_MD, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(out).rstrip() + "\n")


# --- LOG.md --------------------------------------------------------------------

def plural(n, word):
    return f"{n} {word}" + ("" if n == 1 else "es" if word.endswith("s") else "s")


def join_names(items):
    return items[0] if len(items) == 1 else ", ".join(items[:-1]) + " and " + items[-1]


def log_sentence(e):
    if e["first_sync"]:
        return (f"First sync, at level {e['level']} with {plural(e['bosses_total'], 'boss')} beaten "
                f"and {plural(e['regions_total'], 'region')} reached.")
    parts = []
    if e["new_bosses"]:
        parts.append("Beat " + join_names(e["new_bosses"]) + ".")
    if e["new_regions"]:
        parts.append("Reached " + join_names(e["new_regions"]) + ".")
    if e["level"] != e["previous_level"]:
        parts.append(f"Level {e['previous_level']} → {e['level']}.")
    if e["lost"]:
        parts.append("Some progress went missing from the save (a restored backup or a different save?).")
    return " ".join(parts)


def load_log():
    entries = []
    if os.path.isdir(LOG_DIR):
        for name in sorted(os.listdir(LOG_DIR)):
            if name.endswith(".jsonl"):
                with open(os.path.join(LOG_DIR, name), encoding="utf-8") as f:
                    entries += [json.loads(line) for line in f if line.strip()]
    return sorted(entries, key=lambda e: e["time"], reverse=True)


def render_log():
    out = ["# Co-op Log", "",
           "Every sync by every player, newest first. Times are each player's local time. "
           "Generated by `er_sync.py` from `log/*.jsonl`. Don't edit by hand.", ""]
    day = None
    for e in load_log():
        if date_text(e["time"]) != day:
            if day is not None:
                out.append("")
            day = date_text(e["time"])
            out += [f"## {day}", ""]
        out.append(f"- **{time_text(e['time'])} · {e['player']}**: {log_sentence(e)}")
    if day is None:
        out.append("No syncs yet.")
    with open(LOG_MD, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(out).rstrip() + "\n")


# --- Sync ----------------------------------------------------------------------

def write_progress(snap):
    os.makedirs(PROGRESS_DIR, exist_ok=True)
    with open(os.path.join(HERE, progress_path(snap["player"])), "w", encoding="utf-8", newline="\n") as f:
        json.dump(snap, f, indent=1, ensure_ascii=False)
        f.write("\n")


def write_log(snap, d):
    """Rewrite this player's log as the committed version plus one new entry."""
    entry = {"time": snap["save_modified"], "player": snap["player"],
             "bosses_total": len(snap["bosses"]), "regions_total": len(snap["regions"]),
             **{k: d[k] for k in ("first_sync", "lost", "level", "previous_level", "new_bosses", "new_regions")}}
    os.makedirs(LOG_DIR, exist_ok=True)
    with open(os.path.join(HERE, log_path(snap["player"])), "w", encoding="utf-8", newline="\n") as f:
        f.write((committed(log_path(snap["player"])) or "") + json.dumps(entry, ensure_ascii=False) + "\n")


def report(d):
    lines = [f"Level {d['level']}" + (f" (was {d['previous_level']})"
             if d["previous_level"] not in (None, d["level"]) else "")]
    if d["new_bosses"]:
        lines.append("New bosses: " + ", ".join(d["new_bosses"]))
    if d["new_regions"]:
        lines.append("New regions: " + ", ".join(d["new_regions"]))
    return "\n".join(lines)


def sync(dry_run, push):
    cfg = load_config()
    snap = read_snapshot(cfg)
    online = push and not dry_run and has_upstream()
    if push and not dry_run and not online:
        print("No upstream branch set; committing locally only.")

    if online:
        pull()
    path = progress_path(cfg["player"])
    prev_text = committed(path)
    prev = json.loads(prev_text) if prev_text else None
    d = diff(prev, snap)
    if not is_change(d):
        print(f"Nothing new for {cfg['player']} since the last sync.")
        return
    if dry_run:
        print("Would sync:\n" + report(d))
        return

    write_progress(snap)
    write_log(snap, d)
    for _ in range(PUSH_ATTEMPTS):
        render()
        git("add", path, log_path(cfg["player"]), *GENERATED)
        git("commit", "-m", commit_message(cfg["player"], d))
        if not online:
            break
        pushed = git("push", check=False)
        if pushed.returncode == 0:
            break
        # Usually someone pushed first: undo our commit, keep our own files, pull and redo
        print("Push rejected; pulling the latest and retrying.")
        git("reset", "--soft", "HEAD~1")
        git("restore", "--staged", *GENERATED)
        pull()
    else:
        sys.exit(f"Push failed {PUSH_ATTEMPTS} times:\n{pushed.stderr.strip()}\n"
                 "Nothing was committed; run the sync again later.")
    print(f"Synced {cfg['player']}.\n" + report(d))


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    args = sys.argv[1:]
    if args[:1] == ["render"]:
        render()
    elif set(args) <= {"--dry-run", "--no-push"}:
        sync(dry_run="--dry-run" in args, push="--no-push" not in args)
    else:
        sys.exit(__doc__)
