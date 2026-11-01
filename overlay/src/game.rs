//! Reads, never writes, the few things the compass needs from game memory: the player's map and
//! position, which way the camera faces, and the dungeon-to-overworld table.
//!
//! The structure layouts come from fromsoftware-rs and only match one game version, so nothing is
//! read unless the game is that version. A game patch turns the compass off until the overlay is
//! rebuilt against an updated fromsoftware-rs; the catch-up list keeps working.
//!
//! Pointers can go stale during loading screens, so values are copied out with
//! ReadProcessMemory on our own process: a bad address gives an error instead of a crash.

use std::ffi::c_void;
use std::mem::{offset_of, size_of, MaybeUninit};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use eldenring::cs::{
    CSCam, CSCamera, PlayerIns, SoloParamRepository, WorldChrMan, WorldMapLegacyConvParam,
};
use eldenring::position::BlockPosition;
use fromsoftware_shared::{F32ViewMatrix, FromStatic, Program};
use hudhook::windows::core::PCWSTR;
use hudhook::windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use hudhook::windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use hudhook::windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
};

use crate::compass::{ConvRow, MapId};

/// The game versions the pinned fromsoftware-rs supports (English and Japanese builds).
const SUPPORTED: [&str; 2] = ["2.7.2.0", "2.7.2.1"];

const STARTING: u8 = 0;
const READY: u8 = 1;
const UNSUPPORTED: u8 = 2;
static STATE: AtomicU8 = AtomicU8::new(STARTING);
static PROBLEM: OnceLock<String> = OnceLock::new();
static CONV: OnceLock<Vec<ConvRow>> = OnceLock::new();

/// Checks the game version and waits for the game to finish starting up. Runs on its own thread.
pub fn init() {
    let fail = |why: String| {
        PROBLEM.get_or_init(|| why);
        STATE.store(UNSUPPORTED, Ordering::SeqCst);
    };
    match exe_version() {
        Some((name, version))
            if name.eq_ignore_ascii_case("eldenring.exe")
                && SUPPORTED.contains(&version.as_str()) => {}
        Some((name, version)) if name.eq_ignore_ascii_case("eldenring.exe") => {
            return fail(format!(
                "Compass needs Elden Ring {}; this is {version}",
                SUPPORTED[0]
            ));
        }
        _ => return fail("Compass only works inside Elden Ring".into()),
    }
    // The crate's startup wait panics on versions it doesn't know; the check above rules that out,
    // but a panic here must never take the game down.
    let waited = std::panic::catch_unwind(|| {
        eldenring::util::system::wait_for_system_init(&Program::current(), Duration::from_secs(600))
            .is_ok()
    });
    match waited {
        Ok(true) => {
            // Find the singletons now, so the first lookup doesn't stall a frame.
            let _ = (
                WorldChrMan::instance_ptr(),
                CSCamera::instance_ptr(),
                SoloParamRepository::instance_ptr(),
            );
            STATE.store(READY, Ordering::SeqCst);
        }
        _ => fail("Compass couldn't attach to the game".into()),
    }
}

/// Why the compass can't work at all, if it can't.
pub fn unavailable() -> Option<&'static str> {
    match STATE.load(Ordering::SeqCst) {
        UNSUPPORTED => PROBLEM.get().map(String::as_str),
        STARTING => Some("Compass is starting"),
        _ => None,
    }
}

/// The running exe's file name and version, e.g. ("eldenring.exe", "2.7.2.0").
fn exe_version() -> Option<(String, String)> {
    let mut path = [0u16; 1024];
    let len = unsafe { GetModuleFileNameW(None, &mut path) } as usize;
    let path = &path[..len];
    let name = String::from_utf16_lossy(path)
        .rsplit(['\\', '/'])
        .next()?
        .to_string();
    let mut path_z = path.to_vec();
    path_z.push(0);
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(path_z.as_ptr()), None);
        if size == 0 {
            return Some((name, String::new()));
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(
            PCWSTR(path_z.as_ptr()),
            None,
            size,
            data.as_mut_ptr().cast(),
        )
        .ok()?;
        let mut info: *mut c_void = std::ptr::null_mut();
        let mut info_len = 0u32;
        let root: Vec<u16> = "\\\0".encode_utf16().collect();
        if !VerQueryValueW(
            data.as_ptr().cast(),
            PCWSTR(root.as_ptr()),
            &mut info,
            &mut info_len,
        )
        .as_bool()
            || info.is_null()
        {
            return Some((name, String::new()));
        }
        let fixed = &*(info as *const VS_FIXEDFILEINFO);
        let (ms, ls) = (fixed.dwProductVersionMS, fixed.dwProductVersionLS);
        Some((
            name,
            format!("{}.{}.{}.{}", ms >> 16, ms & 0xFFFF, ls >> 16, ls & 0xFFFF),
        ))
    }
}

/// Copies a value out of game memory, or None if the address isn't readable.
fn read<T: Copy>(address: usize) -> Option<T> {
    if address == 0 {
        return None;
    }
    let mut value = MaybeUninit::<T>::uninit();
    let mut copied = 0usize;
    unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            address as *const c_void,
            value.as_mut_ptr().cast(),
            size_of::<T>(),
            Some(&mut copied),
        )
        .ok()?;
        (copied == size_of::<T>()).then(|| value.assume_init())
    }
}

/// What the compass needs about the player this frame.
pub struct Snapshot {
    pub map: MapId,
    /// Position within the map, in meters.
    pub local: [f32; 3],
    /// Camera forward direction on the ground (x, z), not normalized.
    pub forward: (f32, f32),
}

pub fn snapshot() -> Result<Snapshot, &'static str> {
    if let Some(why) = unavailable() {
        return Err(why);
    }
    const WAITING: &str = "Load a character to use the compass";
    let chr_man = WorldChrMan::instance_ptr().map_err(|_| WAITING)? as usize;
    let player: usize = read(chr_man + offset_of!(WorldChrMan, main_player)).ok_or(WAITING)?;
    let position: BlockPosition =
        read(player + offset_of!(PlayerIns, block_position)).ok_or(WAITING)?;
    let block: i32 = read(player + offset_of!(PlayerIns, current_block_id)).ok_or(WAITING)?;
    if block == -1 {
        return Err(WAITING);
    }
    let block = block as u32;
    let map = MapId {
        area: (block >> 24) as u8,
        x: (block >> 16) as u8,
        z: (block >> 8) as u8,
        index: block as u8,
    };

    let camera = CSCamera::instance_ptr().map_err(|_| WAITING)? as usize;
    let pers_cam: usize = read(camera + offset_of!(CSCamera, pers_cam_1)).ok_or(WAITING)?;
    let matrix: F32ViewMatrix = read(pers_cam + offset_of!(CSCam, matrix)).ok_or(WAITING)?;
    // Row 2 is the camera's forward axis.
    let forward = (matrix.2 .0, matrix.2 .2);

    Ok(Snapshot {
        map,
        local: [position.x, position.y, position.z],
        forward,
    })
}

/// WorldMapLegacyConvParam, copied once the game has loaded its params.
pub fn conversion_table() -> &'static [ConvRow] {
    if let Some(rows) = CONV.get() {
        return rows;
    }
    // Params load after system init but before a character can be loaded, so only read them once
    // there's a player.
    if snapshot().is_err() {
        return &[];
    }
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        return &[];
    };
    let rows: Vec<ConvRow> = repo
        .rows::<WorldMapLegacyConvParam>()
        .filter(|(_, r)| !r.disable_param_nt())
        .map(|(_, r)| ConvRow {
            src: MapId {
                area: r.src_area_no(),
                x: r.src_grid_x_no(),
                z: r.src_grid_z_no(),
                index: 0,
            },
            src_pos: [r.src_pos_x(), r.src_pos_y(), r.src_pos_z()],
            dst: MapId {
                area: r.dst_area_no(),
                x: r.dst_grid_x_no(),
                z: r.dst_grid_z_no(),
                index: 0,
            },
            dst_pos: [r.dst_pos_x(), r.dst_pos_y(), r.dst_pos_z()],
            base_point: r.is_base_point(),
        })
        .collect();
    if rows.is_empty() {
        // Params not loaded yet; try again next time.
        return &[];
    }
    CONV.get_or_init(|| rows)
}
