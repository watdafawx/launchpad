use std::mem::zeroed;
use std::ptr::null_mut;

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};
use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, TranslateMessage, MSG};

/// Tray icon + message loop. Blocks until Quit.
pub fn run() {
    let menu = Menu::new();
    let settings = MenuItem::new("Settings…", true, None);
    let log = MenuItem::new("Open last-run log", true, None);
    let run_now = MenuItem::new("Run startup list now", true, None);
    let quit = MenuItem::new("Quit launchpad", true, None);
    let _ = menu.append_items(&[&settings, &log, &run_now, &PredefinedMenuItem::separator(), &quit]);

    let Ok(_tray) = TrayIconBuilder::new()
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
