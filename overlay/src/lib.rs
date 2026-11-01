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
    has_bold: bool,
    /// Index into the boss rows of the menu.
    cursor: usize,
    scroll_to_cursor: bool,
    target: Option<Target>,
    /// A target name read from TARGET_FILE, waiting for the list to load.
    saved_target: Option<String>,
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
            has_bold: false,
            cursor: 0,
            scroll_to_cursor: false,
            target: None,
            saved_target,
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

    /// How to use the menu, with the controller's button names when one is connected.
    fn menu_hint(&self, on_target: bool) -> String {
        let pick = if on_target {
            "clear target"
        } else {
            "set target"
        };
        if self.pad.any_connected() {
            let style = self.pad_style();
            let name = |bit| settings::button_label(bit, style);
            format!("D-pad: move · {}: {pick} · {}: close", name(A), name(B))
        } else {
            format!("Arrows: move · Enter: {pick} · Esc: close")
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
        let bold = self
            .has_bold
            .then(|| ui.fonts().fonts().get(1).copied())
            .flatten();
        let _font = bold.map(|f| ui.push_font(f));
        ui.text_colored(colour, text);
    }

    /// The dimmer text after a line's main text, moved to the next line if it won't fit.
    fn aside(&self, ui: &Ui, text: &str, wrap: f32) {
        if text.is_empty() {
            return;
        }
        ui.same_line();
        if ui.cursor_pos()[0] + ui.calc_text_size(text)[0] > wrap {
            ui.new_line();
        }
        ui.text_colored(DIM, text);
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

    /// The catch-up list, with a cursor for picking the compass target.
    fn draw_menu(&mut self, ui: &Ui) {
        let alpha = (self.menu_changed.elapsed().as_secs_f32() / FADE_IN).min(1.0);
        let scroll = std::mem::take(&mut self.scroll_to_cursor);
        let this = &*self;
        this.window(ui, alpha, || {
            let wrap = (ui.current_font_size() * 32.0).min(ui.io().display_size[0] * 0.45);
            let _wrap = ui.push_text_wrap_pos_with_pos(wrap);
            let gap = ui.text_line_height() * 0.35;
            for p in &this.problems {
                ui.text_colored(WARN, p);
            }
            let c = match &this.data {
                Ok(c) => c,
                Err(e) => {
                    if !e.is_empty() {
                        ui.text_colored(WARN, e);
                    }
                    ui.text_colored(DIM, this.menu_hint(false));
                    return;
                }
            };
            let lines = view::lines(c, local_now());
            let (header, rest): (Vec<_>, Vec<_>) = lines
                .iter()
                .partition(|l| matches!(l.style, Style::Title | Style::Summary));
            let (body, footer): (Vec<_>, Vec<_>) =
                rest.into_iter().partition(|l| l.style != Style::Footer);

            for l in &header {
                match l.style {
                    Style::Title => this.bold_text(ui, GOLD, &l.text),
                    _ => ui.text_colored(TEXT, &l.text),
                }
                this.aside(ui, &l.aside, wrap);
            }
            match &this.target {
                Some(t) => {
                    ui.text_colored(DIM, "Compass:");
                    ui.same_line();
                    ui.text_colored(GOLD, &t.name);
                }
                None => ui.text_colored(DIM, "Compass: no target"),
            }

            let player = this.player_position().ok().map(|(p, _)| p);
            if !body.is_empty() {
                ui.dummy([0.0, gap]);
                let row_height = ui.text_line_height_with_spacing();
                let regions = body.iter().filter(|l| l.style == Style::Region).count();
                let full =
                    body.len() as f32 * row_height + regions as f32 * (gap + row_height * 0.2);
                let height = full.min(ui.io().display_size[1] * 0.55);
                ui.child_window("##list").size([wrap, height]).build(|| {
                    let mut row = 0;
                    let mut first = true;
                    for l in &body {
                        match (&l.style, &l.target) {
                            (Style::Region, _) => {
                                if !first {
                                    ui.dummy([0.0, gap]);
                                }
                                this.bold_text(ui, GOLD, &l.text);
                                this.aside(ui, &l.aside, wrap);
                            }
                            (_, Some(t)) => {
                                let selected = row == this.cursor;
                                let targeted =
                                    this.target.as_ref().is_some_and(|c| c.name == t.name);
                                this.draw_row(
                                    ui, t, &l.aside, player, selected, targeted, wrap, alpha,
                                );
                                if selected && scroll {
                                    ui.set_scroll_here_y_with_ratio(0.5);
                                }
                                row += 1;
                            }
                            _ => {
                                ui.indent();
                                ui.text_colored(TEXT, &l.text);
                                this.aside(ui, &l.aside, wrap);
                                ui.unindent();
                            }
                        }
                        first = false;
                    }
                });
            }

            ui.dummy([0.0, gap]);
            ui.separator();
            for l in &footer {
                ui.text_colored(DIM, &l.text);
                this.aside(ui, &l.aside, wrap);
            }
            ui.dummy([0.0, gap]);
            let on_target = this
                .targets()
                .get(this.cursor)
                .zip(this.target.as_ref())
                .is_some_and(|(r, t)| r.name == t.name);
            ui.text_colored(DIM, this.menu_hint(on_target));
        });
    }

    /// One boss in the menu: highlighted under the cursor, marked if it's the compass target.
    #[allow(clippy::too_many_arguments)]
    fn draw_row(
        &self,
        ui: &Ui,
        target: &Target,
        who: &str,
        player: Option<(f32, f32)>,
        selected: bool,
        targeted: bool,
        wrap: f32,
        alpha: f32,
    ) {
        let [x, y] = ui.cursor_screen_pos();
        let height = ui.text_line_height();
        let draw = ui.get_window_draw_list();
        if selected {
            let pad = height * 0.15;
            draw.add_rect(
                [x - pad, y - pad],
                [x + wrap, y + height + pad],
                [GOLD[0], GOLD[1], GOLD[2], 0.18 * alpha],
            )
            .filled(true)
            .rounding(height * 0.2)
            .build();
        }
        if targeted {
            // A small diamond in the indent, like a map marker.
            let (cx, cy, r) = (x + height * 0.4, y + height * 0.5, height * 0.22);
            let gold = [GOLD[0], GOLD[1], GOLD[2], alpha];
            draw.add_triangle([cx, cy - r], [cx + r, cy], [cx - r, cy], gold)
                .filled(true)
                .build();
            draw.add_triangle([cx - r, cy], [cx + r, cy], [cx, cy + r], gold)
                .filled(true)
                .build();
        }
        ui.indent();
        ui.text_colored(if targeted || selected { GOLD } else { TEXT }, &target.name);
        let aside = match self.distance_to(player, target) {
            Some(d) => format!("{who} · {d}"),
            None => who.to_string(),
        };
        self.aside(ui, &aside, wrap);
        ui.unindent();
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
        if let Ok(data) = fs::read(fonts.join("seguisb.ttf")) {
            ctx.fonts().add_font(&[FontSource::TtfData {
                data: &data,
                size_pixels: size,
                config: Some(config),
            }]);
            self.has_bold = true;
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
            let overlay = Overlay::new();
            let combo = overlay.settings.combo;
            let hooked = Hudhook::builder()
                .with::<ImguiDx12Hooks>(overlay)
                .with_hmodule(HINSTANCE(module as _))
                .build()
                .apply();
            if hooked.is_err() {
                hudhook::eject();
                return;
            }
            pad::install(combo.into_iter().collect());
            dinput::install();
            game::init();
        });
    }
    1
}
