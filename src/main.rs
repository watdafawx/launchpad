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
mod update;
mod win;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--ui") => ui::run(),
        Some("--mcp") => mcp::run(),
        Some("--install") => install::install(),
        Some("--uninstall") => install::uninstall(),
        Some("--after-update") => run_mode(true),
        _ => {
            if !install::offer_if_not_installed() {
                run_mode(false)
            }
        }
    }
}

/// Default mode (what the logon task runs): disable managed startup entries,
/// launch the list, then sit in the tray guarding entries and checking for
/// updates. After a self-update we only come back to the tray: the startup
/// list already ran at login.
fn run_mode(after_update: bool) {
    // The old copy may still be exiting after an update; give it a moment.
    let wait = if after_update { 15 } else { 0 };
    if !win::single_instance_wait("Local\\launchpad-run", wait) {
        return;
    }
    update::cleanup();
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
    let launcher = (!after_update).then(|| {
        let c = cfg.clone();
        std::thread::spawn(move || launch::run(c))
    });

    if !resident {
        if let Some(l) = launcher {
            let _ = l.join();
        }
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
    if !update::is_dev_build() {
        update::spawn();
    }
    tray::run();
}
