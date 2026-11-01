//! Thin wrappers over the Win32 calls launchpad needs.

use std::collections::HashSet;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use windows_sys::Win32::System::Environment::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut tok: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) == 0 {
            return false;
        }
        let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(
            tok,
            TokenElevation,
            &mut e as *mut _ as *mut c_void,
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(tok);
        ok != 0 && e.TokenIsElevated != 0
    }
}

/// A primary token handle that can be shared between launcher threads.
pub struct Token(HANDLE);
unsafe impl Send for Token {}
unsafe impl Sync for Token {}
impl Drop for Token {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// The desktop shell's (Explorer's) token: the user's normal, non-elevated
/// rights. Waits for Explorer to come up, since we may start before it.
pub fn shell_token(timeout: Duration) -> Option<Token> {
    let end = Instant::now() + timeout;
    loop {
        if let Some(t) = try_shell_token() {
            return Some(t);
        }
        if Instant::now() > end {
            return None;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn try_shell_token() -> Option<Token> {
    unsafe {
        let hwnd = GetShellWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return None;
        }
        let hp = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if hp.is_null() {
            return None;
        }
        let mut t: HANDLE = null_mut();
        let ok = OpenProcessToken(hp, TOKEN_DUPLICATE | TOKEN_QUERY, &mut t);
        CloseHandle(hp);
        if ok == 0 {
            return None;
        }
        let mut p: HANDLE = null_mut();
        let ok = DuplicateTokenEx(
            t,
            TOKEN_QUERY
                | TOKEN_DUPLICATE
                | TOKEN_ASSIGN_PRIMARY
                | TOKEN_ADJUST_DEFAULT
                | TOKEN_ADJUST_SESSIONID,
            null(),
            SecurityImpersonation,
            TokenPrimary,
            &mut p,
        );
        CloseHandle(t);
        (ok != 0).then(|| Token(p))
    }
}

/// Start a process with the given (non-elevated) user token.
pub fn spawn_as(tok: &Token, cmd: &str, cwd: Option<&str>) -> Result<u32, String> {
    unsafe {
        let mut cmdw = wide(cmd);
        let cwdw = cwd.map(wide);
        let mut desk = wide("winsta0\\default");
        let mut si: STARTUPINFOW = zeroed();
        si.cb = size_of::<STARTUPINFOW>() as u32;
        si.lpDesktop = desk.as_mut_ptr();
        let mut pi: PROCESS_INFORMATION = zeroed();

        let mut env: *mut c_void = null_mut();
        let have_env = CreateEnvironmentBlock(&mut env, tok.0, 0) != 0;
        let flags = NORMAL_PRIORITY_CLASS | if have_env { CREATE_UNICODE_ENVIRONMENT } else { 0 };

        let ok = CreateProcessWithTokenW(
            tok.0,
            0,
            null(),
            cmdw.as_mut_ptr(),
            flags,
            if have_env { env } else { null() },
            cwdw.as_ref().map_or(null(), |w| w.as_ptr()),
            &si,
            &mut pi,
        );
        let err = GetLastError();
        if have_env {
            DestroyEnvironmentBlock(env);
        }
        if ok == 0 {
            return Err(format!("CreateProcessWithTokenW failed (error {err})"));
        }
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
        Ok(pi.dwProcessId)
    }
}

/// Start a process with our own token (elevated if we are).
pub fn spawn_direct(cmd: &str, cwd: Option<&str>) -> Result<u32, String> {
    // Break away from the Task Scheduler job so ending the task can't take
    // children down with it. Falls back if the job doesn't allow it.
    match create_process(cmd, cwd, NORMAL_PRIORITY_CLASS | CREATE_BREAKAWAY_FROM_JOB) {
        Err(ERROR_ACCESS_DENIED) => create_process(cmd, cwd, NORMAL_PRIORITY_CLASS),
        r => r,
    }
    .map_err(|e| format!("CreateProcessW failed (error {e})"))
}

fn create_process(cmd: &str, cwd: Option<&str>, flags: u32) -> Result<u32, u32> {
    unsafe {
        let mut cmdw = wide(cmd);
        let cwdw = cwd.map(wide);
        let mut si: STARTUPINFOW = zeroed();
        si.cb = size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = zeroed();
        let ok = CreateProcessW(
            null(),
            cmdw.as_mut_ptr(),
            null(),
            null(),
            0,
            flags,
            null(),
            cwdw.as_ref().map_or(null(), |w| w.as_ptr()),
            &si,
            &mut pi,
        );
        if ok == 0 {
            return Err(GetLastError());
        }
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
        Ok(pi.dwProcessId)
    }
}

/// ShellExecute with a verb ("open", "runas"). Used for .lnk / shell: targets
/// and for UAC prompts when launchpad itself isn't elevated.
pub fn shell_execute(verb: &str, file: &str, params: &str) -> Result<(), String> {
    let (v, f, p) = (wide(verb), wide(file), wide(params));
    let r = unsafe {
        ShellExecuteW(null_mut(), v.as_ptr(), f.as_ptr(), p.as_ptr(), null(), SW_SHOWNORMAL)
    };
    if r as isize > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecute failed (code {})", r as isize))
    }
}

/// Lower-case exe names of every running process.
pub fn running_processes() -> HashSet<String> {
    let mut out = HashSet::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                out.insert(String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase());
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

fn ft(f: &FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

fn cpu_times() -> (u64, u64) {
    unsafe {
        let (mut i, mut k, mut u): (FILETIME, FILETIME, FILETIME) = (zeroed(), zeroed(), zeroed());
        GetSystemTimes(&mut i, &mut k, &mut u);
        // Kernel time includes idle time.
        (ft(&i), ft(&k) + ft(&u))
    }
}

/// Total CPU use (0–100) measured over `d`.
pub fn cpu_percent_over(d: Duration) -> f32 {
    let (i0, t0) = cpu_times();
    std::thread::sleep(d);
    let (i1, t1) = cpu_times();
    let total = t1.saturating_sub(t0);
    if total == 0 {
        return 0.0;
    }
    100.0 * (1.0 - i1.saturating_sub(i0) as f32 / total as f32)
}

pub fn expand_env(s: &str) -> String {
    let src = wide(s);
    unsafe {
        let n = ExpandEnvironmentStringsW(src.as_ptr(), null_mut(), 0);
        if n == 0 {
            return s.to_string();
        }
        let mut buf = vec![0u16; n as usize];
        let n = ExpandEnvironmentStringsW(src.as_ptr(), buf.as_mut_ptr(), n);
        if n == 0 {
            return s.to_string();
        }
        String::from_utf16_lossy(&buf[..(n as usize).saturating_sub(1)])
    }
}

pub fn msgbox(text: &str) {
    let (t, c) = (wide(text), wide("launchpad"));
    unsafe { MessageBoxW(null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONINFORMATION) };
}

/// False if another instance already holds this name. The handle is kept
/// open for the life of the process on purpose.
pub fn single_instance(name: &str) -> bool {
    let n = wide(name);
    unsafe {
        let h = CreateMutexW(null(), 1, n.as_ptr());
        if h.is_null() {
            return false;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // Close it, or our handle keeps the mutex alive after the other
            // instance exits and every retry would fail.
            CloseHandle(h);
            return false;
        }
        true
    }
}

/// Icon for an exe, dll, .ico or shortcut as (RGBA pixels, size).
/// Exes get a 48 px icon; anything else gets the shell's 32 px one.
pub fn file_icon_rgba(path: &str) -> Option<(Vec<u8>, usize)> {
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};

    let w = wide(path);
    let mut hicon: HICON = null_mut();
    unsafe {
        let lower = path.to_lowercase();
        if lower.ends_with(".exe") || lower.ends_with(".dll") || lower.ends_with(".ico") {
            let mut id = 0u32;
            let n = PrivateExtractIconsW(w.as_ptr(), 0, 48, 48, &mut hicon, &mut id, 1, 0);
            if n == 0 || n == u32::MAX {
                hicon = null_mut();
            }
        }
        if hicon.is_null() {
            let mut sfi: SHFILEINFOW = zeroed();
            SHGetFileInfoW(
                w.as_ptr(),
                FILE_ATTRIBUTE_NORMAL,
                &mut sfi,
                size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_LARGEICON,
            );
            hicon = sfi.hIcon;
        }
        if hicon.is_null() {
            return None;
        }
        let r = hicon_to_rgba(hicon);
        DestroyIcon(hicon);
        r
    }
}

unsafe fn hicon_to_rgba(hicon: HICON) -> Option<(Vec<u8>, usize)> {
    use windows_sys::Win32::Graphics::Gdi::*;

    let mut ii: ICONINFO = zeroed();
    if GetIconInfo(hicon, &mut ii) == 0 {
        return None;
    }
    let cleanup = |ii: &ICONINFO| {
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask);
        }
    };
    if ii.hbmColor.is_null() {
        cleanup(&ii);
        return None; // monochrome icon; not worth handling
    }
    let mut bm: BITMAP = zeroed();
    GetObjectW(ii.hbmColor, size_of::<BITMAP>() as i32, &mut bm as *mut _ as *mut c_void);
    let (w, h) = (bm.bmWidth as usize, bm.bmHeight as usize);
    if w == 0 || w != h {
        cleanup(&ii);
        return None;
    }

    let mut bmi: BITMAPINFO = zeroed();
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w as i32;
    bmi.bmiHeader.biHeight = -(h as i32); // top-down
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;

    let mut px = vec![0u8; w * h * 4];
    let dc = CreateCompatibleDC(null_mut());
    let lines = GetDIBits(dc, ii.hbmColor, 0, h as u32, px.as_mut_ptr() as *mut c_void, &mut bmi, DIB_RGB_COLORS);
    DeleteDC(dc);
    cleanup(&ii);
    if lines == 0 {
        return None;
    }
    // BGRA -> RGBA. Old icons have no alpha channel: treat as opaque.
    let has_alpha = px.chunks_exact(4).any(|p| p[3] != 0);
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
        if !has_alpha {
            p[3] = 255;
        }
    }
    Some((px, w))
}

/// Like `single_instance`, but keeps trying for up to `secs` seconds.
pub fn single_instance_wait(name: &str, secs: u64) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if single_instance(name) {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

pub fn relaunch_elevated(arg: &str) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = shell_execute("runas", &exe.to_string_lossy(), arg);
    }
}
