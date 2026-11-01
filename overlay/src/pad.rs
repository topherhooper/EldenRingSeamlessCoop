//! Controller support, for whatever controller the player uses.
//!
//! The game reads Xbox controllers through XInput. PlayStation controllers reach it either through
//! XInput too, when Steam Input is on (Steam presents them as Xbox controllers), or directly when
//! it's off. So the overlay reads both ways:
//!
//! - XInput, for Xbox controllers and anything Steam Input translates.
//! - HID input reports from DualShock 4 and DualSense controllers, read-only.
//!
//! It also hooks XInputGetState so that while a combo's hold button is down, the game doesn't see
//! its press button: holding Y and pressing View then toggles the overlay without opening the map.
//! A PlayStation controller the game reads directly (Steam Input off) can't be filtered that way,
//! so there the press button also reaches the game.
//!
//! Hints name buttons the way the controller in use labels them (Triangle + Create on a DualSense).

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::windows::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use hudhook::mh::{MH_ApplyQueued, MhHook};
use hudhook::windows::core::{w, PCSTR};
use hudhook::windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use hudhook::windows::Win32::UI::Input::{
    GetRawInputDeviceInfoW, GetRawInputDeviceList, RAWINPUTDEVICELIST, RIDI_DEVICEINFO,
    RIDI_DEVICENAME, RID_DEVICE_INFO, RIM_TYPEHID,
};

use crate::settings::{
    Combo, PadStyle, A, B, DOWN, L3, LB, LEFT, MENU, R3, RB, RIGHT, UP, VIEW, X, Y,
};

/// XINPUT_STATE.
#[repr(C)]
#[derive(Default)]
struct State {
    packet: u32,
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    sticks: [i16; 4],
}

type GetStateFn = unsafe extern "system" fn(u32, *mut State) -> u32;

const SLOTS: usize = 4;
const RESCAN_EVERY: Duration = Duration::from_secs(2);
const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
const SONY: u32 = 0x054C;

/// Left stick up and down, in two XInput bits no controller button uses.
pub const STICK_UP: u16 = 0x0400;
pub const STICK_DOWN: u16 = 0x0800;

// --- XInput -------------------------------------------------------------------------------------

static COMBOS: OnceLock<Vec<Combo>> = OnceLock::new();
/// XInputGetState as exported. Once hooked, calls to it go through `hooked_get_state`.
static EXPORTED: OnceLock<Option<GetStateFn>> = OnceLock::new();
/// The original XInputGetState, for the hook to call.
static TRAMPOLINE: OnceLock<GetStateFn> = OnceLock::new();
static MASKING: AtomicBool = AtomicBool::new(false);
/// Per slot: the buttons last seen (low 16 bits) and the press buttons being hidden from the
/// game (high 16 bits). A press button stays hidden until it's released, even if the hold
/// button is let go first.
static SLOT_STATE: [AtomicU32; SLOTS] = [const { AtomicU32::new(0) }; SLOTS];

thread_local! {
    /// Set while the overlay reads XInput itself, so the hook passes the real buttons through.
    static OVERLAY_READ: Cell<bool> = const { Cell::new(false) };
}

fn exported() -> Option<GetStateFn> {
    *EXPORTED.get_or_init(|| unsafe {
        // The game imports XInputGetState from xinput1_4.dll by ordinal 2.
        let module = LoadLibraryW(w!("xinput1_4.dll")).ok()?;
        let f = GetProcAddress(module, PCSTR(2 as *const u8))?;
        Some(std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            GetStateFn,
        >(f))
    })
}

/// Remembers the combos and hooks XInputGetState so the game doesn't see them. Call once, after
/// hudhook has set up MinHook. If hooking fails, the combos still work but also reach the game.
pub fn install(combos: Vec<Combo>) {
    let combos = COMBOS.get_or_init(|| combos);
    let Some(target) = exported() else { return };
    if combos.is_empty() || TRAMPOLINE.get().is_some() {
        return;
    }
    unsafe {
        let Ok(hook) = MhHook::new(target as *mut c_void, hooked_get_state as *mut c_void) else {
            return;
        };
        TRAMPOLINE
            .get_or_init(|| std::mem::transmute::<*mut c_void, GetStateFn>(hook.trampoline()));
        if hook.queue_enable().is_ok() && MH_ApplyQueued().ok().is_ok() {
            MASKING.store(true, Ordering::SeqCst);
        }
    }
}

/// Whether the game is kept from seeing the combos (false if the hook couldn't be installed).
pub fn masking() -> bool {
    MASKING.load(Ordering::SeqCst)
}

unsafe extern "system" fn hooked_get_state(slot: u32, state: *mut State) -> u32 {
    let Some(original) = TRAMPOLINE.get() else {
        return ERROR_DEVICE_NOT_CONNECTED;
    };
    let result = unsafe { original(slot, state) };
    if result == 0 && !state.is_null() && (slot as usize) < SLOTS && !OVERLAY_READ.get() {
        if crate::input_blocked() {
            // The catch-up menu is open: the game sees a controller nobody is touching.
            unsafe {
                let packet = (*state).packet;
                *state = State {
                    packet,
                    ..State::default()
                };
            }
        } else if let Some(combos) = COMBOS.get() {
            unsafe {
                (*state).buttons =
                    hide_combos(combos, &SLOT_STATE[slot as usize], (*state).buttons);
            }
        }
    }
    result
}

/// Clears each combo's press button from what the game sees, from the moment it goes down while
/// the hold button is held until it's released.
fn hide_combos(combos: &[Combo], slot: &AtomicU32, buttons: u16) -> u16 {
    let stored = slot.load(Ordering::Relaxed);
    let (before, mut hidden) = (stored as u16, (stored >> 16) as u16);
    for c in combos {
        if buttons & c.press == 0 {
            hidden &= !c.press;
        } else if is_new_press(*c, before, buttons) {
            hidden |= c.press;
        }
    }
    slot.store(((hidden as u32) << 16) | buttons as u32, Ordering::Relaxed);
    buttons & !hidden
}

/// The real buttons on an XInput slot, or None if nothing is connected there.
fn read_xinput(slot: u32) -> Option<u16> {
    let get_state = exported()?;
    let mut state = State::default();
    OVERLAY_READ.set(true);
    let result = unsafe { get_state(slot, &mut state) };
    OVERLAY_READ.set(false);
    (result == 0)
        .then_some(state.buttons | stick_bits(state.sticks[1] as i32 * 128 / 32768 + 128, true))
}

/// Left stick pushed up or down, as extra "buttons" for menu navigation. `y` is 0 to 255 with
/// 128 at rest; `up_is_high` says which end is up.
fn stick_bits(y: i32, up_is_high: bool) -> u16 {
    let (up, down) = if up_is_high {
        (y > 208, y < 48)
    } else {
        (y < 48, y > 208)
    };
    match (up, down) {
        (true, _) => STICK_UP,
        (_, true) => STICK_DOWN,
        _ => 0,
    }
}

fn is_new_press(combo: Combo, before: u16, now: u16) -> bool {
    now & combo.hold == combo.hold && now & combo.press != 0 && before & combo.press == 0
}

// --- PlayStation controllers over HID ----------------------------------------------------------

fn sony_style(product: u32) -> Option<PadStyle> {
    match product {
        0x05C4 | 0x09CC | 0x0BA0 => Some(PadStyle::PlayStation4),
        0x0CE6 | 0x0DF2 => Some(PadStyle::PlayStation5),
        _ => None,
    }
}

/// Turns a DualShock 4 or DualSense input report into XInput button bits, with each button
/// mapped to the Xbox button in the same place (Triangle to Y, Create or Share to View).
fn parse_sony(report: &[u8], dualsense: bool) -> Option<u16> {
    // Where the buttons and the left stick's Y axis are.
    let (offset, stick_y) = match (*report.first()?, dualsense) {
        // DualSense over USB; its short Bluetooth report has the DualShock 4 layout.
        (0x01, true) if report.len() > 32 => (8, 2),
        // DualShock 4 over USB, or either controller's short Bluetooth report.
        (0x01, _) => (5, 2),
        // Full Bluetooth reports.
        (0x11, false) => (7, 4),
        (0x31, true) => (9, 3),
        _ => return None,
    };
    let (face, shoulders) = match report.get(offset..offset + 2)? {
        [f, s] => (*f, *s),
        _ => return None,
    };
    let mut bits = match face & 0x0F {
        0 => UP,
        1 => UP | RIGHT,
        2 => RIGHT,
        3 => DOWN | RIGHT,
        4 => DOWN,
        5 => DOWN | LEFT,
        6 => LEFT,
        7 => UP | LEFT,
        _ => 0,
    };
    // PlayStation sticks read 0 at the top.
    bits |= stick_bits(*report.get(stick_y)? as i32, false);
    for (mask, bit) in [(0x10, X), (0x20, A), (0x40, B), (0x80, Y)] {
        if face & mask != 0 {
            bits |= bit;
        }
    }
    for (mask, bit) in [
        (0x01, LB),
        (0x02, RB),
        (0x10, VIEW),
        (0x20, MENU),
        (0x40, L3),
        (0x80, R3),
    ] {
        if shoulders & mask != 0 {
            bits |= bit;
        }
    }
    Some(bits)
}

/// A PlayStation controller being read on its own thread.
struct SonyPad {
    style: PadStyle,
    buttons: Arc<AtomicU16>,
    alive: Arc<AtomicBool>,
}

impl SonyPad {
    fn open(path: String, style: PadStyle) -> Self {
        let buttons = Arc::new(AtomicU16::new(0));
        let alive = Arc::new(AtomicBool::new(true));
        let (b, a) = (Arc::clone(&buttons), Arc::clone(&alive));
        std::thread::spawn(move || {
            const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;
            let dualsense = style == PadStyle::PlayStation5;
            if let Ok(mut device) = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ_WRITE)
                .open(&path)
            {
                let mut report = [0u8; 1024];
                while let Ok(n @ 1..) = device.read(&mut report) {
                    if let Some(bits) = parse_sony(&report[..n], dualsense) {
                        b.store(bits, Ordering::Relaxed);
                    }
                }
            }
            b.store(0, Ordering::Relaxed);
            a.store(false, Ordering::Relaxed);
        });
        SonyPad {
            style,
            buttons,
            alive,
        }
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }
}

struct Gamepad {
    vendor: u32,
    product: u32,
    path: String,
}

/// Connected HID gamepads and joysticks, from the Raw Input device list.
fn gamepads() -> Vec<Gamepad> {
    let mut out = Vec::new();
    unsafe {
        let entry = size_of::<RAWINPUTDEVICELIST>() as u32;
        let mut count = 0u32;
        GetRawInputDeviceList(None, &mut count, entry);
        let mut list = vec![RAWINPUTDEVICELIST::default(); count as usize];
        let got = GetRawInputDeviceList(Some(list.as_mut_ptr()), &mut count, entry);
        if got == u32::MAX {
            return out;
        }
        for device in list
            .iter()
            .take(got as usize)
            .filter(|d| d.dwType == RIM_TYPEHID)
        {
            let mut info = RID_DEVICE_INFO {
                cbSize: size_of::<RID_DEVICE_INFO>() as u32,
                ..Default::default()
            };
            let mut size = info.cbSize;
            let read = GetRawInputDeviceInfoW(
                Some(device.hDevice),
                RIDI_DEVICEINFO,
                Some(&mut info as *mut _ as *mut c_void),
                &mut size,
            );
            let hid = info.Anonymous.hid;
            // Generic desktop page: 4 is joystick, 5 is gamepad.
            if read == u32::MAX || hid.usUsagePage != 1 || !matches!(hid.usUsage, 4 | 5) {
                continue;
            }
            let mut len = 0u32;
            GetRawInputDeviceInfoW(Some(device.hDevice), RIDI_DEVICENAME, None, &mut len);
            let mut name = vec![0u16; len as usize];
            if GetRawInputDeviceInfoW(
                Some(device.hDevice),
                RIDI_DEVICENAME,
                Some(name.as_mut_ptr().cast()),
                &mut len,
            ) == u32::MAX
            {
                continue;
            }
            let path = String::from_utf16_lossy(&name)
                .trim_end_matches('\0')
                .to_string();
            out.push(Gamepad {
                vendor: hid.dwVendorId,
                product: hid.dwProductId,
                path,
            });
        }
    }
    out
}

// --- Polling -------------------------------------------------------------------------------------

/// Reads every controller once per frame from the render thread.
#[derive(Default)]
pub struct Pad {
    previous: u16,
    current: u16,
    xinput_connected: [bool; SLOTS],
    last_scan: Option<Instant>,
    sony: HashMap<String, SonyPad>,
    /// A gamepad other than a PlayStation one is plugged in.
    other_present: bool,
    previous_xinput: u16,
    previous_sony: u16,
    last_xinput_use: Option<Instant>,
    last_sony_use: Option<Instant>,
}

impl Pad {
    pub fn poll(&mut self) {
        let now = Instant::now();
        // Looking at empty XInput slots and listing devices are slow-ish, so only every
        // couple of seconds.
        let rescan = self.last_scan.is_none_or(|t| now - t >= RESCAN_EVERY);
        if rescan {
            self.last_scan = Some(now);
            self.scan_devices();
        }

        let mut xinput = 0;
        for slot in 0..SLOTS {
            if !self.xinput_connected[slot] && !rescan {
                continue;
            }
            let buttons = read_xinput(slot as u32);
            self.xinput_connected[slot] = buttons.is_some();
            xinput |= buttons.unwrap_or(0);
        }
        let sony = self
            .sony
            .values()
            .filter(|p| p.alive())
            .fold(0, |all, p| all | p.buttons.load(Ordering::Relaxed));

        if xinput != self.previous_xinput && xinput != 0 {
            self.last_xinput_use = Some(now);
        }
        if sony != self.previous_sony && sony != 0 {
            self.last_sony_use = Some(now);
        }
        (self.previous_xinput, self.previous_sony) = (xinput, sony);
        // Steam Input can show the same PlayStation controller both ways; combining them means
        // one press still counts once.
        self.previous = self.current;
        self.current = xinput | sony;
    }

    fn scan_devices(&mut self) {
        self.sony.retain(|_, p| p.alive());
        let pads = gamepads();
        self.other_present = pads.iter().any(|p| p.vendor != SONY);
        for p in pads.into_iter().filter(|p| p.vendor == SONY) {
            if let Some(style) = sony_style(p.product) {
                let path = p.path.clone();
                self.sony
                    .entry(p.path)
                    .or_insert_with(|| SonyPad::open(path, style));
            }
        }
    }

    /// True on the frame the combo is pressed on any controller.
    pub fn pressed(&self, combo: Combo) -> bool {
        is_new_press(combo, self.previous, self.current)
    }

    /// Any of these buttons went down this frame.
    pub fn just_pressed(&self, buttons: u16) -> bool {
        self.current & buttons & !self.previous != 0
    }

    /// Any of these buttons is down.
    pub fn held(&self, buttons: u16) -> bool {
        self.current & buttons != 0
    }

    pub fn any_connected(&self) -> bool {
        self.xinput_connected.contains(&true) || self.sony.values().any(|p| p.alive())
    }

    /// The kind of controller being used, for button names in hints.
    pub fn style(&self) -> PadStyle {
        let sony = self.sony.values().find(|p| p.alive()).map(|p| p.style);
        choose_style(
            sony,
            self.other_present,
            self.last_sony_use,
            self.last_xinput_use,
        )
    }
}

/// A PlayStation controller wins if it's the only gamepad plugged in (any XInput activity is then
/// Steam Input translating it) or if it was used at about the same time as, or after, XInput.
fn choose_style(
    sony: Option<PadStyle>,
    other_present: bool,
    last_sony_use: Option<Instant>,
    last_xinput_use: Option<Instant>,
) -> PadStyle {
    let Some(sony) = sony else {
        return PadStyle::Xbox;
    };
    if !other_present {
        return sony;
    }
    match (last_sony_use, last_xinput_use) {
        (Some(s), Some(x)) if s + Duration::from_secs(1) >= x => sony,
        (Some(_), None) => sony,
        _ => PadStyle::Xbox,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMBO: Combo = Combo {
        hold: Y,
        press: VIEW,
    };

    #[test]
    fn combo_press_is_detected_once_and_only_with_hold_first() {
        assert!(is_new_press(COMBO, Y, Y | VIEW));
        assert!(!is_new_press(COMBO, Y | VIEW, Y | VIEW));
        assert!(!is_new_press(COMBO, 0, VIEW));
        // View held first, then Y: View isn't a new press, so no toggle.
        assert!(!is_new_press(COMBO, VIEW, Y | VIEW));
    }

    #[test]
    fn game_sees_view_alone_but_not_as_part_of_the_combo() {
        let slot = AtomicU32::new(0);
        let combos = [COMBO, Combo { hold: Y, press: A }];
        assert_eq!(hide_combos(&combos, &slot, VIEW), VIEW);
        assert_eq!(hide_combos(&combos, &slot, 0), 0);
        assert_eq!(hide_combos(&combos, &slot, Y), Y);
        assert_eq!(hide_combos(&combos, &slot, Y | VIEW | RB), Y | RB);
        // Let go of Y while still holding View: View stays hidden until it's released.
        assert_eq!(hide_combos(&combos, &slot, VIEW), 0);
        assert_eq!(hide_combos(&combos, &slot, 0), 0);
        assert_eq!(hide_combos(&combos, &slot, VIEW), VIEW);
        // View held before Y isn't the combo, so the game keeps seeing it.
        assert_eq!(hide_combos(&combos, &slot, Y | VIEW), Y | VIEW);
        // The second combo works the same way.
        assert_eq!(hide_combos(&combos, &slot, Y), Y);
        assert_eq!(hide_combos(&combos, &slot, Y | A), Y);
    }

    fn report(id: u8, len: usize, offset: usize, face: u8, shoulders: u8) -> Vec<u8> {
        // 128 everywhere puts the sticks at rest.
        let mut r = vec![128u8; len];
        r[0] = id;
        r[offset] = face;
        r[offset + 1] = shoulders;
        r
    }

    #[test]
    fn parses_every_report_layout() {
        // Triangle (0x80) with the D-pad released (8), and Create/Share (0x10).
        let triangle_create = Some(Y | VIEW);
        assert_eq!(
            parse_sony(&report(0x01, 64, 5, 0x88, 0x10), false),
            triangle_create
        ); // DS4 USB
        assert_eq!(
            parse_sony(&report(0x11, 78, 7, 0x88, 0x10), false),
            triangle_create
        ); // DS4 BT
        assert_eq!(
            parse_sony(&report(0x01, 64, 8, 0x88, 0x10), true),
            triangle_create
        ); // DualSense USB
        assert_eq!(
            parse_sony(&report(0x31, 78, 9, 0x88, 0x10), true),
            triangle_create
        ); // DualSense BT
        assert_eq!(
            parse_sony(&report(0x01, 10, 5, 0x88, 0x10), true),
            triangle_create
        ); // short BT
        assert_eq!(parse_sony(&report(0x05, 64, 5, 0x88, 0x10), false), None);
        assert_eq!(parse_sony(&[0x01, 0, 0], false), None);
    }

    #[test]
    fn maps_every_button() {
        assert_eq!(
            parse_sony(&report(0x01, 64, 5, 0x08 | 0x10 | 0x20 | 0x40, 0), false),
            Some(X | A | B)
        );
        assert_eq!(
            parse_sony(
                &report(0x01, 64, 5, 0x08, 0x01 | 0x02 | 0x20 | 0x40 | 0x80),
                false
            ),
            Some(LB | RB | MENU | L3 | R3)
        );
        let dpad: Vec<_> = (0..9)
            .map(|hat| parse_sony(&report(0x01, 64, 5, hat, 0), false).unwrap())
            .collect();
        assert_eq!(
            dpad,
            [
                UP,
                UP | RIGHT,
                RIGHT,
                DOWN | RIGHT,
                DOWN,
                DOWN | LEFT,
                LEFT,
                UP | LEFT,
                0
            ]
        );
    }

    #[test]
    fn left_stick_navigates() {
        let mut up = report(0x01, 64, 5, 0x08, 0);
        up[2] = 10;
        assert_eq!(parse_sony(&up, false), Some(STICK_UP));
        let mut down = report(0x31, 78, 9, 0x08, 0);
        down[3] = 250;
        assert_eq!(parse_sony(&down, true), Some(STICK_DOWN));
        // XInput sticks: positive is up.
        assert_eq!(stick_bits(30000 * 128 / 32768 + 128, true), STICK_UP);
        assert_eq!(stick_bits(-30000 * 128 / 32768 + 128, true), STICK_DOWN);
        assert_eq!(stick_bits(128, true), 0);
    }

    #[test]
    fn picks_the_controller_in_use() {
        let t = Instant::now();
        let ps5 = Some(PadStyle::PlayStation5);
        assert_eq!(choose_style(None, true, None, Some(t)), PadStyle::Xbox);
        // Only a DualSense: PlayStation names even though XInput (Steam Input) is what moves.
        assert_eq!(
            choose_style(ps5, false, None, Some(t)),
            PadStyle::PlayStation5
        );
        // Both plugged in: whichever was used last, counting near-simultaneous use as PlayStation.
        assert_eq!(choose_style(ps5, true, None, None), PadStyle::Xbox);
        assert_eq!(
            choose_style(ps5, true, Some(t), Some(t + Duration::from_millis(200))),
            PadStyle::PlayStation5
        );
        assert_eq!(
            choose_style(ps5, true, Some(t), Some(t + Duration::from_secs(5))),
            PadStyle::Xbox
        );
        assert_eq!(
            choose_style(ps5, true, Some(t + Duration::from_secs(5)), Some(t)),
            PadStyle::PlayStation5
        );
    }
}
