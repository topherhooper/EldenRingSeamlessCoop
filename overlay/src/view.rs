//! Turns a catch-up result into lines of text. The overlay's menu draws them; catchup_preview
//! prints them.

use crate::catchup::{when_text, CatchUp};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    Title,
    Summary,
    Region,
    Item,
    Footer,
}

/// A boss the player can pick as the compass target.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub name: String,
    pub region: String,
    pub flag: Option<u32>,
}

#[derive(Debug, PartialEq)]
pub struct Line {
    pub style: Style,
    pub text: String,
    /// Shown dimmer, after the text.
    pub aside: String,
    /// Set on boss lines.
    pub target: Option<Target>,
}

fn line(style: Style, text: impl Into<String>, aside: impl Into<String>) -> Line {
    Line {
        style,
        text: text.into(),
        aside: aside.into(),
        target: None,
    }
}

fn plural(n: usize, word: &str) -> String {
    let suffix = match (n, word.ends_with('s')) {
        (1, _) => "",
        (_, true) => "es",
        _ => "s",
    };
    format!("{n} {word}{suffix}")
}

/// "3 bosses and 1 region", leaving out whichever is zero.
fn behind_text(c: &CatchUp) -> String {
    match (c.boss_count(), c.regions.len()) {
        (b, 0) => plural(b, "boss"),
        (0, r) => plural(r, "region"),
        (b, r) => format!("{} and {}", plural(b, "boss"), plural(r, "region")),
    }
}

/// Names the players, or says "everyone" when that's all of the others (and there's more than one).
fn who(c: &CatchUp, names: &[String]) -> String {
    if c.others.len() > 1 && names.len() == c.others.len() {
        "everyone".into()
    } else {
        names.join(", ")
    }
}

/// One line for the in-game hint: main text and a dimmer aside.
pub fn compact(c: &CatchUp) -> (String, String) {
    if c.others.is_empty() {
        ("No one else has synced yet".into(), String::new())
    } else if c.bosses.is_empty() && c.regions.is_empty() {
        ("Caught up".into(), String::new())
    } else {
        let aside = c
            .leader
            .as_ref()
            .map(|l| format!("{} is {} ahead", l.player, l.bosses_ahead))
            .unwrap_or_default();
        (format!("{} behind", behind_text(c)), aside)
    }
}

/// The whole catch-up list. `now` is the local time in seconds (see `local_seconds`); without it,
/// sync times are shown as dates.
pub fn lines(c: &CatchUp, now: Option<i64>) -> Vec<Line> {
    let mut out = vec![line(
        Style::Title,
        format!("Catch-up for {}", c.me.player),
        "",
    )];
    if c.others.is_empty() {
        out.push(line(Style::Summary, "No one else has synced yet.", ""));
    } else if c.bosses.is_empty() && c.regions.is_empty() {
        out.push(line(Style::Summary, "Caught up with everyone.", ""));
    } else {
        out.push(line(
            Style::Summary,
            format!("{} to catch up on", behind_text(c)),
            "",
        ));
        if let Some(l) = &c.leader {
            out.push(line(
                Style::Summary,
                format!("Furthest ahead: {}", l.player),
                format!("{} ahead", plural(l.bosses_ahead, "boss")),
            ));
        }
        for (region, bosses) in &c.bosses {
            out.push(line(
                Style::Region,
                region.as_str(),
                bosses.len().to_string(),
            ));
            for b in bosses {
                out.push(Line {
                    target: Some(Target {
                        name: b.name.clone(),
                        region: b.region.clone(),
                        flag: b.flag,
                    }),
                    ..line(Style::Item, b.name.as_str(), who(c, &b.beaten_by))
                });
            }
        }
        if !c.regions.is_empty() {
            out.push(line(
                Style::Region,
                "Regions not reached",
                c.regions.len().to_string(),
            ));
            for r in &c.regions {
                out.push(line(Style::Item, r.name.as_str(), who(c, &r.reached_by)));
            }
        }
    }

    for p in &c.others {
        out.push(line(
            Style::Footer,
            format!(
                "{}: Lv {}, {}",
                p.player,
                p.level,
                plural(p.bosses.len(), "boss")
            ),
            synced_text(&p.save_modified, now),
        ));
    }
    out.push(line(
        Style::Footer,
        format!(
            "You: Lv {}, {}",
            c.me.level,
            plural(c.me.bosses.len(), "boss")
        ),
        synced_text(&c.me.save_modified, now),
    ));
    out
}

/// Seconds since 1970-01-01 for a local date and time, ignoring time zones on both sides.
pub fn local_seconds(year: i64, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
    // Days from civil, from Howard Hinnant's date algorithms.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    days * 86400 + (hour * 3600 + minute * 60 + second) as i64
}

fn parse_iso(iso: &str) -> Option<i64> {
    let (date, time) = iso.split_once('T')?;
    let mut d = date.split('-');
    let (y, mo, dd) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let mut t = time.split(':');
    let (h, mi) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let s = t
        .next()
        .and_then(|s| s.get(..2))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Some(local_seconds(y, mo, dd, h, mi, s))
}

/// "synced 3 h ago", or the date once it's a week old.
fn synced_text(iso: &str, now: Option<i64>) -> String {
    format!("synced {}", age_text(iso, now))
}

/// "3 h ago", or the date once it's a week old.
pub fn age_text(iso: &str, now: Option<i64>) -> String {
    let ago = now.zip(parse_iso(iso)).map(|(now, then)| now - then);
    match ago {
        Some(s) if (0..60).contains(&s) => "just now".to_string(),
        Some(s) if (0..3600).contains(&s) => format!("{} min ago", s / 60),
        Some(s) if (0..86400).contains(&s) => format!("{} h ago", s / 3600),
        Some(s) if (0..86400 * 2).contains(&s) => "yesterday".to_string(),
        Some(s) if (0..86400 * 7).contains(&s) => format!("{} days ago", s / 86400),
        _ => when_text(iso),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catchup::{compute, Boss, Order, Player};

    fn player(name: &str, bosses: &[&str], regions: &[&str]) -> Player {
        Player {
            player: name.into(),
            level: 30,
            bosses: bosses
                .iter()
                .map(|b| Boss {
                    name: b.to_string(),
                    region: "Limgrave".into(),
                    flag: None,
                })
                .collect(),
            regions: regions.iter().map(|r| r.to_string()).collect(),
            save_modified: "2026-10-31T14:58:55".into(),
        }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                if l.aside.is_empty() {
                    l.text.clone()
                } else {
                    format!("{} | {}", l.text, l.aside)
                }
            })
            .collect()
    }

    fn now() -> Option<i64> {
        parse_iso("2026-10-31T17:30:00")
    }

    #[test]
    fn full_list() {
        let order = Order::from_bosses(&[Boss {
            name: "B".into(),
            region: "Limgrave".into(),
            flag: Some(1042360800),
        }]);
        let c = compute(
            "Toozy",
            vec![
                player("Toozy", &["A"], &["Limgrave"]),
                player("Arya", &["A", "B", "C"], &["Limgrave", "Caelid"]),
                player("Ben", &["B"], &["Limgrave"]),
            ],
            &order,
        )
        .unwrap();
        let lines = lines(&c, now());
        assert_eq!(
            texts(&lines),
            vec![
                "Catch-up for Toozy",
                "2 bosses and 1 region to catch up on",
                "Furthest ahead: Arya | 2 bosses ahead",
                "Limgrave | 2",
                "B | everyone",
                "C | Arya",
                "Regions not reached | 1",
                "Caelid | Arya",
                "Arya: Lv 30, 3 bosses | synced 2 h ago",
                "Ben: Lv 30, 1 boss | synced 2 h ago",
                "You: Lv 30, 1 boss | synced 2 h ago",
            ]
        );
        // Only boss lines can be targets, and they carry the flag from er_tables.json.
        let targets: Vec<_> = lines.iter().filter_map(|l| l.target.as_ref()).collect();
        assert_eq!(targets.len(), 2);
        assert_eq!(
            (targets[0].name.as_str(), targets[0].flag),
            ("B", Some(1042360800))
        );
        assert_eq!(targets[1].flag, None);
        assert_eq!(
            compact(&c),
            (
                "2 bosses and 1 region behind".into(),
                "Arya is 2 ahead".into()
            )
        );
    }

    #[test]
    fn leaves_out_zero_counts() {
        let c = compute(
            "Toozy",
            vec![
                player("Toozy", &[], &["Limgrave"]),
                player("Arya", &["A"], &["Limgrave"]),
            ],
            &Order::default(),
        )
        .unwrap();
        assert_eq!(compact(&c).0, "1 boss behind");
        assert_eq!(texts(&lines(&c, None))[1], "1 boss to catch up on");
    }

    #[test]
    fn alone_and_caught_up() {
        let c = compute(
            "Toozy",
            vec![player("Toozy", &["A"], &[])],
            &Order::default(),
        )
        .unwrap();
        assert_eq!(texts(&lines(&c, now()))[1], "No one else has synced yet.");
        assert_eq!(compact(&c).0, "No one else has synced yet");

        let c = compute(
            "Toozy",
            vec![player("Toozy", &["A"], &[]), player("Arya", &["A"], &[])],
            &Order::default(),
        )
        .unwrap();
        assert_eq!(compact(&c), ("Caught up".into(), String::new()));
    }

    #[test]
    fn sync_times() {
        let at = |iso| synced_text("2026-10-31T14:58:55", parse_iso(iso));
        assert_eq!(at("2026-10-31T14:59:30"), "synced just now");
        assert_eq!(at("2026-10-31T15:20:00"), "synced 21 min ago");
        assert_eq!(at("2026-10-31T23:59:00"), "synced 9 h ago");
        assert_eq!(at("2026-11-01T20:00:00"), "synced yesterday");
        assert_eq!(at("2026-11-04T09:00:00"), "synced 3 days ago");
        assert_eq!(at("2026-11-20T09:00:00"), "synced Oct 31, 2:58 PM");
        // A clock behind the sync time, or no clock at all, falls back to the date.
        assert_eq!(at("2026-10-30T09:00:00"), "synced Oct 31, 2:58 PM");
        assert_eq!(
            synced_text("2026-10-31T14:58:55", None),
            "synced Oct 31, 2:58 PM"
        );
    }

    #[test]
    fn civil_days() {
        assert_eq!(local_seconds(1970, 1, 1, 0, 0, 0), 0);
        assert_eq!(
            local_seconds(2000, 3, 1, 0, 0, 0) - local_seconds(2000, 2, 28, 0, 0, 0),
            2 * 86400
        );
        assert_eq!(local_seconds(2026, 10, 31, 0, 0, 1), 1_793_404_801);
    }
}
