//! Turns a catch-up result into lines of text. The overlay draws them; catchup_preview prints them.

use crate::catchup::{when_text, CatchUp};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    Title,
    Summary,
    Region,
    Item,
    Footer,
}

#[derive(Debug, PartialEq)]
pub struct Line {
    pub style: Style,
    pub text: String,
    /// Shown dimmer, after the text.
    pub aside: String,
}

fn line(style: Style, text: impl Into<String>, aside: impl Into<String>) -> Line {
    Line { style, text: text.into(), aside: aside.into() }
}

fn plural(n: usize, word: &str) -> String {
    let suffix = match (n, word.ends_with('s')) {
        (1, _) => "",
        (_, true) => "es",
        _ => "s",
    };
    format!("{n} {word}{suffix}")
}

/// Lists at most `max_lines` bosses and regions, then "and N more".
pub fn lines(c: &CatchUp, max_lines: usize) -> Vec<Line> {
    let mut out = vec![line(Style::Title, format!("Catch-up for {}", c.me.player), "")];
    if c.others.is_empty() {
        out.push(line(Style::Summary, "No one else has synced yet.", ""));
    } else if c.bosses.is_empty() && c.regions.is_empty() {
        out.push(line(Style::Summary, "Caught up with everyone.", ""));
    } else {
        out.push(line(
            Style::Summary,
            format!("{} and {} to catch up on", plural(c.boss_count(), "boss"), plural(c.regions.len(), "region")),
            "",
        ));
        if let Some(l) = &c.leader {
            out.push(line(Style::Summary, format!("Furthest ahead: {}", l.player), format!("{} ahead", plural(l.bosses_ahead, "boss"))));
        }

        let mut shown = 0;
        let total = c.boss_count() + c.regions.len();
        'bosses: for (region, bosses) in &c.bosses {
            out.push(line(Style::Region, region.as_str(), ""));
            for b in bosses {
                if shown == max_lines {
                    out.pop_if(|l| l.style == Style::Region);
                    break 'bosses;
                }
                out.push(line(Style::Item, b.name.as_str(), b.beaten_by.join(", ")));
                shown += 1;
            }
        }
        if shown < max_lines && !c.regions.is_empty() {
            out.push(line(Style::Region, "Regions not reached", ""));
            for r in c.regions.iter().take(max_lines - shown) {
                out.push(line(Style::Item, r.name.as_str(), r.reached_by.join(", ")));
                shown += 1;
            }
        }
        if shown < total {
            out.push(line(Style::Item, format!("and {} more", total - shown), "full list in PROGRESS.md"));
        }
    }

    for p in &c.others {
        out.push(line(
            Style::Footer,
            format!("{}: Lv {}, {}", p.player, p.level, plural(p.bosses.len(), "boss")),
            format!("synced {}", when_text(&p.save_modified)),
        ));
    }
    out.push(line(
        Style::Footer,
        format!("You: Lv {}, {}", c.me.level, plural(c.me.bosses.len(), "boss")),
        format!("synced {}", when_text(&c.me.save_modified)),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catchup::{compute, Boss, Order, Player};

    fn player(name: &str, bosses: &[&str], regions: &[&str]) -> Player {
        Player {
            player: name.into(),
            level: 30,
            bosses: bosses.iter().map(|b| Boss { name: b.to_string(), region: "Limgrave".into() }).collect(),
            regions: regions.iter().map(|r| r.to_string()).collect(),
            save_modified: "2026-10-31T14:58:55".into(),
        }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| if l.aside.is_empty() { l.text.clone() } else { format!("{} | {}", l.text, l.aside) }).collect()
    }

    #[test]
    fn full_list() {
        let c = compute(
            "Toozy",
            vec![player("Toozy", &["A"], &["Limgrave"]), player("Arya", &["A", "B", "C"], &["Limgrave", "Caelid"])],
            &Order::default(),
        )
        .unwrap();
        assert_eq!(
            texts(&lines(&c, 25)),
            vec![
                "Catch-up for Toozy",
                "2 bosses and 1 region to catch up on",
                "Furthest ahead: Arya | 2 bosses ahead",
                "Limgrave",
                "B | Arya",
                "C | Arya",
                "Regions not reached",
                "Caelid | Arya",
                "Arya: Lv 30, 3 bosses | synced Oct 31, 2:58 PM",
                "You: Lv 30, 1 boss | synced Oct 31, 2:58 PM",
            ]
        );
    }

    #[test]
    fn truncates_and_drops_empty_region_header() {
        let c = compute(
            "Toozy",
            vec![player("Toozy", &[], &[]), player("Arya", &["A", "B", "C"], &["Caelid"])],
            &Order::default(),
        )
        .unwrap();
        let t = texts(&lines(&c, 2));
        assert_eq!(t[3..6], ["Limgrave", "A | Arya", "B | Arya"]);
        assert_eq!(t[6], "and 2 more | full list in PROGRESS.md");
    }

    #[test]
    fn alone() {
        let c = compute("Toozy", vec![player("Toozy", &["A"], &[])], &Order::default()).unwrap();
        assert_eq!(texts(&lines(&c, 25))[1], "No one else has synced yet.");
    }
}
