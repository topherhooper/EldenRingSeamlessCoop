//! catchup_overlay.ini, which sits next to the DLL.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Which button names to show in hints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PadStyle {
    Xbox,
    /// DualShock 4: the View position is "Share".
    PlayStation4,
    /// DualSense: the View position is "Create".
    PlayStation5,
}

/// The `controller_style` setting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StyleSetting {
    /// Follow the controller that's connected or was used last.
    Auto,
    Xbox,
    PlayStation,
}

/// Hold one controller button, then press another. Values are XInput button bits; PlayStation
/// buttons use the bit of the Xbox button in the same place (Triangle is Y, Create is View).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Combo {
    pub hold: u16,
    pub press: u16,
}

#[derive(Debug, PartialEq)]
pub struct Settings {
    pub repo: PathBuf,
    /// Overrides `player` from the repo's config.json.
    pub player: Option<String>,
    /// Windows virtual-key code of the key that opens and closes the menu.
    pub toggle_key: i32,
    /// As written in the ini, for on-screen hints.
    pub toggle_key_name: String,
    pub combo: Option<Combo>,
    pub controller_style: StyleSetting,
    pub corner: Corner,
    /// Background opacity, 0.2 to 1.
    pub opacity: f32,
    /// Text height in pixels. 0 picks one from the game's resolution.
    pub font_size: f32,
    /// Show map and coordinates under the compass, to check it against the game.
    pub compass_debug: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            repo: PathBuf::new(),
            player: None,
            toggle_key: 0x78, // F9
            toggle_key_name: "F9".into(),
            combo: Some(Combo {
                hold: Y,
                press: VIEW,
            }),
            controller_style: StyleSetting::Auto,
            corner: Corner::TopRight,
            opacity: 0.75,
            font_size: 0.0,
            compass_debug: false,
        }
    }
}

pub const TEMPLATE: &str = "\
; Settings for catchup_overlay.dll. Lines starting with ; are comments.

; Full path to your clone of the progress repo (the folder with PROGRESS.md in it).
repo =

; Your name as it appears in PROGRESS.md. Leave empty to use \"player\" from the repo's config.json.
player =

; The key and the controller combo both open and close the catch-up menu.

; Keyboard key: F1 to F12, or a single letter or digit.
toggle_key = F9

; Controller: hold the first button, then press the second. While you hold the first button,
; the game doesn't see the second, so Y + View doesn't open the map. Use \"off\" to turn it off.
; Xbox names: A B X Y LB RB L3 R3 View Menu Up Down Left Right
; PlayStation names: Cross Circle Square Triangle L1 R1 L3 R3 Create (or Share) Options Up Down Left Right
controller_combo = Y + View

; Button names in hints: auto (follow the controller you're using), xbox or playstation.
controller_style = auto

; Where to draw the compass and the menu: top-left, top-right, bottom-left, bottom-right.
corner = top-right

; Background opacity, from 0.2 (see-through) to 1 (solid).
opacity = 0.75

; Text height in pixels. 0 = pick one from the game's resolution.
font_size = 0

; 1 shows your map and coordinates under the compass, to check it's pointing the right way.
compass_debug = 0
";

/// Parses the ini text. Returns the settings and one message per line it couldn't use.
pub fn parse(text: &str) -> (Settings, Vec<String>) {
    let mut s = Settings::default();
    let mut problems = Vec::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with(';'))
    {
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
            "toggle_key" => key_code(value)
                .map(|k| {
                    s.toggle_key = k;
                    s.toggle_key_name = value.to_uppercase();
                })
                .is_some(),
            "controller_combo" => parse_combo(value).map(|c| s.combo = c).is_some(),
            "controller_style" => {
                let style = match value.to_lowercase().as_str() {
                    "auto" => Some(StyleSetting::Auto),
                    "xbox" => Some(StyleSetting::Xbox),
                    "playstation" => Some(StyleSetting::PlayStation),
                    _ => None,
                };
                style.map(|v| s.controller_style = v).is_some()
            }
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
            "opacity" => value
                .parse::<f32>()
                .ok()
                .filter(|v| (0.2..=1.0).contains(v))
                .map(|v| s.opacity = v)
                .is_some(),
            "font_size" => value
                .parse::<f32>()
                .ok()
                .filter(|v| (0.0..=200.0).contains(v))
                .map(|v| s.font_size = v)
                .is_some(),
            "compass_debug" => match value {
                "0" | "1" => {
                    s.compass_debug = value == "1";
                    true
                }
                _ => false,
            },
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

// XInput button bits.
pub const UP: u16 = 0x0001;
pub const DOWN: u16 = 0x0002;
pub const LEFT: u16 = 0x0004;
pub const RIGHT: u16 = 0x0008;
pub const MENU: u16 = 0x0010;
pub const VIEW: u16 = 0x0020;
pub const L3: u16 = 0x0040;
pub const R3: u16 = 0x0080;
pub const LB: u16 = 0x0100;
pub const RB: u16 = 0x0200;
pub const A: u16 = 0x1000;
pub const B: u16 = 0x2000;
pub const X: u16 = 0x4000;
pub const Y: u16 = 0x8000;

/// Bit, Xbox name, PlayStation name (DualSense; a DualShock 4 says Share for View).
const BUTTONS: [(u16, &str, &str); 14] = [
    (UP, "Up", "Up"),
    (DOWN, "Down", "Down"),
    (LEFT, "Left", "Left"),
    (RIGHT, "Right", "Right"),
    (MENU, "Menu", "Options"),
    (VIEW, "View", "Create"),
    (L3, "L3", "L3"),
    (R3, "R3", "R3"),
    (LB, "LB", "L1"),
    (RB, "RB", "R1"),
    (A, "A", "Cross"),
    (B, "B", "Circle"),
    (X, "X", "Square"),
    (Y, "Y", "Triangle"),
];

fn button(name: &str) -> Option<u16> {
    let name = name.to_lowercase();
    let alias = match name.as_str() {
        "back" | "select" | "share" => "view",
        "start" => "menu",
        other => other,
    };
    BUTTONS
        .iter()
        .find(|(_, xbox, ps)| xbox.to_lowercase() == alias || ps.to_lowercase() == alias)
        .map(|(bit, _, _)| *bit)
}

/// The name of a button on the given controller.
pub fn button_label(bit: u16, style: PadStyle) -> &'static str {
    let Some((_, xbox, ps)) = BUTTONS.iter().find(|(b, _, _)| *b == bit) else {
        return "?";
    };
    match style {
        PadStyle::Xbox => xbox,
        PadStyle::PlayStation4 if bit == VIEW => "Share",
        PadStyle::PlayStation4 | PadStyle::PlayStation5 => ps,
    }
}

/// "Y + View", or "Triangle + Create" on a DualSense.
pub fn combo_label(combo: Combo, style: PadStyle) -> String {
    format!(
        "{} + {}",
        button_label(combo.hold, style),
        button_label(combo.press, style)
    )
}

/// `Some(None)` for "off", `None` if the text isn't a valid combo.
fn parse_combo(text: &str) -> Option<Option<Combo>> {
    if text.eq_ignore_ascii_case("off") {
        return Some(None);
    }
    let (hold, press) = text.split_once('+')?;
    let (hold, press) = (button(hold.trim())?, button(press.trim())?);
    (hold != press).then_some(Some(Combo { hold, press }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_to_defaults_apart_from_repo() {
        let (s, problems) = parse(TEMPLATE);
        assert_eq!(s, Settings::default());
        assert_eq!(
            problems,
            vec!["Set repo = to the path of your progress repo clone"]
        );
    }

    #[test]
    fn reads_values() {
        let (s, problems) = parse(
            "repo = \"D:\\Projects\\EldenRingSeamlessCoop\"\nplayer = Toozy\ntoggle_key = f10\n\
             controller_combo = lb + back\ncontroller_style = PlayStation\ncorner = Bottom-Left\n\
             opacity = 0.5\nfont_size = 22\ncompass_debug = 1\n",
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(s.repo, PathBuf::from("D:\\Projects\\EldenRingSeamlessCoop"));
        assert_eq!(s.player.as_deref(), Some("Toozy"));
        assert_eq!((s.toggle_key, s.toggle_key_name.as_str()), (0x79, "F10"));
        assert_eq!(
            s.combo,
            Some(Combo {
                hold: LB,
                press: VIEW
            })
        );
        assert_eq!(s.controller_style, StyleSetting::PlayStation);
        assert_eq!(s.corner, Corner::BottomLeft);
        assert_eq!(s.opacity, 0.5);
        assert_eq!(s.font_size, 22.0);
        assert!(s.compass_debug);
    }

    #[test]
    fn playstation_names_map_to_the_same_buttons() {
        let (s, problems) = parse("repo = C:\\x\ncontroller_combo = Triangle + Create\n");
        assert!(problems.is_empty());
        assert_eq!(s.combo, Settings::default().combo);
        let (s, _) = parse("repo = C:\\x\ncontroller_combo = l1 + share\n");
        assert_eq!(
            s.combo,
            Some(Combo {
                hold: LB,
                press: VIEW
            })
        );
    }

    #[test]
    fn labels_follow_the_controller() {
        let combo = Settings::default().combo.unwrap();
        assert_eq!(combo_label(combo, PadStyle::Xbox), "Y + View");
        assert_eq!(
            combo_label(combo, PadStyle::PlayStation5),
            "Triangle + Create"
        );
        assert_eq!(
            combo_label(combo, PadStyle::PlayStation4),
            "Triangle + Share"
        );
        assert_eq!(
            combo_label(Combo { hold: LB, press: A }, PadStyle::PlayStation5),
            "L1 + Cross"
        );
    }

    #[test]
    fn combo_off_and_bad_combos() {
        let (s, problems) = parse("repo = C:\\x\ncontroller_combo = off\n");
        assert!(problems.is_empty());
        assert_eq!(s.combo, None);

        for bad in ["Y", "Y + Y", "Triangle + Y", "Y + Touchpad", "Y + View + A"] {
            let (s, problems) = parse(&format!("repo = C:\\x\ncontroller_combo = {bad}\n"));
            assert_eq!(problems.len(), 1, "{bad}");
            assert_eq!(s.combo, Settings::default().combo);
        }
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
        let (s, problems) =
            parse("repo = C:\\x\ntoggle_key = Space\ncorner = middle\ncolour = red\njunk\n");
        assert_eq!(s.toggle_key, 0x78);
        assert_eq!(s.corner, Corner::TopRight);
        assert_eq!(problems.len(), 4);
    }
}
