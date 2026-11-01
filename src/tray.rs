use std::mem::zeroed;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, Ordering};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PostThreadMessageW, TranslateMessage, MSG, WM_APP,
};

/// Messages from background threads to the tray thread.
#[derive(Clone, Copy)]
pub enum Msg {
    UpdateFound = 1,
    Quit = 2,
}

static TRAY_THREAD: AtomicU32 = AtomicU32::new(0);

/// Wake the tray thread with a message. No-op if the tray isn't running.
pub fn post(m: Msg) {
    let tid = TRAY_THREAD.load(Ordering::SeqCst);
    if tid != 0 {
        unsafe { PostThreadMessageW(tid, WM_APP, m as usize, 0) };
    } else if matches!(m, Msg::Quit) {
        std::process::exit(0);
    }
}

/// Tray icon + message loop. Blocks until Quit.
pub fn run() {
    TRAY_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);

    let menu = Menu::new();
    let settings = MenuItem::new("Settings…", true, None);
    let log = MenuItem::new("Open last-run log", true, None);
    let run_now = MenuItem::new("Run startup list now", true, None);
    let updates = MenuItem::new(format!("Check for updates (v{})", env!("CARGO_PKG_VERSION")), true, None);
    let quit = MenuItem::new("Quit launchpad", true, None);
    let sep = || PredefinedMenuItem::separator();
    let _ = menu.append_items(&[&settings, &log, &run_now, &sep(), &updates, &sep(), &quit]);

    let Ok(tray) = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("launchpad")
        .with_icon(icon())
        .build()
    else {
        return;
    };

    let rx = MenuEvent::receiver();
    unsafe {
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if msg.hwnd.is_null() && msg.message == WM_APP {
                match msg.wParam {
                    1 => {
                        if let Some(u) = crate::update::FOUND.lock().unwrap().as_ref() {
                            updates.set_text(format!("Install update {}", u.version));
                            let _ = tray.set_tooltip(Some(format!("launchpad: update {} available", u.version)));
                        }
                    }
                    2 => std::process::exit(0),
                    _ => {}
                }
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
            while let Ok(ev) = rx.try_recv() {
                if ev.id == *settings.id() {
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = std::process::Command::new(exe).arg("--ui").spawn();
                    }
                } else if ev.id == *log.id() {
                    let _ = std::process::Command::new("notepad.exe").arg(crate::config::log_path()).spawn();
                } else if ev.id == *run_now.id() {
                    if let Ok(cfg) = crate::config::load() {
                        std::thread::spawn(move || crate::launch::run(cfg));
                    }
                } else if ev.id == *updates.id() {
                    let found = crate::update::FOUND.lock().unwrap().take();
                    std::thread::spawn(move || match found {
                        Some(u) => crate::update::install(u, true),
                        None => crate::update::check_now(true),
                    });
                } else if ev.id == *quit.id() {
                    std::process::exit(0);
                }
            }
        }
    }
}

/// Blue circle with a white up-arrow, drawn in code so there's no asset file.
pub fn icon_rgba() -> Vec<u8> {
    const N: usize = 32;
    let mut px = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
            let d = (dx * dx + dy * dy).sqrt();
            let yf = y as f32;
            let arrow = ((6.0..=17.0).contains(&yf) && dx.abs() <= (yf - 6.0) * 0.75)
                || ((17.0..=25.0).contains(&yf) && dx.abs() <= 2.5);
            let c: [u8; 4] = if d > 15.5 {
                [0, 0, 0, 0]
            } else if arrow {
                [255, 255, 255, 255]
            } else {
                // Soft edge on the circle.
                let a = ((15.5 - d).clamp(0.0, 1.0) * 255.0) as u8;
                [37, 99, 235, a]
            };
            px[(y * N + x) * 4..][..4].copy_from_slice(&c);
        }
    }
    px
}

fn icon() -> Icon {
    Icon::from_rgba(icon_rgba(), 32, 32).expect("icon")
}
