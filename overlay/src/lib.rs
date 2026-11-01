//! catchup_overlay.dll: an Elden Ring overlay that lists the bosses and regions the rest of the
//! group has done that you haven't. It draws with Dear ImGui through hudhook's DX12 hooks and
//! reads only the progress repo's JSON files. It never reads or writes game memory or the save.
//!
//! Load it with ModEngine2 (`external_dlls`), or rename it to dinput8.dll and put it in the game
//! folder: it forwards DirectInput8Create to the real dinput8.dll.

pub mod catchup;
pub mod settings;
pub mod view;

use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use hudhook::hooks::dx12::ImguiDx12Hooks;
use hudhook::imgui::{Condition, ConfigFlags, Context, FontConfig, FontSource, StyleColor, Ui, WindowFlags};
use hudhook::windows::core::{s, PCWSTR};
use hudhook::windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use hudhook::windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use hudhook::windows::Win32::System::Threading::GetCurrentProcessId;
use hudhook::windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use hudhook::windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use hudhook::{ImguiRenderLoop, RenderContext};

use catchup::CatchUp;
use settings::{Corner, Settings};
use view::Style;

const INI_NAME: &str = "catchup_overlay.ini";
const RELOAD_EVERY: Duration = Duration::from_secs(2);

const GOLD: [f32; 4] = [0.86, 0.76, 0.48, 1.0];
const TEXT: [f32; 4] = [0.93, 0.91, 0.86, 1.0];
const DIM: [f32; 4] = [0.62, 0.60, 0.56, 1.0];
const WARN: [f32; 4] = [0.95, 0.55, 0.45, 1.0];

struct Overlay {
    settings: Settings,
    /// Setup problems to show in the overlay, e.g. a bad ini line.
    problems: Vec<String>,
    visible: bool,
    key_was_down: bool,
    data: Result<CatchUp, String>,
    /// Modified times of the files `data` was built from.
    stamps: Vec<(PathBuf, Option<SystemTime>)>,
    last_check: Option<Instant>,
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
                    format!("Created {}. Set repo = in it, then restart the game.", ini.display())
                } else {
                    format!("Couldn't create {}. Create it with a repo = line.", ini.display())
                };
                (settings::parse(settings::TEMPLATE).0, vec![message])
            }
        };
        Overlay {
            visible: settings.start_visible || !problems.is_empty(),
            settings,
            problems,
            key_was_down: false,
            data: Err(String::new()),
            stamps: Vec::new(),
            last_check: None,
        }
    }

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
        self.data = catchup::load(&self.settings.repo, self.settings.player.as_deref());
        self.stamps = stamps;
    }

    /// Flips `visible` when the toggle key goes down while the game window has focus.
    fn poll_toggle(&mut self) {
        let down = unsafe { GetAsyncKeyState(self.settings.toggle_key) } as u16 & 0x8000 != 0;
        if down && !self.key_was_down && game_has_focus() {
            self.visible = !self.visible;
        }
        self.key_was_down = down;
    }

    fn draw(&self, ui: &Ui) {
        // Wrap long lines (setup messages, long boss names) instead of growing past the screen.
        let wrap = (ui.current_font_size() * 30.0).min(ui.io().display_size[0] * 0.45);
        let _wrap = ui.push_text_wrap_pos_with_pos(wrap);
        for p in &self.problems {
            ui.text_colored(WARN, p);
        }
        let c = match &self.data {
            Ok(c) => c,
            Err(e) => {
                if !e.is_empty() {
                    ui.text_colored(WARN, e);
                }
                return;
            }
        };
        let small_gap = ui.text_line_height() * 0.35;
        let mut previous = Style::Title;
        for l in view::lines(c, self.settings.max_lines) {
            if l.style != previous && matches!(l.style, Style::Region | Style::Footer) {
                ui.dummy([0.0, small_gap]);
                if l.style == Style::Footer && previous != Style::Footer {
                    ui.separator();
                }
            }
            previous = l.style;
            let (colour, indent) = match l.style {
                Style::Title => (GOLD, false),
                Style::Summary => (TEXT, false),
                Style::Region => (GOLD, false),
                Style::Item => (TEXT, true),
                Style::Footer => (DIM, false),
            };
            if indent {
                ui.indent();
            }
            ui.text_colored(colour, &l.text);
            if !l.aside.is_empty() {
                ui.same_line();
                if ui.cursor_pos()[0] + ui.calc_text_size(&l.aside)[0] > wrap {
                    ui.new_line();
                }
                ui.text_colored(DIM, &l.aside);
            }
            if indent {
                ui.unindent();
            }
        }
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
        let font = fs::read(Path::new(&windows).join(r"Fonts\segoeui.ttf"));
        let config = FontConfig { oversample_h: 2, ..FontConfig::default() };
        match &font {
            Ok(data) => ctx.fonts().add_font(&[FontSource::TtfData { data, size_pixels: size, config: Some(config) }]),
            Err(_) => ctx.fonts().add_font(&[FontSource::DefaultFontData {
                config: Some(FontConfig { size_pixels: size, ..config }),
            }]),
        };

        let style = ctx.style_mut();
        style.window_rounding = 4.0;
        style.window_border_size = 0.0;
        style.window_padding = [size * 0.7, size * 0.5];
        style.item_spacing = [size * 0.35, size * 0.15];
        style.indent_spacing = size * 0.8;
        style[StyleColor::WindowBg] = [0.05, 0.05, 0.05, 0.72];
        style[StyleColor::Separator] = [0.86, 0.76, 0.48, 0.35];
    }

    fn render(&mut self, ui: &mut Ui) {
        self.poll_toggle();
        if !self.visible {
            return;
        }
        self.reload_if_changed();

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
        ui.window("##catchup")
            .position(pos, Condition::Always)
            .position_pivot(pivot)
            .flags(flags)
            .build(|| self.draw(ui));
    }
}

fn game_has_focus() -> bool {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
        pid == GetCurrentProcessId()
    }
}

hudhook::hudhook!(ImguiDx12Hooks, Overlay::new());

type DirectInput8CreateFn =
    unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut c_void, *mut c_void) -> i32;

/// Lets the DLL stand in for dinput8.dll: the game imports only this function from it.
///
/// # Safety
/// Called by the game with the same arguments as the real DirectInput8Create.
#[no_mangle]
pub unsafe extern "system" fn DirectInput8Create(
    instance: *mut c_void,
    version: u32,
    riid: *const c_void,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> i32 {
    static REAL: OnceLock<Option<DirectInput8CreateFn>> = OnceLock::new();
    let real = REAL.get_or_init(|| unsafe {
        let mut dir = [0u16; 260];
        let len = GetSystemDirectoryW(Some(&mut dir)) as usize;
        let mut path: Vec<u16> = dir[..len].to_vec();
        path.extend("\\dinput8.dll\0".encode_utf16());
        let module = LoadLibraryW(PCWSTR(path.as_ptr())).ok()?;
        let f = GetProcAddress(module, s!("DirectInput8Create"))?;
        Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, DirectInput8CreateFn>(f))
    });
    match real {
        Some(f) => f(instance, version, riid, out, outer),
        None => 0x80004005u32 as i32, // E_FAIL
    }
}
