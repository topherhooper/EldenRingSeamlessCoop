//! catchup_overlay.ini, which sits next to the DLL.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, PartialEq)]
pub struct Settings {
    pub repo: PathBuf,
    /// Overrides `player` from the repo's config.json.
    pub player: Option<String>,
    /// Windows virtual-key code.
    pub toggle_key: i32,
    pub start_visible: bool,
    pub corner: Corner,
    /// Text height in pixels. 0 picks one from the game's resolution.
    pub font_size: f32,
    /// Most boss and region lines to show before "and N more".
    pub max_lines: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            repo: PathBuf::new(),
            player: None,
            toggle_key: 0x78, // F9
            start_visible: false,
            corner: Corner::TopRight,
            font_size: 0.0,
            max_lines: 25,
        }
    }
}

pub const TEMPLATE: &str = "\
; Settings for catchup_overlay.dll. Lines starting with ; are comments.

; Full path to your clone of the progress repo (the folder with PROGRESS.md in it).
repo =

; Your name as it appears in PROGRESS.md. Leave empty to use \"player\" from the repo's config.json.
player =

; Key that shows and hides the overlay: F1 to F12, or a single letter or digit.
toggle_key = F9

; Show the overlay as soon as the game starts: 1 = yes, 0 = no.
start_visible = 0

; Where to draw it: top-left, top-right, bottom-left, bottom-right.
corner = top-right

; Text height in pixels. 0 = pick one from the game's resolution.
font_size = 0

; Most boss and region lines to list before \"and N more\".
max_lines = 25
";

/// Parses the ini text. Returns the settings and one message per line it couldn't use.
pub fn parse(text: &str) -> (Settings, Vec<String>) {
    let mut s = Settings::default();
    let mut problems = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with(';')) {
        let Some((key, value)) = line.split_once('=') else {
            problems.push(format!("Ignored line without '=': {line}"));
            continue;
        };
        let (key, value) = (key.trim().to_lowercase(), value.trim().trim_matches('"'));
        let ok = match key.as_str() {
            "repo" => {
                s.repo = PathBuf::from(value);
                true
            }
            "player" => {
                s.player = (!value.is_empty()).then(|| value.to_string());
                true
            }
            "toggle_key" => key_code(value).map(|k| s.toggle_key = k).is_some(),
            "start_visible" => match value {
                "0" | "1" => {
                    s.start_visible = value == "1";
                    true
                }
                _ => false,
            },
            "corner" => {
                let corner = match value.to_lowercase().as_str() {
                    "top-left" => Some(Corner::TopLeft),
                    "top-right" => Some(Corner::TopRight),
                    "bottom-left" => Some(Corner::BottomLeft),
                    "bottom-right" => Some(Corner::BottomRight),
                    _ => None,
                };
                corner.map(|c| s.corner = c).is_some()
            }
            "font_size" => value.parse::<f32>().ok().filter(|v| (0.0..=200.0).contains(v)).map(|v| s.font_size = v).is_some(),
            "max_lines" => value.parse::<usize>().ok().filter(|v| *v > 0).map(|v| s.max_lines = v).is_some(),
            _ => {
                problems.push(format!("Unknown setting: {key}"));
                continue;
            }
        };
        if !ok {
            problems.push(format!("Bad value for {key}: {value}"));
        }
    }
    if s.repo.as_os_str().is_empty() {
        problems.push("Set repo = to the path of your progress repo clone".into());
    }
    (s, problems)
}

fn key_code(name: &str) -> Option<i32> {
    let upper = name.to_uppercase();
    if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<i32>().ok()) {
        return (1..=12).contains(&n).then_some(0x70 + n - 1);
    }
    match upper.as_bytes() {
        [c @ (b'A'..=b'Z' | b'0'..=b'9')] => Some(*c as i32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_to_defaults_apart_from_repo() {
        let (s, problems) = parse(TEMPLATE);
        assert_eq!(s, Settings::default());
        assert_eq!(problems, vec!["Set repo = to the path of your progress repo clone"]);
    }

    #[test]
    fn reads_values() {
        let (s, problems) = parse(
            "repo = \"D:\\Projects\\EldenRingSeamlessCoop\"\nplayer = Toozy\ntoggle_key = f10\nstart_visible = 1\n\
             corner = Bottom-Left\nfont_size = 22\nmax_lines = 10\n",
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(s.repo, PathBuf::from("D:\\Projects\\EldenRingSeamlessCoop"));
        assert_eq!(s.player.as_deref(), Some("Toozy"));
        assert_eq!(s.toggle_key, 0x79);
        assert!(s.start_visible);
        assert_eq!(s.corner, Corner::BottomLeft);
        assert_eq!(s.font_size, 22.0);
        assert_eq!(s.max_lines, 10);
    }

    #[test]
    fn keys() {
        assert_eq!(key_code("F1"), Some(0x70));
        assert_eq!(key_code("F12"), Some(0x7B));
        assert_eq!(key_code("o"), Some(b'O' as i32));
        assert_eq!(key_code("7"), Some(b'7' as i32));
        assert_eq!(key_code("F13"), None);
        assert_eq!(key_code("Space"), None);
    }

    #[test]
    fn reports_bad_lines_and_keeps_defaults() {
        let (s, problems) = parse("repo = C:\\x\ntoggle_key = Space\ncorner = middle\ncolour = red\njunk\n");
        assert_eq!(s.toggle_key, 0x78);
        assert_eq!(s.corner, Corner::TopRight);
        assert_eq!(problems.len(), 4);
    }
}
