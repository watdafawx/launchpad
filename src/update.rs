//! Self-update from GitHub releases (same scheme as qalam and razerlite):
//! check, download launchpad.exe, verify its SHA-256 against the release's
//! SHA256SUMS.txt, then swap the running exe (rename old -> launchpad.old.exe,
//! write new, start it with --after-update so it doesn't re-run the startup list).

use std::path::PathBuf;
use std::sync::Mutex;

use windows_sys::Win32::Networking::WinHttp::*;
use windows_sys::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};

const API: &str = "https://api.github.com/repos/watdafawx/launchpad/releases/latest";
const EXE: &str = "launchpad.exe";

#[derive(Clone, Debug)]
pub struct Update {
    pub version: String,
    exe_url: String,
    sums_url: String,
}

/// Update found by the checker but not installed (auto-update off).
pub static FOUND: Mutex<Option<Update>> = Mutex::new(None);

fn parse_ver(s: &str) -> Vec<u32> {
    s.trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

pub fn newer(remote: &str, local: &str) -> bool {
    parse_ver(remote) > parse_ver(local)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// HTTPS GET with WinHTTP (system proxy, follows redirects).
fn get(url: &str) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("https://").ok_or("not https")?;
    let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    unsafe {
        let session = WinHttpOpen(
            wide("launchpad-updater").as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        );
        if session.is_null() {
            return Err("WinHttpOpen failed".into());
        }
        let close = |h: *mut core::ffi::c_void| {
            if !h.is_null() {
                WinHttpCloseHandle(h);
            }
        };
        let conn = WinHttpConnect(session, wide(host).as_ptr(), INTERNET_DEFAULT_HTTPS_PORT as u16, 0);
        let req = if conn.is_null() {
            std::ptr::null_mut()
        } else {
            WinHttpOpenRequest(
                conn,
                wide("GET").as_ptr(),
                wide(&path).as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        };
        let result = (|| {
            if req.is_null() {
                return Err("connect failed".to_string());
            }
            let headers: Vec<u16> = "Accept: application/vnd.github+json\r\n".encode_utf16().collect();
            if WinHttpSendRequest(req, headers.as_ptr(), headers.len() as u32, std::ptr::null(), 0, 0, 0) == 0 {
                return Err("send failed".into());
            }
            if WinHttpReceiveResponse(req, std::ptr::null_mut()) == 0 {
                return Err("no response".into());
            }
            let mut status = 0u32;
            let mut size = 4u32;
            WinHttpQueryHeaders(
                req,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                std::ptr::null(),
                &mut status as *mut u32 as *mut _,
                &mut size,
                std::ptr::null_mut(),
            );
            if status != 200 {
                return Err(format!("HTTP {status}"));
            }
            let mut body = Vec::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let mut n = 0u32;
                if WinHttpReadData(req, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut n) == 0 {
                    return Err("read failed".into());
                }
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&buf[..n as usize]);
                if body.len() > 64 * 1024 * 1024 {
                    return Err("response too big".into());
                }
            }
            Ok(body)
        })();
        close(req);
        close(conn);
        close(session);
        result
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut out = [0u8; 32];
    unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, std::ptr::null(), 0, data.as_ptr(), data.len() as u32, out.as_mut_ptr(), 32) };
    out.iter().map(|b| format!("{b:02x}")).collect()
}

/// Ask GitHub for the latest release; Some if it's newer than this build.
pub fn check() -> Result<Option<Update>, String> {
    let body = get(API)?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or("no tag")?.to_string();
    if !newer(&tag, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let asset = |name: &str| {
        v["assets"].as_array().and_then(|a| {
            a.iter().find(|x| x["name"] == name).and_then(|x| x["browser_download_url"].as_str()).map(str::to_string)
        })
    };
    let exe_url = asset(EXE).ok_or("release has no launchpad.exe")?;
    let sums_url = asset("SHA256SUMS.txt").ok_or("release has no SHA256SUMS.txt")?;
    Ok(Some(Update { version: tag, exe_url, sums_url }))
}

fn old_path() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.with_file_name("launchpad.old.exe"))
}

/// Remove the previous exe left behind by an update.
pub fn cleanup() {
    if let Some(p) = old_path() {
        let _ = std::fs::remove_file(p);
    }
}

/// Dev builds (running from cargo's target folder) never replace themselves.
pub fn is_dev_build() -> bool {
    std::env::current_exe().is_ok_and(|p| p.to_string_lossy().to_lowercase().contains("\\target\\"))
}

/// Download, verify and swap in the new exe, then start it. Caller quits afterwards.
pub fn apply(u: &Update) -> Result<(), String> {
    if is_dev_build() {
        return Err("dev build: not replacing it (install launchpad first)".into());
    }
    let exe = get(&u.exe_url)?;
    let sums = String::from_utf8_lossy(&get(&u.sums_url)?).to_string();
    let want = sums
        .lines()
        .find_map(|l| l.split_once("  ").filter(|(_, n)| n.trim() == EXE).map(|(h, _)| h.trim().to_lowercase()))
        .ok_or("launchpad.exe missing from SHA256SUMS.txt")?;
    let got = sha256_hex(&exe);
    if got != want {
        return Err(format!("checksum mismatch (got {got})"));
    }
    if exe.len() < 100_000 || &exe[..2] != b"MZ" {
        return Err("download isn't an exe".into());
    }
    let cur = std::env::current_exe().map_err(|e| e.to_string())?;
    let old = old_path().ok_or("no exe path")?;
    let _ = std::fs::remove_file(&old);
    // A running exe can't be overwritten, but it can be renamed.
    std::fs::rename(&cur, &old).map_err(|e| format!("rename: {e}"))?;
    if let Err(e) = std::fs::write(&cur, &exe) {
        let _ = std::fs::rename(&old, &cur);
        return Err(format!("write: {e}"));
    }
    std::process::Command::new(&cur).arg("--after-update").spawn().map_err(|e| e.to_string())?;
    Ok(())
}

fn log(msg: &str) {
    use std::io::Write;
    let p = crate::config::dir().join("update.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "[{}] {msg}", crate::suggest::now());
    }
}

/// Background checker: shortly after start, then every 12 hours.
pub fn spawn() {
    std::thread::Builder::new()
        .name("update".into())
        .spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(60));
            loop {
                check_now(false);
                std::thread::sleep(std::time::Duration::from_secs(12 * 3600));
            }
        })
        .ok();
}

/// Check, and install if auto-update is on. `manual` = from the tray menu:
/// always installs and reports "up to date" and errors in a message box.
pub fn check_now(manual: bool) {
    let r = check();
    log(&format!("check ({}): {r:?}", if manual { "manual" } else { "auto" }));
    match r {
        Ok(Some(u)) => {
            let auto = crate::config::load().map(|c| c.auto_update).unwrap_or(true);
            if manual || auto {
                install(u, manual);
            } else {
                *FOUND.lock().unwrap() = Some(u);
                crate::tray::post(crate::tray::Msg::UpdateFound);
            }
        }
        Ok(None) if manual => {
            crate::win::msgbox(&format!("launchpad is up to date (v{}).", env!("CARGO_PKG_VERSION")))
        }
        Err(e) if manual => crate::win::msgbox(&format!("Couldn't check for updates:\n{e}")),
        _ => {}
    }
}

/// Install now; the tray quits once the new exe has started.
pub fn install(u: Update, manual: bool) {
    match apply(&u) {
        Ok(()) => {
            log(&format!("installed {}", u.version));
            crate::tray::post(crate::tray::Msg::Quit);
        }
        Err(e) => {
            log(&format!("install {} failed: {e}", u.version));
            if manual {
                crate::win::msgbox(&format!("launchpad update failed:\n{e}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn versions() {
        assert!(super::newer("v0.1.10", "0.1.9"));
        assert!(!super::newer("v0.1.4", "0.1.4"));
        assert!(super::newer("v1.0.0", "0.9.9"));
    }

    #[test]
    fn sha() {
        assert_eq!(super::sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
