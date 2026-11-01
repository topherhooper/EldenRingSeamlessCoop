//! catchup_overlay.dll: an Elden Ring overlay for catching up with the rest of the group.
//!
//! - During play it shows a compass pointing at the boss you picked, and nothing else.
//! - A key or controller combo opens a menu with the catch-up list (the bosses and regions the
//!   others have done that you haven't), where you pick the compass target. While the menu is
//!   open the game doesn't see keyboard, mouse or controller input.
//!
//! The list comes from the progress repo's JSON files. The compass reads the player's position
//! and the camera from game memory, read-only (see `game`). Nothing is written to the game or
//! the save.
//!
//! Load it with ModEngine2 (`external_dlls`), or rename it to dinput8.dll and put it in the game
//! folder: it forwards DirectInput8Create to the real dinput8.dll.

pub mod catchup;
pub mod compass;
mod dinput;
mod game;
pub mod pad;
pub mod settings;
pub mod view;

use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use hudhook::hooks::dx12::ImguiDx12Hooks;
use hudhook::imgui::{
    Condition, ConfigFlags, Context, FontConfig, FontSource, StyleColor, StyleVar, Ui, WindowFlags,
};
use hudhook::windows::Win32::Foundation::HINSTANCE;
use hudhook::windows::Win32::System::SystemInformation::GetLocalTime;
use hudhook::windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use hudhook::windows::Win32::System::Threading::GetCurrentProcessId;
use hudhook::windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use hudhook::windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId,
};
use hudhook::{Hudhook, ImguiRenderLoop, RenderContext};

use catchup::CatchUp;
use pad::{Pad, STICK_DOWN, STICK_UP};
use settings::{Corner, PadStyle, Settings, StyleSetting, A, B, DOWN, LB, RB, UP};
use view::{Style, Target};

const INI_NAME: &str = "catchup_overlay.ini";
/// Remembers the compass target between game sessions.
const TARGET_FILE: &str = "catchup_overlay_target.txt";
const RELOAD_EVERY: Duration = Duration::from_secs(2);
const FADE_IN: f32 = 0.15;
/// How long the hint stays up at start, after closing the menu, or when the list changes.
const HINT_FOR: Duration = Duration::from_secs(8);
const REPEAT_AFTER: Duration = Duration::from_millis(350);
const REPEAT_EVERY: Duration = Duration::from_millis(80);
const PAGE: usize = 8;

const GOLD: [f32; 4] = [0.86, 0.76, 0.48, 1.0];
const TEXT: [f32; 4] = [0.93, 0.91, 0.86, 1.0];
const DIM: [f32; 4] = [0.62, 0.60, 0.56, 1.0];
const WARN: [f32; 4] = [0.95, 0.55, 0.45, 1.0];

/// Keys the menu reads: the toggle key (filled in from the ini), arrows, Page Up/Down, Enter,
/// Esc and Backspace.
const NAV_KEYS: [i32; 8] = [0, 0x26, 0x28, 0x21, 0x22, 0x0D, 0x1B, 0x08];

static INPUT_BLOCKED: AtomicBool = AtomicBool::new(false);

/// Whether the game should see no keyboard, mouse or controller input (the menu is open).
pub(crate) fn input_blocked() -> bool {
    INPUT_BLOCKED.load(Ordering::Relaxed)
}

/// Appends a line to catchup_overlay.log next to the DLL, started fresh each game launch, so
/// a problem in someone's game can be traced without a debugger.
pub(crate) fn log(message: impl AsRef<str>) {
    use std::io::Write;
    static FILE: std::sync::OnceLock<Option<std::sync::Mutex<fs::File>>> =
        std::sync::OnceLock::new();
    let file = FILE.get_or_init(|| {
        let path = hudhook::util::get_dll_path()?.with_file_name("catchup_overlay.log");
        fs::File::create(path).ok().map(std::sync::Mutex::new)
    });
    if let Some(Ok(mut f)) = file.as_ref().map(|m| m.lock()) {
        let t = unsafe { GetLocalTime() };
        let _ = writeln!(
            f,
            "{:02}:{:02}:{:02} {}",
            t.wHour,
            t.wMinute,
            t.wSecond,
            message.as_ref()
        );
    }
}

/// Fires once when a key or button goes down, then repeats while it's held.
#[derive(Default)]
struct Repeat {
    since: Option<Instant>,
    last: Option<Instant>,
}

impl Repeat {
    fn fire(&mut self, held: bool, now: Instant) -> bool {
        let (Some(since), Some(last)) = (self.since, self.last) else {
            if held {
                (self.since, self.last) = (Some(now), Some(now));
            }
            return held;
        };
        if !held {
            *self = Repeat::default();
            false
        } else if now - since >= REPEAT_AFTER && now - last >= REPEAT_EVERY {
            self.last = Some(now);
            true
        } else {
            false
        }
    }
}

/// What the player pressed this frame.
#[derive(Default)]
struct Input {
    toggle: bool,
    up: bool,
    down: bool,
    page_up: bool,
    page_down: bool,
    select: bool,
    back: bool,
    /// A key or button that closes the menu or picks a target is down.
    holding: bool,
}

/// Where the compass points, or why it can't.
struct Reading {
    angle: Option<f32>,
    detail: String,
    debug: Option<String>,
}

struct Overlay {
    settings: Settings,
    /// Setup problems to show in the overlay, e.g. a bad ini line.
    problems: Vec<String>,
    dir: PathBuf,
    menu_open: bool,
    menu_changed: Instant,
    hint_until: Instant,
    /// Shown with the hint, e.g. that the target was beaten.
    note: Option<String>,
    keys_down: [bool; NAV_KEYS.len()],
    up: Repeat,
    down: Repeat,
    page_up: Repeat,
    page_down: Repeat,
    pad: Pad,
    /// After the menu closes, the game stays blocked until the keys and buttons used in it are
    /// released, so closing with B doesn't also backstep.
    releasing: bool,
    data: Result<CatchUp, String>,
    /// Modified times of the files `data` was built from.
    stamps: Vec<(PathBuf, Option<SystemTime>)>,
    last_check: Option<Instant>,
    /// Font atlas indices of the semibold and title fonts, if Windows has them.
    bold_font: Option<usize>,
    title_font: Option<usize>,
    /// Index into the boss rows of the menu.
    cursor: usize,
    scroll_to_cursor: bool,
    target: Option<Target>,
    /// A target name read from TARGET_FILE, waiting for the list to load.
    saved_target: Option<String>,
    /// The game has drawn a frame. The DLL loads before the game draws anything, so the start-up
    /// hint is timed from the first frame instead.
    drawing: bool,
    /// A character is loaded, as far as the compass can tell.
    in_world: bool,
}

impl Overlay {
    fn new() -> Self {
        let dir = hudhook::util::get_dll_path()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        let ini = dir.join(INI_NAME);
        let (settings, problems) = match fs::read_to_string(&ini) {
            Ok(text) => {
                let (settings, mut problems) = settings::parse(&text);
                if !problems.is_empty() {
                    problems.push(format!("Settings file: {}", ini.display()));
                }
                (settings, problems)
            }
            Err(_) => {
                let message = if fs::write(&ini, settings::TEMPLATE).is_ok() {
                    format!(
                        "Created {}. Set repo = in it, then restart the game.",
                        ini.display()
                    )
                } else {
                    format!(
                        "Couldn't create {}. Create it with a repo = line.",
                        ini.display()
                    )
                };
                (settings::parse(settings::TEMPLATE).0, vec![message])
            }
        };
        for p in &problems {
            log(format!("Setup: {p}"));
        }
        let saved_target = fs::read_to_string(dir.join(TARGET_FILE))
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        let now = Instant::now();
        Overlay {
            settings,
            problems,
            dir,
            menu_open: false,
            menu_changed: now,
            hint_until: now + HINT_FOR,
            note: None,
            keys_down: [false; NAV_KEYS.len()],
            up: Repeat::default(),
            down: Repeat::default(),
            page_up: Repeat::default(),
            page_down: Repeat::default(),
            pad: Pad::default(),
            releasing: false,
            data: Err(String::new()),
            stamps: Vec::new(),
            last_check: None,
            bold_font: None,
            title_font: None,
            cursor: 0,
            scroll_to_cursor: false,
            target: None,
            saved_target,
            drawing: false,
            in_world: false,
        }
    }

    // --- Data ---------------------------------------------------------------------------------

    fn watched_files(&self) -> Vec<(PathBuf, Option<SystemTime>)> {
        let repo = &self.settings.repo;
        let mut files = vec![repo.join("config.json"), repo.join("er_tables.json")];
        if let Ok(entries) = fs::read_dir(repo.join("progress")) {
            files.extend(entries.flatten().map(|e| e.path()));
        }
        files.sort();
        files
            .into_iter()
            .map(|f| {
                let modified = fs::metadata(&f).and_then(|m| m.modified()).ok();
                (f, modified)
            })
            .collect()
    }

    fn reload_if_changed(&mut self) {
        // Without a repo there's nothing to load; the setup message says so.
        if self.settings.repo.as_os_str().is_empty() {
            return;
        }
        if self.last_check.is_some_and(|t| t.elapsed() < RELOAD_EVERY) {
            return;
        }
        self.last_check = Some(Instant::now());
        let stamps = self.watched_files();
        if stamps == self.stamps && self.data.is_ok() {
            return;
        }
        let before = self.data.as_ref().map(|c| c.boss_count()).ok();
        self.data = catchup::load(&self.settings.repo, self.settings.player.as_deref());
        self.stamps = stamps;
        log(match &self.data {
            Ok(c) => format!(
                "Loaded progress for {} with {} other players: {} bosses to catch up on",
                c.me.player,
                c.others.len(),
                c.boss_count()
            ),
            Err(e) => format!("Couldn't load progress: {e}"),
        });
        if let Ok(c) = &self.data {
            if before.is_some_and(|b| b != c.boss_count()) {
                self.hint_until = Instant::now() + HINT_FOR;
            }
        }
        self.check_target();
    }

    /// The bosses the menu lets you pick, in list order.
    fn targets(&self) -> Vec<Target> {
        match &self.data {
            Ok(c) => view::lines(c, None)
                .into_iter()
                .filter_map(|l| l.target)
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// After a reload: restore a saved target, and drop the target once it's off the list.
    fn check_target(&mut self) {
        if self.data.is_err() {
            return;
        }
        let targets = self.targets();
        if let Some(name) = self.saved_target.take() {
            self.target = targets.iter().find(|t| t.name == name).cloned();
        }
        if let Some(current) = &self.target {
            match targets.iter().find(|t| t.name == current.name) {
                Some(t) => self.target = Some(t.clone()),
                None => {
                    self.note = Some(format!("{} is off your list", current.name));
                    self.hint_until = Instant::now() + HINT_FOR;
                    self.set_target(None);
                }
            }
        }
    }

    fn set_target(&mut self, target: Option<Target>) {
        let name = target.as_ref().map(|t| t.name.clone()).unwrap_or_default();
        let _ = fs::write(self.dir.join(TARGET_FILE), name);
        self.target = target;
    }

    // --- Input --------------------------------------------------------------------------------

    fn read_input(&mut self) -> Input {
        self.pad.poll();
        let now = Instant::now();
        let mut down = [false; NAV_KEYS.len()];
        for (i, vk) in NAV_KEYS.iter().enumerate() {
            let vk = if i == 0 {
                self.settings.toggle_key
            } else {
                *vk
            };
            down[i] = unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0;
        }
        let pressed = |i: usize| down[i] && !self.keys_down[i];
        let pad = &self.pad;
        let input = Input {
            toggle: pressed(0) || self.settings.combo.is_some_and(|c| pad.pressed(c)),
            up: self.up.fire(down[1] || pad.held(UP | STICK_UP), now),
            down: self.down.fire(down[2] || pad.held(DOWN | STICK_DOWN), now),
            page_up: self.page_up.fire(down[3] || pad.held(LB), now),
            page_down: self.page_down.fire(down[4] || pad.held(RB), now),
            select: pressed(5) || pad.just_pressed(A),
            back: pressed(6) || pressed(7) || pad.just_pressed(B),
            // Only the keys and buttons that close the menu or pick a target: moving the stick
            // right after closing should move the character.
            holding: [0, 5, 6, 7].iter().any(|&i| down[i])
                || pad.held(A | B | self.settings.combo.map_or(0, |c| c.hold | c.press)),
        };
        self.keys_down = down;
        if game_has_focus() {
            input
        } else {
            Input {
                holding: input.holding,
                ..Input::default()
            }
        }
    }

    fn handle(&mut self, input: Input) {
        if !self.menu_open {
            if input.toggle {
                self.open_menu();
            }
        } else if input.toggle || input.back {
            self.close_menu();
        } else {
            let targets = self.targets();
            let n = targets.len();
            let before = self.cursor;
            if n > 0 {
                if input.up {
                    self.cursor = (self.cursor + n - 1) % n;
                }
                if input.down {
                    self.cursor = (self.cursor + 1) % n;
                }
                if input.page_up {
                    self.cursor = self.cursor.saturating_sub(PAGE);
                }
                if input.page_down {
                    self.cursor = (self.cursor + PAGE).min(n - 1);
                }
                if input.select {
                    let picked = targets[self.cursor.min(n - 1)].clone();
                    let clear = self.target.as_ref().is_some_and(|t| t.name == picked.name);
                    self.set_target((!clear).then_some(picked));
                    self.close_menu();
                }
            }
            self.scroll_to_cursor |= self.cursor != before;
        }
        if !self.menu_open && self.releasing && !input.holding {
            self.releasing = false;
        }
        INPUT_BLOCKED.store(self.menu_open || self.releasing, Ordering::Relaxed);
    }

    fn open_menu(&mut self) {
        self.menu_open = true;
        self.menu_changed = Instant::now();
        self.last_check = None;
        self.note = None;
        let targets = self.targets();
        self.cursor = self
            .target
            .as_ref()
            .and_then(|t| targets.iter().position(|r| r.name == t.name))
            .unwrap_or(0);
        self.scroll_to_cursor = true;
    }

    fn close_menu(&mut self) {
        self.menu_open = false;
        self.releasing = true;
        let now = Instant::now();
        self.menu_changed = now;
        self.hint_until = now + HINT_FOR;
    }

    // --- Hints ---------------------------------------------------------------------------------

    /// Button names to use: the controller in use, unless the ini says otherwise.
    fn pad_style(&self) -> PadStyle {
        match self.settings.controller_style {
            StyleSetting::Auto => self.pad.style(),
            StyleSetting::Xbox => PadStyle::Xbox,
            StyleSetting::PlayStation => match self.pad.style() {
                PadStyle::Xbox => PadStyle::PlayStation5,
                ps => ps,
            },
        }
    }

    /// "F9 or Y + View" ("F9 or Triangle + Create" on a DualSense), naming the controller combo
    /// only when a controller is connected.
    fn toggle_hint(&self) -> String {
        let key = &self.settings.toggle_key_name;
        match self.settings.combo {
            Some(combo) if self.pad.any_connected() => {
                format!(
                    "{key} or {}",
                    settings::combo_label(combo, self.pad_style())
                )
            }
            _ => key.clone(),
        }
    }

    // --- Compass -------------------------------------------------------------------------------

    /// The player's global position on the overworld, if the game says where they are.
    fn player_position(&self) -> Result<((f32, f32), game::Snapshot), String> {
        let snap = game::snapshot().map_err(String::from)?;
        match compass::to_global(snap.map, snap.local, game::conversion_table()) {
            Some(p) => Ok((p, snap)),
            None => Err(format!("No compass in this area ({})", snap.map)),
        }
    }

    fn reading(&self, target: &Target) -> Reading {
        let conv = game::conversion_table();
        let boss = target.flag.and_then(|f| compass::boss_global(f, conv));
        let mut debug = None;
        let (angle, detail) = match (self.player_position(), boss) {
            (Err(why), _) => (None, why),
            (Ok(_), None) => (None, "No map position for this boss".to_string()),
            (Ok((player, snap)), Some(boss)) => {
                let (angle, distance) = compass::bearing(player, boss, snap.forward);
                if self.settings.compass_debug {
                    debug = Some(format!(
                        "{} ({:.0}, {:.0}) = ({:.0}, {:.0}); target ({:.0}, {:.0}); facing ({:.2}, {:.2})",
                        snap.map, snap.local[0], snap.local[2], player.0, player.1, boss.0, boss.1,
                        snap.forward.0, snap.forward.1
                    ));
                }
                let angle = (distance >= compass::NEARBY).then_some(angle);
                (
                    angle,
                    format!("{} · {}", compass::distance_text(distance), target.region),
                )
            }
        };
        Reading {
            angle,
            detail,
            debug,
        }
    }

    /// Distance to a boss for the menu, if both positions are known.
    fn distance_to(&self, player: Option<(f32, f32)>, target: &Target) -> Option<String> {
        let boss = compass::boss_global(target.flag?, game::conversion_table())?;
        let (_, distance) = compass::bearing(player?, boss, (0.0, 1.0));
        Some(compass::distance_text(distance))
    }

    // --- Drawing -------------------------------------------------------------------------------

    fn bold_text(&self, ui: &Ui, colour: [f32; 4], text: &str) {
        self.text_in_font(ui, self.bold_font, colour, text);
    }

    /// The menu title, in a serif face like the game's own menus.
    fn title_text(&self, ui: &Ui, colour: [f32; 4], text: &str) {
        self.text_in_font(ui, self.title_font.or(self.bold_font), colour, text);
    }

    fn text_in_font(&self, ui: &Ui, index: Option<usize>, colour: [f32; 4], text: &str) {
        let font = index.and_then(|i| ui.fonts().fonts().get(i).copied());
        let _font = font.map(|f| ui.push_font(f));
        ui.text_colored(colour, text);
    }

    /// A round compass with an arrow pointing `angle` radians clockwise from straight ahead, or a
    /// dot when there's no direction.
    fn draw_dial(&self, ui: &Ui, radius: f32, angle: Option<f32>, alpha: f32) {
        let [x, y] = ui.cursor_screen_pos();
        let c = [x + radius, y + radius];
        let fade = |[r, g, b, a]: [f32; 4]| [r, g, b, a * alpha];
        let draw = ui.get_window_draw_list();
        draw.add_circle(c, radius, fade([0.0, 0.0, 0.0, 0.45]))
            .filled(true)
            .build();
        draw.add_circle(c, radius, fade([GOLD[0], GOLD[1], GOLD[2], 0.6]))
            .thickness(radius * 0.08)
            .build();
        match angle {
            Some(a) => {
                let point =
                    |r: f32, turn: f32| [c[0] + r * (a + turn).sin(), c[1] - r * (a + turn).cos()];
                let tip = point(radius * 0.78, 0.0);
                let left = point(radius * 0.55, 2.45);
                let right = point(radius * 0.55, -2.45);
                let notch = point(radius * 0.18, std::f32::consts::PI);
                draw.add_triangle(tip, left, notch, fade(GOLD))
                    .filled(true)
                    .build();
                draw.add_triangle(tip, notch, right, fade(GOLD))
                    .filled(true)
                    .build();
            }
            None => {
                draw.add_circle(c, radius * 0.16, fade(GOLD))
                    .filled(true)
                    .build();
            }
        }
        ui.dummy([radius * 2.0, radius * 2.0]);
    }

    /// During play: the compass, a hint, setup problems, or nothing.
    fn draw_hud(&self, ui: &Ui) {
        let hint_left = self
            .hint_until
            .saturating_duration_since(Instant::now())
            .as_secs_f32();
        let show_hint = hint_left > 0.0 && self.target.is_none();
        if self.problems.is_empty() && self.target.is_none() && !show_hint {
            return;
        }
        let alpha = (self.menu_changed.elapsed().as_secs_f32() / FADE_IN).min(1.0)
            * if self.target.is_none() {
                hint_left.min(0.5) * 2.0
            } else {
                1.0
            };
        self.window(ui, alpha, || {
            for p in &self.problems {
                ui.text_colored(WARN, p);
            }
            if let Some(target) = &self.target {
                let reading = self.reading(target);
                let line = ui.text_line_height_with_spacing();
                self.draw_dial(ui, line, reading.angle, alpha);
                ui.same_line();
                ui.group(|| {
                    self.bold_text(ui, GOLD, &target.name);
                    ui.text_colored(DIM, &reading.detail);
                });
                if let Some(debug) = reading.debug {
                    ui.text_colored(DIM, debug);
                }
            } else if show_hint {
                self.bold_text(ui, GOLD, "Catch-up");
                ui.same_line();
                match &self.data {
                    Ok(c) => {
                        let (text, aside) = view::compact(c);
                        ui.text_colored(TEXT, text);
                        if !aside.is_empty() {
                            ui.same_line();
                            ui.text_colored(DIM, aside);
                        }
                    }
                    Err(_) => ui.text_colored(WARN, "can't read the progress files"),
                }
                if let Some(note) = &self.note {
                    ui.text_colored(TEXT, note);
                }
                ui.text_colored(
                    DIM,
                    format!(
                        "{}: open the list and pick a compass target",
                        self.toggle_hint()
                    ),
                );
            }
        });
    }

    /// The catch-up menu: a centered panel over the dimmed game, with the list, details of the
    /// highlighted boss and button prompts.
    fn draw_menu(&mut self, ui: &Ui) {
        let alpha = (self.menu_changed.elapsed().as_secs_f32() / FADE_IN).min(1.0);
        let scroll = std::mem::take(&mut self.scroll_to_cursor);
        let this = &*self;
        let [screen_w, screen_h] = ui.io().display_size;
        let font = ui.current_font_size();

        // Dim the game: input is blocked while the menu is open, so make that obvious.
        ui.get_background_draw_list()
            .add_rect(
                [0.0, 0.0],
                [screen_w, screen_h],
                [0.0, 0.0, 0.0, 0.55 * alpha],
            )
            .filled(true)
            .build();

        let size = [
            (font * 36.0).min(screen_w * 0.9),
            (screen_h * 0.78)
                .min(font * 40.0)
                .max(font * 16.0)
                .min(screen_h * 0.95),
        ];
        let _alpha = ui.push_style_var(StyleVar::Alpha(alpha));
        let _border = ui.push_style_var(StyleVar::WindowBorderSize(1.0));
        let _border_colour =
            ui.push_style_color(StyleColor::Border, [GOLD[0], GOLD[1], GOLD[2], 0.45]);
        let _padding = ui.push_style_var(StyleVar::WindowPadding([font * 1.2, font * 0.9]));
        ui.window("##catchup-menu")
            .position([screen_w / 2.0, screen_h / 2.0], Condition::Always)
            .position_pivot([0.5, 0.5])
            .size(size, Condition::Always)
            .bg_alpha(0.94)
            .flags(
                WindowFlags::NO_DECORATION
                    | WindowFlags::NO_INPUTS
                    | WindowFlags::NO_NAV
                    | WindowFlags::NO_SAVED_SETTINGS
                    | WindowFlags::NO_FOCUS_ON_APPEARING,
            )
            .build(|| this.menu_contents(ui, scroll, alpha));
    }

    fn menu_contents(&self, ui: &Ui, scroll: bool, alpha: f32) {
        let font = ui.current_font_size();
        let line = ui.text_line_height_with_spacing();
        let gap = font * 0.5;
        let full_width = ui.content_region_avail()[0];
        let now = local_now();

        // Title row: "Catch-up" on the left, the player on the right.
        self.title_text(ui, GOLD, "Catch-up");
        if let Ok(c) = &self.data {
            let who = format!(
                "{}  ·  Lv {}  ·  {} bosses",
                c.me.player,
                c.me.level,
                c.me.bosses.len()
            );
            self.right_aligned(ui, DIM, &who, full_width);
        }
        for p in &self.problems {
            ui.text_colored(WARN, p);
        }
        let c = match &self.data {
            Ok(c) => Some(c),
            Err(e) => {
                let message = if e.is_empty() {
                    "Set repo = in catchup_overlay.ini"
                } else {
                    e.as_str()
                };
                ui.text_colored(WARN, message);
                None
            }
        };
        if let Some(c) = c.filter(|c| !c.others.is_empty()) {
            let lines = view::lines(c, now);
            if let Some(summary) = lines.iter().find(|l| l.style == Style::Summary) {
                ui.text_colored(TEXT, &summary.text);
            }
            if let Some(l) = &c.leader {
                self.right_aligned(
                    ui,
                    DIM,
                    &format!("{} is furthest ahead", l.player),
                    full_width,
                );
            }
        }
        self.rule(ui, alpha);

        // The list, or an empty state.
        let lines = c.map(|c| view::lines(c, now)).unwrap_or_default();
        let body: Vec<_> = lines
            .iter()
            .filter(|l| matches!(l.style, Style::Region | Style::Item))
            .collect();
        let rows: Vec<(&Target, &str)> = body
            .iter()
            .filter_map(|l| l.target.as_ref().map(|t| (t, l.aside.as_str())))
            .collect();
        let footer_lines = if rows.is_empty() { 2.6 } else { 4.6 };
        let list_height =
            (ui.content_region_avail()[1] - footer_lines * line - gap).max(line * 3.0);
        let player = self.player_position().ok().map(|(p, _)| p);
        ui.child_window("##list")
            .size([full_width, list_height])
            .build(|| {
                if body.is_empty() {
                    let (heading, detail) = match c {
                    Some(c) if c.others.is_empty() => (
                        "Nothing to catch up on yet",
                        "Bosses show up here once someone else in the group syncs their progress.",
                    ),
                    Some(_) => ("You're caught up", "You've beaten every boss the others have."),
                    None => ("No progress loaded", "Check the repo path in catchup_overlay.ini."),
                };
                    let avail = ui.content_region_avail();
                    ui.dummy([0.0, (avail[1] / 2.0 - line * 1.5).max(0.0)]);
                    self.centered(ui, GOLD, heading, avail[0], true);
                    self.centered(ui, DIM, detail, avail[0], false);
                    return;
                }
                let width = ui.content_region_avail()[0];
                let mut row = 0;
                for (i, l) in body.iter().enumerate() {
                    match (&l.style, &l.target) {
                        (Style::Region, _) => {
                            if i > 0 {
                                ui.dummy([0.0, gap * 0.6]);
                            }
                            self.bold_text(ui, GOLD, &l.text);
                            self.right_aligned(ui, DIM, &l.aside, width);
                        }
                        (_, Some(t)) => {
                            let selected = row == self.cursor;
                            let targeted = self.target.as_ref().is_some_and(|c| c.name == t.name);
                            let distance = self.distance_to(player, t);
                            self.draw_row(
                                ui,
                                &t.name,
                                distance.as_deref(),
                                selected,
                                targeted,
                                width,
                                alpha,
                            );
                            if selected && scroll {
                                ui.set_scroll_here_y_with_ratio(0.5);
                            }
                            row += 1;
                        }
                        _ => self.draw_row(ui, &l.text, Some(&l.aside), false, false, width, alpha),
                    }
                }
            });
        self.rule(ui, alpha);

        // Details of the highlighted boss.
        if let Some((t, who)) = rows.get(self.cursor) {
            let targeted = self.target.as_ref().is_some_and(|c| c.name == t.name);
            self.bold_text(ui, GOLD, &t.name);
            if targeted {
                self.right_aligned(ui, GOLD, "Compass target", full_width);
            }
            let distance = self
                .distance_to(player, t)
                .unwrap_or_else(|| "distance unknown".into());
            ui.text_colored(
                DIM,
                format!("{}  ·  beaten by {who}  ·  {distance}", t.region),
            );
        }

        // Who synced when, and the button prompts.
        if let Some(c) = c {
            let mut synced: Vec<String> = c
                .others
                .iter()
                .map(|p| format!("{} {}", p.player, view::age_text(&p.save_modified, now)))
                .collect();
            synced.push(format!("you {}", view::age_text(&c.me.save_modified, now)));
            ui.text_colored(DIM, format!("Last synced: {}", synced.join(", ")));
        }
        ui.dummy([0.0, gap * 0.4]);
        let on_target = rows
            .get(self.cursor)
            .zip(self.target.as_ref())
            .is_some_and(|((r, _), t)| r.name == t.name);
        let pick = match (rows.is_empty(), on_target) {
            (true, _) => None,
            (false, true) => Some("Clear target"),
            (false, false) => Some("Set target"),
        };
        self.draw_prompts(ui, &self.prompts(pick), full_width, alpha);
    }

    /// One line of the list: highlighted under the cursor, marked if it's the compass target,
    /// with its value right-aligned.
    #[allow(clippy::too_many_arguments)]
    fn draw_row(
        &self,
        ui: &Ui,
        text: &str,
        value: Option<&str>,
        selected: bool,
        targeted: bool,
        width: f32,
        alpha: f32,
    ) {
        let [x, y] = ui.cursor_screen_pos();
        let height = ui.text_line_height();
        let gold = |a: f32| [GOLD[0], GOLD[1], GOLD[2], a * alpha];
        let draw = ui.get_window_draw_list();
        if selected {
            let pad = height * 0.18;
            draw.add_rect(
                [x - pad, y - pad],
                [x + width, y + height + pad],
                gold(0.16),
            )
            .filled(true)
            .rounding(height * 0.15)
            .build();
            draw.add_rect(
                [x - pad, y - pad],
                [x - pad + height * 0.12, y + height + pad],
                gold(1.0),
            )
            .filled(true)
            .build();
        }
        if targeted {
            // A small diamond, like a map marker.
            let (cx, cy, r) = (x + height * 0.45, y + height * 0.5, height * 0.22);
            draw.add_triangle([cx, cy - r], [cx + r, cy], [cx - r, cy], gold(1.0))
                .filled(true)
                .build();
            draw.add_triangle([cx - r, cy], [cx + r, cy], [cx, cy + r], gold(1.0))
                .filled(true)
                .build();
        }
        ui.indent();
        ui.text_colored(if selected || targeted { GOLD } else { TEXT }, text);
        ui.unindent();
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            self.right_aligned(ui, DIM, value, width);
        }
    }

    /// Text at the right edge of a `width`-wide area, on the current line if it fits.
    fn right_aligned(&self, ui: &Ui, colour: [f32; 4], text: &str, width: f32) {
        let text_width = ui.calc_text_size(text)[0];
        ui.same_line();
        let x = width - text_width;
        if x > ui.cursor_pos()[0] {
            ui.same_line_with_pos(x);
        } else {
            ui.new_line();
        }
        ui.text_colored(colour, text);
    }

    /// A line centered in a `width`-wide area.
    fn centered(&self, ui: &Ui, colour: [f32; 4], text: &str, width: f32, bold: bool) {
        let text_width = ui.calc_text_size(text)[0].min(width);
        ui.set_cursor_pos([((width - text_width) / 2.0).max(0.0), ui.cursor_pos()[1]]);
        if bold {
            self.bold_text(ui, colour, text);
        } else {
            ui.text_colored(colour, text);
        }
    }

    /// A thin gold line across the menu.
    fn rule(&self, ui: &Ui, alpha: f32) {
        let gap = ui.current_font_size() * 0.45;
        ui.dummy([0.0, gap]);
        let [x, y] = ui.cursor_screen_pos();
        let width = ui.content_region_avail()[0];
        ui.get_window_draw_list()
            .add_line(
                [x, y],
                [x + width, y],
                [GOLD[0], GOLD[1], GOLD[2], 0.4 * alpha],
            )
            .build();
        ui.dummy([0.0, gap]);
    }

    /// Button prompts for the menu, named for the controller in use or the keyboard.
    fn prompts(&self, pick: Option<&'static str>) -> Vec<(String, &'static str)> {
        let mut out = Vec::new();
        if self.pad.any_connected() {
            let style = self.pad_style();
            let name = |bit| settings::button_label(bit, style).to_string();
            if let Some(pick) = pick {
                out.push(("D-pad".to_string(), "Move"));
                out.push((name(A), pick));
            }
            out.push((name(B), "Back"));
        } else {
            if let Some(pick) = pick {
                out.push(("Arrows".to_string(), "Move"));
                out.push(("Enter".to_string(), pick));
            }
            out.push(("Esc".to_string(), "Back"));
        }
        out
    }

    /// Prompts like "[A] Set target  [B] Back", right-aligned like the game's own menus.
    fn draw_prompts(&self, ui: &Ui, prompts: &[(String, &str)], width: f32, alpha: f32) {
        let font = ui.current_font_size();
        let pad = [font * 0.4, font * 0.12];
        let spacing = font * 0.35;
        let between = font * 1.1;
        let keys: Vec<f32> = prompts
            .iter()
            .map(|(key, _)| ui.calc_text_size(key)[0] + pad[0] * 2.0)
            .collect();
        let total: f32 = prompts
            .iter()
            .zip(&keys)
            .map(|((_, action), key)| key + spacing + ui.calc_text_size(action)[0])
            .sum::<f32>()
            + between * prompts.len().saturating_sub(1) as f32;
        let [cx, cy] = ui.cursor_pos();
        ui.set_cursor_pos([cx + (width - total).max(0.0), cy]);
        for (i, ((key, action), key_width)) in prompts.iter().zip(&keys).enumerate() {
            if i > 0 {
                ui.same_line_with_spacing(0.0, between);
            }
            let [x, y] = ui.cursor_screen_pos();
            let (top, bottom) = (y - pad[1], y + ui.text_line_height() + pad[1]);
            let draw = ui.get_window_draw_list();
            draw.add_rect(
                [x, top],
                [x + key_width, bottom],
                [GOLD[0], GOLD[1], GOLD[2], 0.15 * alpha],
            )
            .filled(true)
            .rounding(font * 0.2)
            .build();
            draw.add_rect(
                [x, top],
                [x + key_width, bottom],
                [GOLD[0], GOLD[1], GOLD[2], 0.6 * alpha],
            )
            .rounding(font * 0.2)
            .build();
            draw.add_text([x + pad[0], y], [TEXT[0], TEXT[1], TEXT[2], alpha], key);
            ui.dummy([*key_width, ui.text_line_height()]);
            ui.same_line_with_spacing(0.0, spacing);
            ui.text_colored(TEXT, action);
        }
    }

    /// The overlay's window in the configured corner.
    fn window(&self, ui: &Ui, alpha: f32, contents: impl FnOnce()) {
        let [w, h] = ui.io().display_size;
        let m = h * 0.03;
        let (pos, pivot) = match self.settings.corner {
            Corner::TopLeft => ([m, m], [0.0, 0.0]),
            Corner::TopRight => ([w - m, m], [1.0, 0.0]),
            Corner::BottomLeft => ([m, h - m], [0.0, 1.0]),
            Corner::BottomRight => ([w - m, h - m], [1.0, 1.0]),
        };
        let flags = WindowFlags::NO_DECORATION
            | WindowFlags::NO_INPUTS
            | WindowFlags::NO_NAV
            | WindowFlags::ALWAYS_AUTO_RESIZE
            | WindowFlags::NO_SAVED_SETTINGS
            | WindowFlags::NO_FOCUS_ON_APPEARING;
        let _alpha = ui.push_style_var(StyleVar::Alpha(alpha));
        ui.window("##catchup")
            .position(pos, Condition::Always)
            .position_pivot(pivot)
            .bg_alpha(self.settings.opacity)
            .flags(flags)
            .build(contents);
    }
}

impl ImguiRenderLoop for Overlay {
    fn initialize(&mut self, ctx: &mut Context, _render_context: &mut dyn RenderContext) {
        // Keep ImGui from writing imgui.ini into the game folder or touching the cursor.
        ctx.set_ini_filename(None::<PathBuf>);
        ctx.set_log_filename(None::<PathBuf>);
        ctx.io_mut().config_flags |= ConfigFlags::NO_MOUSE | ConfigFlags::NO_MOUSE_CURSOR_CHANGE;

        let height = ctx.io().display_size[1];
        let size = match self.settings.font_size {
            s if s > 0.0 => s,
            _ => (height / 1080.0 * 20.0).clamp(14.0, 48.0),
        };
        let windows = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let fonts = Path::new(&windows).join("Fonts");
        let config = FontConfig {
            oversample_h: 2,
            ..FontConfig::default()
        };
        // The first font added is the default.
        match fs::read(fonts.join("segoeui.ttf")) {
            Ok(data) => ctx.fonts().add_font(&[FontSource::TtfData {
                data: &data,
                size_pixels: size,
                config: Some(config.clone()),
            }]),
            Err(_) => ctx.fonts().add_font(&[FontSource::DefaultFontData {
                config: Some(FontConfig {
                    size_pixels: size,
                    ..config.clone()
                }),
            }]),
        };
        let mut next = 1;
        if let Ok(data) = fs::read(fonts.join("seguisb.ttf")) {
            ctx.fonts().add_font(&[FontSource::TtfData {
                data: &data,
                size_pixels: size,
                config: Some(config.clone()),
            }]);
            self.bold_font = Some(next);
            next += 1;
        }
        if let Ok(data) = fs::read(fonts.join("georgia.ttf")) {
            ctx.fonts().add_font(&[FontSource::TtfData {
                data: &data,
                size_pixels: size * 1.6,
                config: Some(config),
            }]);
            self.title_font = Some(next);
        }

        let style = ctx.style_mut();
        style.window_rounding = size * 0.3;
        style.window_border_size = 0.0;
        style.window_padding = [size * 0.7, size * 0.5];
        style.item_spacing = [size * 0.35, size * 0.15];
        style.indent_spacing = size * 0.8;
        style.scrollbar_size = size * 0.4;
        style[StyleColor::WindowBg] = [0.05, 0.05, 0.05, 1.0];
        style[StyleColor::ChildBg] = [0.0, 0.0, 0.0, 0.0];
        style[StyleColor::Separator] = [0.86, 0.76, 0.48, 0.35];
        style[StyleColor::ScrollbarBg] = [0.0, 0.0, 0.0, 0.0];
        style[StyleColor::ScrollbarGrab] = [0.86, 0.76, 0.48, 0.35];
    }

    fn render(&mut self, ui: &mut Ui) {
        // Show the hint on the first frame, and again on loading into the world, which is
        // when the player is looking.
        let in_world = game::snapshot().is_ok();
        if !self.drawing {
            let [w, h] = ui.io().display_size;
            log(format!("First frame drawn ({w}x{h})"));
        }
        if in_world && !self.in_world {
            log("Character loaded");
        }
        if !self.drawing || (in_world && !self.in_world) {
            self.drawing = true;
            self.hint_until = Instant::now() + HINT_FOR;
        }
        self.in_world = in_world;
        let input = self.read_input();
        self.reload_if_changed();
        self.handle(input);
        if self.menu_open {
            self.draw_menu(ui);
        } else {
            self.draw_hud(ui);
        }
    }
}

fn game_has_focus() -> bool {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
        pid == GetCurrentProcessId()
    }
}

/// The local time in `view::local_seconds` form.
pub fn local_now() -> Option<i64> {
    let t = unsafe { GetLocalTime() };
    Some(view::local_seconds(
        t.wYear as i64,
        t.wMonth as u32,
        t.wDay as u32,
        t.wHour as u32,
        t.wMinute as u32,
        t.wSecond as u32,
    ))
}

/// Same as hudhook's `hudhook!` macro, plus the input hooks once hudhook has set up MinHook,
/// then waiting for the game so the compass can read it.
///
/// # Safety
/// Called by the Windows loader.
#[no_mangle]
pub unsafe extern "system" fn DllMain(module: HINSTANCE, reason: u32, _: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        let module = module.0 as usize;
        std::thread::spawn(move || {
            log(format!(
                "Loaded into {}",
                std::env::current_exe().unwrap_or_default().display()
            ));
            let overlay = Overlay::new();
            let combo = overlay.settings.combo;
            let hooked = Hudhook::builder()
                .with::<ImguiDx12Hooks>(overlay)
                .with_hmodule(HINSTANCE(module as _))
                .build()
                .apply();
            if let Err(e) = hooked {
                log(format!(
                    "Couldn't hook DirectX 12 drawing: {e:?}. Unloading."
                ));
                hudhook::eject();
                return;
            }
            log("Hooked DirectX 12 drawing");
            pad::install(combo.into_iter().collect());
            log(format!(
                "Controller combo hidden from the game: {}",
                pad::masking()
            ));
            log(format!(
                "Keyboard and mouse blocking hooked: {}",
                dinput::install()
            ));
            game::init();
            log(match game::unavailable() {
                None => "Compass ready".to_string(),
                Some(why) => format!("Compass off: {why}"),
            });
        });
    }
    1
}
