use std::fs::File;
use std::io::Write;
use std::sync::Mutex;
use std::time::Instant;

/// Writes `last-run.log`, one line per event with seconds since start.
pub struct Log {
    start: Instant,
    file: Mutex<Option<File>>,
}

impl Log {
    pub fn create() -> Log {
        let _ = std::fs::create_dir_all(crate::config::dir());
        Log { start: Instant::now(), file: Mutex::new(File::create(crate::config::log_path()).ok()) }
    }

    pub fn line(&self, msg: &str) {
        let s = format!("[+{:6.1}s] {msg}\r\n", self.start.elapsed().as_secs_f32());
        if let Ok(mut f) = self.file.lock() {
            if let Some(f) = f.as_mut() {
                let _ = f.write_all(s.as_bytes());
                let _ = f.flush();
            }
        }
        #[cfg(debug_assertions)]
        eprint!("{s}");
    }
}
