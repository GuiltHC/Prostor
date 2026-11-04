//! Files in %LocalAppData%\Prostor — the same folder and formats as the C# version,
//! so settings and the action journal carry over.

pub mod journal;
pub mod settings;

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};

pub fn data_dir() -> PathBuf {
    crate::sys::shell::local_app_data().join("Prostor")
}

pub fn backups_dir() -> PathBuf {
    data_dir().join("Backups")
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// Writes through a temporary file so a crash never leaves half a file.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, path)
}

/// Appends to error.log (trimmed at 1 MB).
pub fn log_error(context: &str, err: &dyn std::fmt::Display) {
    use std::io::Write;
    let path = data_dir().join("error.log");
    let _ = std::fs::create_dir_all(data_dir());
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "[{}] {context}: {err}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}
