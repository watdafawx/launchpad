use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Keep running in the tray after launching (needed for the guard).
    pub stay_resident: bool,
    /// Keep managed startup entries disabled, even if an app re-enables itself.
    pub guard: bool,
    /// Install new GitHub releases automatically.
    pub auto_update: bool,
    #[serde(rename = "app")]
    pub apps: Vec<App>,
}

impl Default for Config {
    fn default() -> Self {
        Config { stay_resident: true, guard: true, auto_update: true, apps: Vec::new() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct App {
    pub name: String,
    /// Full command line, a .lnk path, or a shell: URI. %VARS% are expanded.
    pub command: String,
    pub enabled: bool,
    /// Seconds to wait (after `after` is done, or after launchpad starts).
    pub delay: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// Wait until total CPU use stays below this percent (max 90 s).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_below: Option<u8>,
    /// Start elevated. Everything else starts as the normal user.
    pub admin: bool,
    /// Process name used to skip the app if it's already running. Auto-detected if empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
    /// The Windows startup entry this replaces; launchpad keeps it disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup: Option<StartupRef>,
}

impl Default for App {
    fn default() -> Self {
        App {
            name: String::new(),
            command: String::new(),
            enabled: true,
            delay: 0.0,
            after: None,
            cpu_below: None,
            admin: false,
            process: None,
            startup: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub struct StartupRef {
    pub source: Source,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    HkcuRun,
    HklmRun,
    HklmRun32,
    UserFolder,
    CommonFolder,
}

impl Source {
    /// Machine-wide entries: changing them needs admin.
    pub fn is_machine(self) -> bool {
        matches!(self, Source::HklmRun | Source::HklmRun32 | Source::CommonFolder)
    }

    pub fn label(self) -> &'static str {
        match self {
            Source::HkcuRun => "Run (user)",
            Source::HklmRun => "Run (all users)",
            Source::HklmRun32 => "Run (all users, 32-bit)",
            Source::UserFolder => "Startup folder (user)",
            Source::CommonFolder => "Startup folder (all users)",
        }
    }
}

/// %LOCALAPPDATA%\launchpad. Per-user: launchpad runs as the normal user, and
/// apps marked admin still need the user to accept a UAC prompt each time.
pub fn dir() -> PathBuf {
    #[cfg(debug_assertions)]
    if let Ok(d) = std::env::var("LAUNCHPAD_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("launchpad")
}

/// Copy config.toml from the old %ProgramData% location if there's none yet.
pub fn migrate_legacy() {
    let old = PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into()))
        .join("launchpad")
        .join("config.toml");
    if !config_path().exists() && old.exists() {
        let _ = std::fs::create_dir_all(dir());
        let _ = std::fs::copy(old, config_path());
    }
}

pub fn config_path() -> PathBuf {
    dir().join("config.toml")
}

pub fn log_path() -> PathBuf {
    dir().join("last-run.log")
}

pub fn load() -> Result<Config, String> {
    match std::fs::read_to_string(config_path()) {
        Ok(s) => toml::from_str(&s).map_err(|e| format!("config.toml: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(format!("config.toml: {e}")),
    }
}

pub fn save(cfg: &Config) -> Result<(), String> {
    let s = toml::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    std::fs::write(config_path(), s).map_err(|e| format!("can't write config: {e}"))
}
