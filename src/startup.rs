//! Windows startup entries: list them, enable/disable them the same way Task
//! Manager does (StartupApproved), and watch for apps re-enabling themselves.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use winreg::enums::*;
use winreg::{RegKey, RegValue};

use crate::config::{Config, Source, StartupRef};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

#[derive(Clone, Debug)]
pub struct Item {
    pub r: StartupRef,
    pub command: String,
    pub enabled: bool,
}

fn hive(hklm: bool) -> RegKey {
    RegKey::predef(if hklm { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER })
}

/// (is HKLM, StartupApproved subkey) for a source.
fn approved_loc(src: Source) -> (bool, &'static str) {
    match src {
        Source::HkcuRun => (false, "Run"),
        Source::HklmRun => (true, "Run"),
        Source::HklmRun32 => (true, "Run32"),
        Source::UserFolder => (false, "StartupFolder"),
        Source::CommonFolder => (true, "StartupFolder"),
    }
}

pub fn is_enabled(r: &StartupRef) -> bool {
    let (hklm, sub) = approved_loc(r.source);
    hive(hklm)
        .open_subkey(format!(r"{APPROVED}\{sub}"))
        .and_then(|k| k.get_raw_value(&r.name))
        .map(|v| v.bytes.first().map_or(true, |b| b & 1 == 0))
        .unwrap_or(true)
}

pub fn set_enabled(r: &StartupRef, on: bool) -> Result<(), String> {
    if is_enabled(r) == on {
        return Ok(());
    }
    let (hklm, sub) = approved_loc(r.source);
    let (k, _) = hive(hklm)
        .create_subkey(format!(r"{APPROVED}\{sub}"))
        .map_err(|e| format!("{}: {e}", r.name))?;
    let mut bytes = vec![0u8; 12];
    if on {
        bytes[0] = 2;
    } else {
        bytes[0] = 3;
        // Task Manager stores when the entry was disabled as a FILETIME.
        let ft = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() / 100
            + 116_444_736_000_000_000;
        bytes[4..12].copy_from_slice(&(ft as u64).to_le_bytes());
    }
    k.set_raw_value(&r.name, &RegValue { bytes, vtype: REG_BINARY })
        .map_err(|e| format!("{}: {e}", r.name))
}

fn user_startup_dir() -> PathBuf {
    PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
        .join(r"Microsoft\Windows\Start Menu\Programs\Startup")
}

fn common_startup_dir() -> PathBuf {
    PathBuf::from(std::env::var("ProgramData").unwrap_or_default())
        .join(r"Microsoft\Windows\Start Menu\Programs\StartUp")
}

pub fn scan() -> Vec<Item> {
    let mut out = Vec::new();
    let runs = [
        (Source::HkcuRun, false, KEY_READ),
        (Source::HklmRun, true, KEY_READ | KEY_WOW64_64KEY),
        (Source::HklmRun32, true, KEY_READ | KEY_WOW64_32KEY),
    ];
    for (source, hklm, flags) in runs {
        let Ok(k) = hive(hklm).open_subkey_with_flags(RUN, flags) else { continue };
        for (name, _) in k.enum_values().flatten() {
            let command: String = k.get_value(&name).unwrap_or_default();
            let r = StartupRef { source, name };
            out.push(Item { enabled: is_enabled(&r), r, command });
        }
    }
    for (source, dir) in [(Source::UserFolder, user_startup_dir()), (Source::CommonFolder, common_startup_dir())] {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let r = StartupRef { source, name };
            out.push(Item { enabled: is_enabled(&r), r, command: e.path().to_string_lossy().into_owned() });
        }
    }
    out.sort_by_key(|i| i.r.name.to_lowercase());
    out
}

/// Disable every startup entry the config manages. Returns errors, if any.
pub fn apply_guard(cfg: &Config) -> Vec<String> {
    cfg.apps
        .iter()
        .filter_map(|a| a.startup.as_ref())
        .filter_map(|r| set_enabled(r, false).err())
        .collect()
}

/// Block forever, calling `f` whenever a Run key or StartupApproved key changes.
pub fn watch(f: impl Fn()) {
    use windows_sys::Win32::System::Registry::{RegNotifyChangeKeyValue, REG_NOTIFY_CHANGE_LAST_SET};
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects, INFINITE};

    let mut keys: Vec<RegKey> = Vec::new();
    let watch_flags = KEY_NOTIFY | KEY_READ;
    for (hklm, flags) in [(false, 0), (true, KEY_WOW64_64KEY), (true, KEY_WOW64_32KEY)] {
        if let Ok(k) = hive(hklm).open_subkey_with_flags(RUN, watch_flags | flags) {
            keys.push(k);
        }
    }
    for (hklm, sub) in [(false, "Run"), (false, "StartupFolder"), (true, "Run"), (true, "Run32"), (true, "StartupFolder")] {
        if let Ok((k, _)) = hive(hklm).create_subkey_with_flags(format!(r"{APPROVED}\{sub}"), watch_flags) {
            keys.push(k);
        }
    }
    if keys.is_empty() {
        return;
    }

    unsafe {
        let events: Vec<_> = keys.iter().map(|_| CreateEventW(std::ptr::null(), 0, 0, std::ptr::null())).collect();
        let arm = |i: usize| {
            RegNotifyChangeKeyValue(keys[i].raw_handle() as _, 0, REG_NOTIFY_CHANGE_LAST_SET, events[i], 1);
        };
        (0..keys.len()).for_each(arm);
        loop {
            let r = WaitForMultipleObjects(events.len() as u32, events.as_ptr(), 0, INFINITE) as usize;
            if r >= events.len() {
                return;
            }
            arm(r);
            // Let an installer finish writing before we react.
            std::thread::sleep(Duration::from_millis(750));
            f();
        }
    }
}
