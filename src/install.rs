//! `--install` / `--uninstall`: copy to Program Files, lock down the config
//! folder, and register an elevated logon task (no UAC prompt at login).

use std::path::PathBuf;
use std::process::Command;

use crate::{config, startup, win};

const TASK: &str = "launchpad";

pub fn install() {
    if !win::is_elevated() {
        win::relaunch_elevated("--install");
        return;
    }
    match do_install() {
        Ok(m) => win::msgbox(&m),
        Err(e) => win::msgbox(&format!("Install failed:\n{e}")),
    }
}

pub fn uninstall() {
    if !win::is_elevated() {
        win::relaunch_elevated("--uninstall");
        return;
    }
    let _ = run("schtasks", &["/Delete", "/TN", TASK, "/F"]);
    let mut restored = 0;
    if let Ok(cfg) = config::load() {
        for r in cfg.apps.iter().filter_map(|a| a.startup.as_ref()) {
            if startup::set_enabled(r, true).is_ok() {
                restored += 1;
            }
        }
    }
    win::msgbox(&format!(
        "Logon task removed and {restored} startup entries turned back on.\n\n\
         Files left in place (delete by hand if you want):\n{}\n{}",
        install_dir().display(),
        config::dir().display()
    ));
}

fn install_dir() -> PathBuf {
    PathBuf::from(std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into())).join("launchpad")
}

fn run(cmd: &str, args: &[&str]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = Command::new(cmd).args(args).creation_flags(CREATE_NO_WINDOW).output().map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{cmd}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }
}

fn do_install() -> Result<String, String> {
    // 1. Program Files is admin-only, so nothing else can swap the exe the
    //    elevated task runs.
    let dir = install_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let target = dir.join("launchpad.exe");
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    if me != target {
        std::fs::copy(&me, &target)
            .map_err(|e| format!("copy to {}: {e}\n(Quit launchpad from the tray first if it's running.)", target.display()))?;
    }

    // 2. Config folder: admins + SYSTEM write, users read only. Otherwise any
    //    program could add a command for launchpad to run as admin.
    let data = config::dir();
    std::fs::create_dir_all(&data).map_err(|e| format!("{}: {e}", data.display()))?;
    let d = data.to_string_lossy().into_owned();
    run(
        "icacls",
        &[&d, "/inheritance:r", "/grant:r", "*S-1-5-32-544:(OI)(CI)F", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-545:(OI)(CI)RX", "/T", "/Q"],
    )?;
    if !config::config_path().exists() {
        config::save(&config::Config::default())?;
    }

    // 3. Logon task, highest privileges, normal priority (the default 7 would
    //    make every launched app start at below-normal priority).
    let user = format!(
        "{}\\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    );
    let xml = TASK_XML.replace("{user}", &esc(&user)).replace("{exe}", &esc(&target.to_string_lossy()));
    let xml_path = std::env::temp_dir().join("launchpad-task.xml");
    // schtasks wants UTF-16 with a BOM.
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(xml.encode_utf16().flat_map(|c| c.to_le_bytes()));
    std::fs::write(&xml_path, bytes).map_err(|e| e.to_string())?;
    let r = run("schtasks", &["/Create", "/TN", TASK, "/XML", &xml_path.to_string_lossy(), "/F"]);
    let _ = std::fs::remove_file(&xml_path);
    r?;

    Ok(format!(
        "Installed.\n\nApp: {}\nConfig: {}\n\nlaunchpad will start at your next login. \
         Run it now from the Start menu or by running the exe above, then right-click the tray icon > Settings.",
        target.display(),
        config::config_path().display()
    ))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const TASK_XML: &str = r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>launchpad: staggered startup apps</Description></RegistrationInfo>
  <Triggers>
    <LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec><Command>"{exe}"</Command></Exec>
  </Actions>
</Task>
"#;
