#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod install;
mod launch;
mod log;
mod mcp;
mod startup;
mod suggest;
mod tray;
mod ui;
mod win;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--ui") => ui::run(),
        Some("--mcp") => mcp::run(),
        Some("--install") => install::install(),
        Some("--uninstall") => install::uninstall(),
        _ => run_mode(),
    }
}

/// Default mode (what the logon task runs): disable managed startup entries,
/// launch the list, then sit in the tray guarding entries.
fn run_mode() {
    if !win::single_instance("Local\\launchpad-run") {
        return;
    }
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            win::msgbox(&format!("launchpad couldn't read its config, so nothing was started.\n\n{e}"));
            return;
        }
    };
    if cfg.guard {
        startup::apply_guard(&cfg);
    }

    let resident = cfg.stay_resident;
    let guard = cfg.guard;
    let launcher = std::thread::spawn(move || launch::run(cfg));

    if !resident {
        let _ = launcher.join();
        return;
    }
    if guard {
        std::thread::spawn(|| {
            startup::watch(|| {
                // Re-read so changes made in Settings apply without a restart.
                if let Ok(c) = config::load() {
                    if c.guard {
                        startup::apply_guard(&c);
                    }
                }
            })
        });
    }
    tray::run();
}
