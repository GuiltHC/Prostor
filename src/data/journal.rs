//! journal.json — everything the app changed, with what is needed to roll it back
//! (port of Services/ActionJournal.cs, same format).

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_ENTRIES: usize = 500;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum UndoKind {
    #[default]
    None,
    /// data: .reg backup files to import.
    RegImport,
    /// data: original paths of items moved to the Recycle Bin.
    RecycleRestore,
    /// data: "task name|xml backup path".
    TaskRestore,
    /// data: folders compressed with NTFS (WOF) compression.
    Decompress,
    /// data: [original path, new location].
    MoveBack,
    /// Re-enables hibernation.
    Hibernation,
    /// data: [startup item key, delayed task name].
    DelayedStart,
}

impl UndoKind {
    pub fn describe(self) -> Option<&'static str> {
        Some(match self {
            UndoKind::RegImport => "Восстановить запись в реестре",
            UndoKind::RecycleRestore => "Вернуть из корзины",
            UndoKind::TaskRestore => "Восстановить задачу",
            UndoKind::Decompress => "Распаковать",
            UndoKind::MoveBack => "Вернуть на место",
            UndoKind::Hibernation => "Включить гибернацию",
            UndoKind::DelayedStart => "Вернуть обычный запуск",
            UndoKind::None => return None,
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct Entry {
    pub id: String,
    pub time: DateTime<Local>,
    /// uninstall, entry, leftovers, force, cleanup, autoclean, delete, duplicates, compress, move, startup, system.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub bytes: i64,
    #[serde(default)]
    pub undo: UndoKind,
    #[serde(default)]
    pub undo_data: Vec<String>,
    #[serde(default)]
    pub undone: bool,
}

impl Entry {
    pub fn new(kind: &str, title: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            time: Local::now(),
            kind: kind.into(),
            title: title.into(),
            detail: None,
            bytes: 0,
            undo: UndoKind::None,
            undo_data: Vec::new(),
            undone: false,
        }
    }

    pub fn detail(mut self, d: impl Into<String>) -> Self {
        let d = d.into();
        self.detail = (!d.is_empty()).then_some(d);
        self
    }

    pub fn bytes(mut self, b: u64) -> Self {
        self.bytes = b as i64;
        self
    }

    pub fn undo(mut self, kind: UndoKind, data: Vec<String>) -> Self {
        self.undo = kind;
        self.undo_data = data;
        self
    }

    pub fn can_undo(&self) -> bool {
        self.undo != UndoKind::None && !self.undone
    }
}

fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:x}{:08x}{:04x}", nanos, std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed) & 0xffff)
}

static LOCK: Mutex<()> = Mutex::new(());
/// Bumped on every change, so views can tell when to reload.
pub static VERSION: AtomicU64 = AtomicU64::new(0);

fn path() -> std::path::PathBuf {
    super::data_dir().join("journal.json")
}

pub fn load() -> Vec<Entry> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    super::read_json(&path()).unwrap_or_default()
}

fn save(list: &[Entry]) {
    if let Err(e) = super::write_json(&path(), &list) {
        super::log_error("journal", &e);
    }
    VERSION.fetch_add(1, Ordering::Relaxed);
}

pub fn add(entry: Entry) {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut list: Vec<Entry> = super::read_json(&path()).unwrap_or_default();
    list.insert(0, entry);
    list.truncate(MAX_ENTRIES);
    save(&list);
}

pub fn mark_undone(id: &str) {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut list: Vec<Entry> = super::read_json(&path()).unwrap_or_default();
    if let Some(e) = list.iter_mut().find(|e| e.id == id) {
        e.undone = true;
        save(&list);
    }
}

pub fn clear() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    save(&[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_csharp_entries() {
        let json = r#"[{"Id":"abc","Time":"2026-10-30T12:00:00.5+03:00","Kind":"cleanup","Title":"Очистка",
            "Detail":null,"Bytes":1024,"Undo":"RecycleRestore","UndoData":["C:\\x"],"Undone":false}]"#;
        let v: Vec<Entry> = serde_json::from_str(json).unwrap();
        assert_eq!(v[0].undo, UndoKind::RecycleRestore);
        assert!(v[0].can_undo());
        let back = serde_json::to_string(&v).unwrap();
        assert!(back.contains("\"Undo\":\"RecycleRestore\""));
        assert!(back.contains("\"UndoData\""));
    }

    #[test]
    fn ids_are_unique() {
        assert_ne!(new_id(), new_id());
    }
}
