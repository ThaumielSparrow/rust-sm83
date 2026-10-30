//! Warning/error log. Release builds on Windows have no console, so messages
//! also go to `rust-gbe.log` next to config.json (the previous run's log is
//! kept as `rust-gbe.old.log`).

use std::fs::File;
use std::io::Write;
use std::sync::Mutex;

static FILE: Mutex<Option<File>> = Mutex::new(None);

/// Open the log file for this run. Logging still works (stderr only) if this
/// is never called or the file can't be created.
pub fn init() {
    let path = crate::config::config_path().with_file_name("rust-gbe.log");
    let _ = std::fs::rename(&path, path.with_file_name("rust-gbe.old.log"));
    if let (Ok(file), Ok(mut slot)) = (File::create(&path), FILE.lock()) {
        *slot = Some(file);
    }
}

/// Record a warning or error.
pub fn warn(message: impl std::fmt::Display) {
    eprintln!("{message}");
    if let Ok(mut slot) = FILE.lock()
        && let Some(file) = slot.as_mut()
    {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(file, "[unix {secs}] {message}");
    }
}
