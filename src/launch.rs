//! Runs the startup list: one thread per app, each waiting on its own rules.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{App, Config};
use crate::log::Log;
use crate::win::{self, Token};

static RUNNING: AtomicBool = AtomicBool::new(false);

type Done = Arc<(Mutex<HashMap<String, ()>>, Condvar)>;

pub fn run(cfg: Config) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let log = Arc::new(Log::create());
    let elevated = win::is_elevated();
    log.line(&format!("launchpad started (elevated: {elevated})"));

    let apps: Vec<App> = cfg.apps.into_iter().filter(|a| a.enabled && !a.command.trim().is_empty()).collect();
    let names: Arc<HashSet<String>> = Arc::new(apps.iter().map(|a| a.name.clone()).collect());

    // Non-admin apps run with Explorer's token so they aren't elevated too.
    let token = if elevated && apps.iter().any(|a| !a.admin) {
        let t = win::shell_token(Duration::from_secs(120));
        if t.is_none() {
            log.line("Explorer not found after 120 s; non-admin apps will fail");
        }
        t.map(Arc::new)
    } else {
        None
    };

    let done: Done = Arc::new((Mutex::new(HashMap::new()), Condvar::new()));
    let handles: Vec<_> = apps
        .into_iter()
        .map(|app| {
            let (log, names, token, done) = (log.clone(), names.clone(), token.clone(), done.clone());
            thread::spawn(move || {
                run_one(&app, elevated, token.as_deref(), &names, &done, &log);
                done.0.lock().unwrap().insert(app.name.clone(), ());
                done.1.notify_all();
            })
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
    log.line("all done");
    RUNNING.store(false, Ordering::SeqCst);
}

fn run_one(app: &App, elevated: bool, token: Option<&Token>, names: &HashSet<String>, done: &Done, log: &Log) {
    let name = &app.name;
    if let Some(dep) = app.after.as_ref().filter(|d| !d.is_empty()) {
        if !names.contains(dep) {
            log.line(&format!("{name}: '{dep}' isn't enabled, not waiting for it"));
        } else if !wait_done(done, dep, Duration::from_secs(300)) {
            log.line(&format!("{name}: gave up waiting for '{dep}' after 300 s"));
        }
    }
    if app.delay > 0.0 {
        thread::sleep(Duration::from_secs_f32(app.delay));
    }
    if let Some(pct) = app.cpu_below {
        if !wait_cpu(pct as f32, Duration::from_secs(90)) {
            log.line(&format!("{name}: CPU never dropped below {pct}% in 90 s, starting anyway"));
        }
    }
    if let Some(p) = process_name(app) {
        if win::running_processes().contains(&p) {
            log.line(&format!("{name}: already running ({p}), skipped"));
            return;
        }
    }
    match launch(app, elevated, token) {
        Ok(how) => log.line(&format!("{name}: started {how}")),
        Err(e) => log.line(&format!("{name}: FAILED: {e}")),
    }
}

fn wait_done(done: &Done, dep: &str, max: Duration) -> bool {
    let end = Instant::now() + max;
    let mut g = done.0.lock().unwrap();
    while !g.contains_key(dep) {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        g = done.1.wait_timeout(g, left).unwrap().0;
    }
    true
}

/// Three 1-second samples in a row under `pct`.
fn wait_cpu(pct: f32, max: Duration) -> bool {
    let start = Instant::now();
    let mut calm = 0;
    while start.elapsed() < max {
        if win::cpu_percent_over(Duration::from_secs(1)) < pct {
            calm += 1;
            if calm >= 3 {
                return true;
            }
        } else {
            calm = 0;
        }
    }
    false
}

/// Split a command line into (program, arguments). Handles quoted paths and
/// unquoted paths with spaces that end in .exe.
pub fn split_first(cmd: &str) -> (String, String) {
    let c = cmd.trim();
    if let Some(r) = c.strip_prefix('"') {
        return match r.find('"') {
            Some(i) => (r[..i].to_string(), r[i + 1..].trim().to_string()),
            None => (r.to_string(), String::new()),
        };
    }
    let lower = c.to_ascii_lowercase();
    if let Some(i) = lower.find(".exe") {
        let end = i + 4;
        if end == c.len() || c.as_bytes()[end] == b' ' {
            return (c[..end].to_string(), c[end..].trim().to_string());
        }
    }
    match c.find(' ') {
        Some(i) => (c[..i].to_string(), c[i + 1..].trim().to_string()),
        None => (c.to_string(), String::new()),
    }
}

/// Exe name to check before launching. Squirrel apps (Discord, Slack) start
/// via `Update.exe --processStart Real.exe`, so use the real name there.
pub fn process_name(app: &App) -> Option<String> {
    if let Some(p) = app.process.as_ref().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        return Some(p.to_lowercase());
    }
    let (file, rest) = split_first(&win::expand_env(&app.command));
    let mut it = rest.split_whitespace();
    while let Some(a) = it.next() {
        if a.eq_ignore_ascii_case("--processStart") {
            return it.next().map(|s| s.trim_matches('"').to_lowercase());
        }
    }
    let f = Path::new(&file).file_name()?.to_string_lossy().to_lowercase();
    f.ends_with(".exe").then_some(f)
}

/// File to take an app's icon from. For Squirrel apps that's the real exe in
/// the newest `app-x.y.z` folder, not Update.exe.
pub fn icon_source(command: &str) -> Option<String> {
    let cmd = win::expand_env(command.trim());
    if cmd.to_lowercase().starts_with("shell:") {
        return None;
    }
    let (file, rest) = split_first(&cmd);
    let path = Path::new(&file);
    if path.file_name().is_some_and(|n| n.eq_ignore_ascii_case("update.exe")) {
        let mut it = rest.split_whitespace();
        while let Some(a) = it.next() {
            if a.eq_ignore_ascii_case("--processStart") {
                let target = it.next()?.trim_matches('"').to_string();
                let dir = path.parent()?;
                let mut versions: Vec<_> = std::fs::read_dir(dir)
                    .ok()?
                    .flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with("app-"))
                    .map(|e| e.path().join(&target))
                    .filter(|p| p.exists())
                    .collect();
                versions.sort();
                if let Some(p) = versions.pop() {
                    return Some(p.to_string_lossy().into_owned());
                }
            }
        }
    }
    path.exists().then_some(file)
}

fn is_shell_target(file: &str, cmd: &str) -> bool {
    let f = file.to_lowercase();
    f.ends_with(".lnk") || f.ends_with(".url") || cmd.trim_start().to_lowercase().starts_with("shell:")
}

fn launch(app: &App, elevated: bool, token: Option<&Token>) -> Result<String, String> {
    let cmd = win::expand_env(app.command.trim());
    let (file, rest) = split_first(&cmd);
    let shell = is_shell_target(&file, &cmd);
    let cwd = if shell {
        None
    } else {
        Path::new(&file).parent().filter(|p| p.is_dir()).map(|p| p.to_string_lossy().into_owned())
    };
    let line = if rest.is_empty() { format!("\"{file}\"") } else { format!("\"{file}\" {rest}") };

    match (app.admin, elevated) {
        (true, true) if shell => win::shell_execute("open", &file, &rest).map(|_| "as admin".into()),
        (true, true) => win::spawn_direct(&line, cwd.as_deref()).map(|p| format!("as admin (pid {p})")),
        (true, false) => win::shell_execute("runas", &file, &rest).map(|_| "via UAC prompt".into()),
        (false, true) => {
            let t = token.ok_or("no user token (Explorer not running)")?;
            let line = if shell { format!("{} \"{cmd}\"", win::expand_env(r"%SystemRoot%\explorer.exe")) } else { line };
            win::spawn_as(t, &line, cwd.as_deref()).map(|p| format!("(pid {p})"))
        }
        (false, false) if shell => win::shell_execute("open", &file, &rest).map(|_| String::new()),
        (false, false) => win::spawn_direct(&line, cwd.as_deref()).map(|p| format!("(pid {p})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split() {
        assert_eq!(split_first(r#""C:\a b\x.exe" --y"#), (r"C:\a b\x.exe".into(), "--y".into()));
        assert_eq!(split_first(r"C:\Program Files\x\a.exe --min"), (r"C:\Program Files\x\a.exe".into(), "--min".into()));
        assert_eq!(split_first(r"C:\x\a.exe"), (r"C:\x\a.exe".into(), "".into()));
        assert_eq!(split_first("shell:AppsFolder\\X!App"), ("shell:AppsFolder\\X!App".into(), "".into()));
    }

    #[test]
    fn squirrel_process_name() {
        let a = App {
            command: r#""C:\Users\u\AppData\Local\Discord\Update.exe" --processStart Discord.exe --process-start-args --start-minimized"#.into(),
            ..App::default()
        };
        assert_eq!(process_name(&a).as_deref(), Some("discord.exe"));
    }
}
