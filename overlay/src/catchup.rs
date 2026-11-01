//! Works out what "me" is missing compared with the rest of the group.
//!
//! Same rules as the "Catch-up" section that `er_sync.py render` writes to PROGRESS.md:
//! a boss is missing if any other player has beaten it and I haven't, a region is missing
//! if any other player has reached it and I haven't. Everything is read from the repo clone.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde::Deserialize;

#[derive(Deserialize, Clone, Debug)]
pub struct Boss {
    pub name: String,
    pub region: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Player {
    pub player: String,
    pub level: u32,
    pub bosses: Vec<Boss>,
    pub regions: Vec<String>,
    pub save_modified: String,
}

#[derive(Deserialize)]
struct Tables {
    bosses: Vec<Boss>,
}

#[derive(Deserialize)]
struct RepoConfig {
    player: String,
}

/// Boss and region order from er_tables.json, so the list reads in the same order as PROGRESS.md.
#[derive(Default)]
pub struct Order {
    boss: HashMap<String, usize>,
    region: HashMap<String, usize>,
}

impl Order {
    pub fn from_bosses(bosses: &[Boss]) -> Self {
        let mut order = Order::default();
        for (i, b) in bosses.iter().enumerate() {
            order.boss.entry(b.name.clone()).or_insert(i);
            let next = order.region.len();
            order.region.entry(b.region.clone()).or_insert(next);
        }
        order
    }

    fn boss(&self, name: &str) -> usize {
        self.boss.get(name).copied().unwrap_or(usize::MAX)
    }

    fn region(&self, name: &str) -> usize {
        self.region.get(name).copied().unwrap_or(usize::MAX)
    }
}

#[derive(Debug, PartialEq)]
pub struct MissingBoss {
    pub name: String,
    pub region: String,
    pub beaten_by: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub struct MissingRegion {
    pub name: String,
    pub reached_by: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub struct Leader {
    pub player: String,
    /// Bosses the leader has beaten that I haven't.
    pub bosses_ahead: usize,
}

#[derive(Debug)]
pub struct CatchUp {
    pub me: Player,
    pub others: Vec<Player>,
    /// Grouped by region, regions and bosses in table order.
    pub bosses: Vec<(String, Vec<MissingBoss>)>,
    pub regions: Vec<MissingRegion>,
    /// The other player with the most bosses, if anyone is ahead of me.
    pub leader: Option<Leader>,
}

impl CatchUp {
    pub fn boss_count(&self) -> usize {
        self.bosses.iter().map(|(_, b)| b.len()).sum()
    }
}

pub fn compute(me: &str, mut players: Vec<Player>, order: &Order) -> Result<CatchUp, String> {
    players.sort_by_key(|p| p.player.to_lowercase());
    let i = players
        .iter()
        .position(|p| p.player == me)
        .ok_or_else(|| format!("No progress file for {me} yet. Run er_sync.py once."))?;
    let me = players.remove(i);
    let others = players;

    let mine: HashSet<&str> = me.bosses.iter().map(|b| b.name.as_str()).collect();
    let mut region_of: HashMap<&str, &str> = HashMap::new();
    let mut beaten_by: HashMap<&str, Vec<String>> = HashMap::new();
    for o in &others {
        for b in &o.bosses {
            if !mine.contains(b.name.as_str()) {
                region_of.insert(&b.name, &b.region);
                beaten_by.entry(&b.name).or_default().push(o.player.clone());
            }
        }
    }

    let mut grouped: BTreeMap<(usize, String), Vec<MissingBoss>> = BTreeMap::new();
    for (name, who) in beaten_by {
        let region = region_of[name];
        grouped.entry((order.region(region), region.to_string())).or_default().push(MissingBoss {
            name: name.to_string(),
            region: region.to_string(),
            beaten_by: who,
        });
    }
    let bosses = grouped
        .into_iter()
        .map(|((_, region), mut list)| {
            list.sort_by(|a, b| order.boss(&a.name).cmp(&order.boss(&b.name)).then(a.name.cmp(&b.name)));
            (region, list)
        })
        .collect();

    let reached: HashSet<&str> = me.regions.iter().map(String::as_str).collect();
    let mut regions: Vec<MissingRegion> = Vec::new();
    for o in &others {
        for r in o.regions.iter().filter(|r| !reached.contains(r.as_str())) {
            match regions.iter_mut().find(|m| &m.name == r) {
                Some(m) => m.reached_by.push(o.player.clone()),
                None => regions.push(MissingRegion { name: r.clone(), reached_by: vec![o.player.clone()] }),
            }
        }
    }

    let leader = others
        .iter()
        .map(|o| Leader {
            player: o.player.clone(),
            bosses_ahead: o.bosses.iter().filter(|b| !mine.contains(b.name.as_str())).count(),
        })
        .filter(|l| l.bosses_ahead > 0)
        .max_by_key(|l| l.bosses_ahead);

    Ok(CatchUp { me, others, bosses, regions, leader })
}

/// Reads progress/*.json, er_tables.json and, unless `me` is given, config.json from the repo clone.
pub fn load(repo: &Path, me: Option<&str>) -> Result<CatchUp, String> {
    let read = |p: &Path| fs::read_to_string(p).map_err(|e| format!("Can't read {}: {e}", p.display()));
    let me = match me {
        Some(m) => m.to_string(),
        None => {
            let path = repo.join("config.json");
            serde_json::from_str::<RepoConfig>(&read(&path)?)
                .map_err(|e| format!("Bad {}: {e}", path.display()))?
                .player
        }
    };
    let tables_path = repo.join("er_tables.json");
    let tables: Tables =
        serde_json::from_str(&read(&tables_path)?).map_err(|e| format!("Bad {}: {e}", tables_path.display()))?;

    let dir = repo.join("progress");
    let entries = fs::read_dir(&dir).map_err(|e| format!("Can't read {}: {e}", dir.display()))?;
    let mut players = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|x| x == "json") {
            players.push(serde_json::from_str(&read(&path)?).map_err(|e| format!("Bad {}: {e}", path.display()))?);
        }
    }
    compute(&me, players, &Order::from_bosses(&tables.bosses))
}

/// "2026-10-31T14:58:55" -> "Oct 31, 2:58 PM". Falls back to the raw text.
pub fn when_text(iso: &str) -> String {
    let parse = || -> Option<String> {
        let (date, time) = iso.split_once('T')?;
        let mut d = date.split('-').skip(1).map(|s| s.parse::<usize>().ok());
        let (month, day) = (d.next()??, d.next()??);
        let mut t = time.split(':');
        let (hour, minute) = (t.next()?.parse::<u32>().ok()?, t.next()?);
        let name = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
            .get(month.checked_sub(1)?)?;
        let h12 = if hour % 12 == 0 { 12 } else { hour % 12 };
        Some(format!("{name} {day}, {h12}:{minute} {}", if hour < 12 { "AM" } else { "PM" }))
    };
    parse().unwrap_or_else(|| iso.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boss(name: &str, region: &str) -> Boss {
        Boss { name: name.into(), region: region.into() }
    }

    fn player(name: &str, bosses: &[(&str, &str)], regions: &[&str]) -> Player {
        Player {
            player: name.into(),
            level: 10,
            bosses: bosses.iter().map(|(n, r)| boss(n, r)).collect(),
            regions: regions.iter().map(|r| r.to_string()).collect(),
            save_modified: "2026-10-31T14:58:55".into(),
        }
    }

    fn order() -> Order {
        Order::from_bosses(&[
            boss("Margit", "Limgrave"),
            boss("Godrick", "Limgrave"),
            boss("Tree Sentinel", "Limgrave"),
            boss("Rennala", "Liurnia"),
            boss("Smarag", "Liurnia"),
        ])
    }

    #[test]
    fn lists_what_others_have_that_i_dont_in_table_order() {
        let players = vec![
            player("Toozy", &[("Tree Sentinel", "Limgrave")], &["Limgrave"]),
            player("Arya", &[("Smarag", "Liurnia"), ("Godrick", "Limgrave"), ("Margit", "Limgrave")], &["Limgrave", "Liurnia", "Caelid"]),
            player("Ben", &[("Margit", "Limgrave"), ("Tree Sentinel", "Limgrave")], &["Limgrave", "Caelid"]),
        ];
        let c = compute("Toozy", players, &order()).unwrap();

        assert_eq!(c.bosses.len(), 2);
        assert_eq!(c.bosses[0].0, "Limgrave");
        let limgrave: Vec<_> = c.bosses[0].1.iter().map(|b| (b.name.as_str(), b.beaten_by.clone())).collect();
        assert_eq!(limgrave, vec![("Margit", vec!["Arya".into(), "Ben".into()]), ("Godrick", vec!["Arya".into()])]);
        assert_eq!(c.bosses[1].0, "Liurnia");
        assert_eq!(c.boss_count(), 3);

        assert_eq!(
            c.regions,
            vec![
                MissingRegion { name: "Liurnia".into(), reached_by: vec!["Arya".into()] },
                MissingRegion { name: "Caelid".into(), reached_by: vec!["Arya".into(), "Ben".into()] },
            ]
        );
        assert_eq!(c.leader, Some(Leader { player: "Arya".into(), bosses_ahead: 3 }));
    }

    #[test]
    fn caught_up_has_no_leader() {
        let players = vec![
            player("Toozy", &[("Margit", "Limgrave"), ("Godrick", "Limgrave")], &["Limgrave"]),
            player("Arya", &[("Margit", "Limgrave")], &["Limgrave"]),
        ];
        let c = compute("Toozy", players, &order()).unwrap();
        assert!(c.bosses.is_empty() && c.regions.is_empty());
        assert_eq!(c.leader, None);
    }

    #[test]
    fn unknown_bosses_go_last() {
        let players = vec![
            player("Toozy", &[], &[]),
            player("Arya", &[("Some DLC Boss", "Gravesite Plain"), ("Rennala", "Liurnia")], &[]),
        ];
        let c = compute("Toozy", players, &order()).unwrap();
        let regions: Vec<_> = c.bosses.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(regions, vec!["Liurnia", "Gravesite Plain"]);
    }

    #[test]
    fn missing_me_is_an_error() {
        let err = compute("Toozy", vec![player("Arya", &[], &[])], &order()).unwrap_err();
        assert!(err.contains("Toozy"));
    }

    #[test]
    fn formats_sync_time() {
        assert_eq!(when_text("2026-10-31T14:58:55"), "Oct 31, 2:58 PM");
        assert_eq!(when_text("2026-01-05T00:07:00"), "Jan 5, 12:07 AM");
        assert_eq!(when_text("garbage"), "garbage");
    }

    #[test]
    fn loads_the_real_repo() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let c = load(repo, Some("Toozy")).unwrap();
        assert_eq!(c.me.player, "Toozy");
    }
}
