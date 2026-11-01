//! Suggested config changes. Anything (the MCP server, other tools) can write
//! suggestions to the user's own folder; only the elevated Settings window
//! can apply them, and only when the user clicks Apply and then Save.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::{App, Config};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Suggestion {
    pub id: String,
    pub created: u64,
    #[serde(default)]
    pub source: String,
    pub summary: String,
    #[serde(default)]
    pub reason: String,
    pub ops: Vec<Op>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    AddApp { app: App },
    UpdateApp { name: String, #[serde(default)] changes: Patch },
    RemoveApp { name: String },
    MoveApp { name: String, position: usize },
    SetOptions {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stay_resident: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        guard: Option<bool>,
    },
}

/// Fields to change on an existing app. Missing = unchanged.
/// `after: ""` means "after login", `cpu_below: 0` turns the CPU wait off,
/// `process: ""` goes back to auto-detect.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Patch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delay: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_below: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admin: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
}

pub fn path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("launchpad").join("suggestions.json")
}

pub fn load() -> Vec<Suggestion> {
    std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(list: &[Suggestion]) -> Result<(), String> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let s = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
    std::fs::write(&p, s).map_err(|e| format!("{}: {e}", p.display()))
}

pub fn remove(id: &str) -> Result<bool, String> {
    let mut list = load();
    let before = list.len();
    list.retain(|s| s.id != id);
    if list.len() == before {
        return Ok(false);
    }
    save(&list).map(|_| true)
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn new_id() -> String {
    let n = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros()).unwrap_or(0);
    format!("s{:x}", n)
}

fn find(cfg: &Config, name: &str) -> Result<usize, String> {
    cfg.apps.iter().position(|a| a.name == name).ok_or_else(|| format!("no app named \"{name}\""))
}

fn rename_refs(cfg: &mut Config, old: &str, new: Option<&str>) {
    for a in &mut cfg.apps {
        if a.after.as_deref() == Some(old) {
            a.after = new.map(str::to_string);
        }
    }
}

/// Apply ops in order. Fails (leaving `cfg` partly changed) on the first bad
/// op, so callers apply to a clone.
pub fn apply(cfg: &mut Config, ops: &[Op]) -> Result<(), String> {
    for (n, op) in ops.iter().enumerate() {
        apply_one(cfg, op).map_err(|e| format!("change {}: {e}", n + 1))?;
    }
    Ok(())
}

fn apply_one(cfg: &mut Config, op: &Op) -> Result<(), String> {
    match op {
        Op::AddApp { app } => {
            let name = app.name.trim();
            if name.is_empty() {
                return Err("new app needs a name".into());
            }
            if cfg.apps.iter().any(|a| a.name == name) {
                return Err(format!("an app named \"{name}\" already exists"));
            }
            if app.command.trim().is_empty() {
                return Err(format!("\"{name}\" needs a command"));
            }
            if let Some(dep) = app.after.as_deref().filter(|d| !d.is_empty()) {
                find(cfg, dep)?;
            }
            let mut a = app.clone();
            a.name = name.to_string();
            a.after = a.after.filter(|d| !d.is_empty());
            a.cpu_below = a.cpu_below.filter(|&p| p > 0).map(|p| p.clamp(5, 95));
            a.delay = a.delay.clamp(0.0, 900.0);
            cfg.apps.push(a);
        }
        Op::UpdateApp { name, changes: p } => {
            let i = find(cfg, name)?;
            if let Some(new) = p.name.as_deref().map(str::trim) {
                if new.is_empty() {
                    return Err("name can't be empty".into());
                }
                if new != name && cfg.apps.iter().any(|a| a.name == new) {
                    return Err(format!("an app named \"{new}\" already exists"));
                }
                cfg.apps[i].name = new.to_string();
                rename_refs(cfg, name, Some(new));
            }
            let self_name = cfg.apps[i].name.clone();
            if let Some(dep) = &p.after {
                if dep.is_empty() {
                    cfg.apps[i].after = None;
                } else if *dep == self_name {
                    return Err("an app can't wait for itself".into());
                } else {
                    find(cfg, dep)?;
                    cfg.apps[i].after = Some(dep.clone());
                }
            }
            let a = &mut cfg.apps[i];
            if let Some(c) = &p.command {
                if c.trim().is_empty() {
                    return Err("command can't be empty".into());
                }
                a.command = c.clone();
            }
            if let Some(v) = p.enabled {
                a.enabled = v;
            }
            if let Some(v) = p.delay {
                a.delay = v.clamp(0.0, 900.0);
            }
            if let Some(v) = p.cpu_below {
                a.cpu_below = (v > 0).then(|| v.clamp(5, 95));
            }
            if let Some(v) = p.admin {
                a.admin = v;
            }
            if let Some(v) = &p.process {
                a.process = (!v.trim().is_empty()).then(|| v.trim().to_string());
            }
        }
        Op::RemoveApp { name } => {
            let i = find(cfg, name)?;
            cfg.apps.remove(i);
            rename_refs(cfg, name, None);
        }
        Op::MoveApp { name, position } => {
            let i = find(cfg, name)?;
            let a = cfg.apps.remove(i);
            let pos = (*position).min(cfg.apps.len());
            cfg.apps.insert(pos, a);
        }
        Op::SetOptions { stay_resident, guard } => {
            if let Some(v) = stay_resident {
                cfg.stay_resident = *v;
            }
            if let Some(v) = guard {
                cfg.guard = *v;
            }
        }
    }
    Ok(())
}

/// Human-readable lines for one op: (text, needs attention).
pub fn describe(cfg: &Config, op: &Op) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    match op {
        Op::AddApp { app } => {
            out.push((format!("Add \"{}\", {}", app.name, timing(app.after.as_deref(), app.delay, app.cpu_below)), false));
            out.push((format!("    runs: {}", app.command), false));
            if app.admin {
                out.push(("    runs as Administrator".into(), true));
            }
            if !app.enabled {
                out.push(("    added switched off".into(), false));
            }
            if let Some(r) = &app.startup {
                out.push((format!("    turns off Windows startup entry \"{}\"", r.name), false));
            }
        }
        Op::UpdateApp { name, changes: p } => {
            let old = cfg.apps.iter().find(|a| &a.name == name);
            out.push((format!("Change \"{name}\""), false));
            if let Some(v) = &p.name {
                out.push((format!("    rename to \"{v}\""), false));
            }
            if let Some(v) = &p.command {
                out.push((format!("    new command: {v}"), old.is_some_and(|a| a.admin) || p.admin == Some(true)));
            }
            if p.after.is_some() || p.delay.is_some() || p.cpu_below.is_some() {
                let after = p.after.as_deref().or(old.and_then(|a| a.after.as_deref()));
                let delay = p.delay.or(old.map(|a| a.delay)).unwrap_or(0.0);
                let cpu = match p.cpu_below {
                    Some(0) => None,
                    Some(v) => Some(v),
                    None => old.and_then(|a| a.cpu_below),
                };
                out.push((format!("    start {}", timing(after, delay, cpu)), false));
            }
            if let Some(v) = p.enabled {
                out.push((format!("    switch {}", if v { "on" } else { "off" }), false));
            }
            if let Some(v) = p.admin {
                out.push((if v { "    run as Administrator".into() } else { "    run as you (not admin)".into() }, v));
            }
            if let Some(v) = &p.process {
                out.push((format!("    skip-if-running process: {}", if v.is_empty() { "auto" } else { v }), false));
            }
        }
        Op::RemoveApp { name } => out.push((format!("Remove \"{name}\""), false)),
        Op::MoveApp { name, position } => out.push((format!("Move \"{name}\" to position {}", position + 1), false)),
        Op::SetOptions { stay_resident, guard } => {
            if let Some(v) = stay_resident {
                out.push((format!("Stay in tray: {}", on_off(*v)), false));
            }
            if let Some(v) = guard {
                out.push((format!("Guard: {}", on_off(*v)), false));
            }
        }
    }
    out
}

fn on_off(v: bool) -> &'static str {
    if v { "on" } else { "off" }
}

fn timing(after: Option<&str>, delay: f32, cpu: Option<u8>) -> String {
    let base = match after.filter(|s| !s.is_empty()) {
        Some(d) if delay > 0.0 => format!("{delay}s after {d}"),
        Some(d) => format!("right after {d}"),
        None if delay > 0.0 => format!("{delay}s after login"),
        None => "at login".into(),
    };
    match cpu.filter(|&c| c > 0) {
        Some(c) => format!("{base}, once CPU < {c}%"),
        None => base,
    }
}

/// Estimated seconds after login for app `i`, following `after` chains.
/// CPU waits aren't included.
pub fn estimate(cfg: &Config, i: usize) -> f32 {
    let mut t = 0.0;
    let mut cur = Some(i);
    let mut seen = std::collections::HashSet::new();
    while let Some(c) = cur {
        if !seen.insert(c) {
            break;
        }
        let a = &cfg.apps[c];
        t += a.delay;
        cur = a.after.as_ref().and_then(|n| cfg.apps.iter().position(|o| &o.name == n && o.enabled));
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        let mut c = Config::default();
        c.apps.push(App { name: "A".into(), command: "a.exe".into(), ..App::default() });
        c.apps.push(App { name: "B".into(), command: "b.exe".into(), after: Some("A".into()), delay: 10.0, ..App::default() });
        c
    }

    #[test]
    fn rename_follows_refs() {
        let mut c = cfg();
        let p = Patch { name: Some("Alpha".into()), ..Patch::default() };
        apply(&mut c, &[Op::UpdateApp { name: "A".into(), changes: p }]).unwrap();
        assert_eq!(c.apps[1].after.as_deref(), Some("Alpha"));
    }

    #[test]
    fn rejects_unknown_and_duplicate() {
        let mut c = cfg();
        assert!(apply(&mut c, &[Op::RemoveApp { name: "Z".into() }]).is_err());
        let dup = App { name: "A".into(), command: "x".into(), ..App::default() };
        assert!(apply(&mut c, &[Op::AddApp { app: dup }]).is_err());
    }

    #[test]
    fn ops_parse_from_json() {
        let ops: Vec<Op> = serde_json::from_str(
            r#"[{"op":"update_app","name":"B","changes":{"delay":30,"cpu_below":25}},
                {"op":"add_app","app":{"name":"C","command":"c.exe","after":"B"}}]"#,
        )
        .unwrap();
        let mut c = cfg();
        apply(&mut c, &ops).unwrap();
        assert_eq!(c.apps[1].delay, 30.0);
        assert_eq!(estimate(&c, 2), 30.0);
    }
}
