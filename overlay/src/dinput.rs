//! DirectInput 8, which the game uses for keyboard and mouse.
//!
//! - When the overlay is installed as dinput8.dll, DirectInput8Create is forwarded to the real
//!   dinput8.dll in System32.
//! - While the catch-up menu is open, the game's keyboard and mouse reads come back empty, so
//!   browsing the menu with the arrow keys doesn't also switch items or turn the camera.

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::OnceLock;

use hudhook::mh::{MH_ApplyQueued, MhHook};
use hudhook::windows::core::{s, GUID, PCWSTR};
use hudhook::windows::Win32::System::LibraryLoader::{
    GetModuleHandleW, GetProcAddress, LoadLibraryW,
};
use hudhook::windows::Win32::System::SystemInformation::GetSystemDirectoryW;

const E_FAIL: i32 = 0x80004005u32 as i32;
const DIRECTINPUT_VERSION: u32 = 0x0800;
const IID_IDIRECTINPUT8A: GUID = GUID::from_u128(0xbf798030_483a_4da2_aa99_5d64ed369700);
const IID_IDIRECTINPUT8W: GUID = GUID::from_u128(0xbf798031_483a_4da2_aa99_5d64ed369700);
const GUID_SYS_KEYBOARD: GUID = GUID::from_u128(0x6f1d2b61_d5a0_11cf_bfc7_444553540000);

type CreateFn =
    unsafe extern "system" fn(*mut c_void, u32, *const GUID, *mut *mut c_void, *mut c_void) -> i32;
type CreateDeviceFn =
    unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void, *mut c_void) -> i32;
type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;
type GetDeviceStateFn = unsafe extern "system" fn(*mut c_void, u32, *mut c_void) -> i32;
type GetDeviceDataFn =
    unsafe extern "system" fn(*mut c_void, u32, *mut c_void, *mut u32, u32) -> i32;

/// DirectInput8Create from System32's dinput8.dll.
fn real_create() -> Option<CreateFn> {
    static REAL: OnceLock<Option<CreateFn>> = OnceLock::new();
    *REAL.get_or_init(|| unsafe {
        let mut dir = [0u16; 260];
        let len = GetSystemDirectoryW(Some(&mut dir)) as usize;
        let mut path: Vec<u16> = dir[..len].to_vec();
        path.extend("\\dinput8.dll\0".encode_utf16());
        let module = LoadLibraryW(PCWSTR(path.as_ptr())).ok()?;
        let f = GetProcAddress(module, s!("DirectInput8Create"))?;
        Some(std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            CreateFn,
        >(f))
    })
}

/// Lets the DLL stand in for dinput8.dll: the game imports only this function from it.
///
/// # Safety
/// Called by the game with the same arguments as the real DirectInput8Create.
#[no_mangle]
pub unsafe extern "system" fn DirectInput8Create(
    instance: *mut c_void,
    version: u32,
    riid: *const GUID,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> i32 {
    match real_create() {
        Some(f) => unsafe { f(instance, version, riid, out, outer) },
        None => E_FAIL,
    }
}

// Separate trampolines for the ANSI and wide interfaces, in case dinput8 implements them apart.
static STATE_ORIGINAL: [OnceLock<GetDeviceStateFn>; 2] = [OnceLock::new(), OnceLock::new()];
static DATA_ORIGINAL: [OnceLock<GetDeviceDataFn>; 2] = [OnceLock::new(), OnceLock::new()];

unsafe extern "system" fn get_device_state<const I: usize>(
    this: *mut c_void,
    size: u32,
    data: *mut c_void,
) -> i32 {
    let Some(original) = STATE_ORIGINAL[I].get() else {
        return E_FAIL;
    };
    let result = unsafe { original(this, size, data) };
    if result >= 0 && crate::input_blocked() && !data.is_null() {
        unsafe { std::ptr::write_bytes(data.cast::<u8>(), 0, size as usize) };
    }
    result
}

unsafe extern "system" fn get_device_data<const I: usize>(
    this: *mut c_void,
    object_size: u32,
    data: *mut c_void,
    count: *mut u32,
    flags: u32,
) -> i32 {
    let Some(original) = DATA_ORIGINAL[I].get() else {
        return E_FAIL;
    };
    let result = unsafe { original(this, object_size, data, count, flags) };
    if result >= 0 && crate::input_blocked() && !count.is_null() {
        // The events were taken from the buffer; report none.
        unsafe { *count = 0 };
    }
    result
}

/// GetDeviceState and GetDeviceData of a DirectInput device, found through a throwaway keyboard
/// device. Every device of the same interface shares these.
unsafe fn device_methods(iid: &GUID) -> Option<(usize, usize)> {
    let create = real_create()?;
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        let mut input: *mut c_void = null_mut();
        if create(module.0, DIRECTINPUT_VERSION, iid, &mut input, null_mut()) < 0 || input.is_null()
        {
            return None;
        }
        let vtable = *(input as *const *const usize);
        let create_device: CreateDeviceFn = std::mem::transmute(*vtable.add(3));
        let release_input: ReleaseFn = std::mem::transmute(*vtable.add(2));
        let mut device: *mut c_void = null_mut();
        let methods = if create_device(input, &GUID_SYS_KEYBOARD, &mut device, null_mut()) >= 0
            && !device.is_null()
        {
            let device_vtable = *(device as *const *const usize);
            let release_device: ReleaseFn = std::mem::transmute(*device_vtable.add(2));
            // IDirectInputDevice8: 9 is GetDeviceState, 10 is GetDeviceData.
            let found = (*device_vtable.add(9), *device_vtable.add(10));
            release_device(device);
            Some(found)
        } else {
            None
        };
        release_input(input);
        methods
    }
}

/// Hooks DirectInput so `input_blocked()` can hide keyboard and mouse input from the game. Call
/// once, after hudhook has set up MinHook. Returns whether anything was hooked.
pub fn install() -> bool {
    let mut hooked: Vec<usize> = Vec::new();
    for (i, iid) in [IID_IDIRECTINPUT8A, IID_IDIRECTINPUT8W].iter().enumerate() {
        let Some((state, data)) = (unsafe { device_methods(iid) }) else {
            continue;
        };
        let (state_hook, data_hook) = match i {
            0 => (
                get_device_state::<0> as *mut c_void,
                get_device_data::<0> as *mut c_void,
            ),
            _ => (
                get_device_state::<1> as *mut c_void,
                get_device_data::<1> as *mut c_void,
            ),
        };
        unsafe {
            if !hooked.contains(&state) {
                if let Ok(hook) = MhHook::new(state as *mut c_void, state_hook) {
                    STATE_ORIGINAL[i].get_or_init(|| std::mem::transmute(hook.trampoline()));
                    if hook.queue_enable().is_ok() {
                        hooked.push(state);
                    }
                }
            }
            if !hooked.contains(&data) {
                if let Ok(hook) = MhHook::new(data as *mut c_void, data_hook) {
                    DATA_ORIGINAL[i].get_or_init(|| std::mem::transmute(hook.trampoline()));
                    if hook.queue_enable().is_ok() {
                        hooked.push(data);
                    }
                }
            }
        }
    }
    !hooked.is_empty() && unsafe { MH_ApplyQueued() }.ok().is_ok()
}
