//! `--install` / `--uninstall`: per-user install, no admin rights needed.
//! Copies the exe to %LOCALAPPDATA%\Programs\launchpad and adds a value under
//! HKCU\...\Run so it starts at login as the normal user.

use std::path::PathBuf;
use std::process::Command;

use winreg::enums::*;
use winreg::RegKey;

use crate::{config, startup, win};

const NAME: &str = "launchpad";
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

pub fn install() {
    match do_install() {
        Ok(m) => win::msgbox(&m),
        Err(e) => win::msgbox(&format!("Install failed:\n{e}")),
    }
}

pub fn uninstall() {
    if let Ok(k) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN, KEY_SET_VALUE) {
        let _ = k.delete_value(NAME);
    }
    let mut restored = 0;
    let mut need_admin = Vec::new();
    if let Ok(cfg) = config::load() {
        for r in cfg.apps.iter().filter_map(|a| a.startup.as_ref()) {
            match startup::set_enabled(r, true) {
                Ok(()) => restored += 1,
                Err(_) if r.source.is_machine() => need_admin.push((r.clone(), true)),
                Err(_) => {}
            }
        }
    }
    if !need_admin.is_empty() {
        restored += need_admin.len();
        startup::set_elevated(&need_admin);
    }
    win::msgbox(&format!(
        "launchpad won't start at login any more, and {restored} startup entries were turned back on.\n\n\
         Files left in place (delete by hand if you want):\n{}\n{}",
        install_dir().display(),
        config::dir().display()
    ));
}

/// Double-clicked from Downloads or similar: offer to install instead of
/// running from there. Returns true if it handled the start (caller exits).
pub fn offer_if_not_installed() -> bool {
    let Ok(me) = std::env::current_exe() else {
        return false;
    };
    let installed = install_dir().join("launchpad.exe");
    if me == installed || crate::update::is_dev_build() {
        return false;
    }
    let msg = if installed.exists() {
        "launchpad is already installed. Replace it with this copy?"
    } else {
        "Install launchpad?\n\nIt will be installed for your user account and start at login. \
         No admin rights needed."
    };
    if win::confirm(msg) {
        install();
    }
    true
}

pub fn install_dir() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into()))
        .join("Programs")
        .join("launchpad")
}

fn run(cmd: &str, args: &[&str]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = Command::new(cmd)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

fn do_install() -> Result<String, String> {
    let dir = install_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let target = dir.join("launchpad.exe");
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    if me != target {
        // The installed exe may be running (tray, Settings, an MCP client).
        // It can't be overwritten but it can be renamed; the next start
        // deletes the .old copy.
        if target.exists() {
            let old = target.with_file_name("launchpad.old.exe");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(&target, &old);
        }
        std::fs::copy(&me, &target).map_err(|e| format!("copy to {}: {e}", target.display()))?;
    }

    config::migrate_legacy(false);
    if !config::config_path().exists() {
        config::save(&config::Config::default())?;
    }

    let (k, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(RUN)
        .map_err(|e| format!("Run key: {e}"))?;
    k.set_value(NAME, &format!("\"{}\"", target.display()))
        .map_err(|e| format!("Run key: {e}"))?;

    let legacy = remove_legacy_offer();

    // Start the tray now instead of waiting for the next login.
    let _ = Command::new(&target).spawn();

    Ok(format!(
        "Installed and running.\n\nApp: {}\nConfig: {}\n\nRight-click the tray icon (blue arrow) > Settings to pick your apps.{legacy}",
        target.display(),
        config::config_path().display()
    ))
}

/// Older versions installed to Program Files with an elevated logon task.
/// Removing that task needs admin once.
fn remove_legacy_offer() -> &'static str {
    if run("schtasks", &["/Query", "/TN", NAME]).is_err() {
        return "";
    }
    if win::confirm(
        "An older launchpad install (Program Files + an admin logon task) is still registered.\n\n\
         Remove it now? Windows will ask for admin rights once.",
    ) {
        win::relaunch_elevated("--remove-legacy");
        "\n\nThe old install is being removed."
    } else {
        "\n\nThe old install is still there. Run `launchpad.exe --remove-legacy` as admin to remove it."
    }
}

/// `--remove-legacy` (elevated): delete the old logon task and Program Files copy.
pub fn remove_legacy() {
    if !win::is_elevated() {
        win::relaunch_elevated("--remove-legacy");
        return;
    }
    // The old config folder is admin-only, so the copy at install may have
    // failed and left a fresh empty config. Copy it over now if so.
    if config::load().map_or(true, |c| c.apps.is_empty()) {
        config::migrate_legacy(true);
    }
    let _ = run("schtasks", &["/Delete", "/TN", NAME, "/F"]);
    let pf =
        PathBuf::from(std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into()))
            .join("launchpad");
    let _ = std::fs::remove_dir_all(&pf);
}
