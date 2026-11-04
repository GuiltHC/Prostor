//! settings.json (port of Core/AppSettings.cs). Field names match the C# version;
//! fields this version does not know about are kept as they were.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock, RwLockReadGuard};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub struct WindowBounds {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
}

fn default_theme() -> String {
    "system".into()
}
fn default_true() -> bool {
    true
}
fn default_unused() -> i32 {
    6
}
fn default_off() -> String {
    "off".into()
}
fn default_repo() -> String {
    "GuiltHC/Prostor".into()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct Settings {
    /// "system", "dark" or "light".
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub window: Option<WindowBounds>,
    #[serde(default)]
    pub last_page: Option<String>,
    #[serde(default)]
    pub programs_sort: Option<String>,

    // programs
    #[serde(default = "default_true")]
    pub restore_point_before_uninstall: bool,
    #[serde(default = "default_true")]
    pub search_leftovers: bool,
    #[serde(default = "default_unused")]
    pub unused_months: i32,

    // cleanup
    /// Folders that are never cleaned, deleted or offered as leftovers.
    #[serde(default)]
    pub exclusions: Vec<String>,
    /// Checked state of cleanup items the user changed (item id → checked).
    #[serde(default)]
    pub cleanup_checks: BTreeMap<String, bool>,
    /// "off", "weekly" or "monthly".
    #[serde(default = "default_off")]
    pub auto_clean: String,
    #[serde(default)]
    pub auto_clean_items: Vec<String>,
    #[serde(default)]
    pub last_auto_clean: Option<DateTime<Local>>,
    #[serde(default)]
    pub last_auto_clean_freed: i64,

    // startup
    /// Services switched from automatic to manual by Простор (so they stay in the list).
    #[serde(default)]
    pub manual_services: Vec<String>,

    // updates
    #[serde(default = "default_true")]
    pub check_updates: bool,
    #[serde(default = "default_repo")]
    pub update_repo: String,
    #[serde(default)]
    pub skipped_version: Option<String>,

    /// Everything else (e.g. WPF-only options), written back unchanged.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for Settings {
    fn default() -> Self {
        serde_json::from_str("{}").expect("defaults")
    }
}

impl Settings {
    /// True when `path` is one of the exclusions or lies inside one.
    pub fn is_excluded(&self, path: &str) -> bool {
        self.exclusions.iter().any(|e| !e.trim().is_empty() && crate::sys::path_within(path, e.trim()))
    }

    /// Checked state for a cleanup item: the user's choice, else `default`.
    pub fn cleanup_checked(&self, id: &str, default: bool) -> bool {
        self.cleanup_checks
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(id))
            .map(|(_, v)| *v)
            .unwrap_or(default)
    }
}

static CURRENT: LazyLock<RwLock<Settings>> = LazyLock::new(|| RwLock::new(load()));

fn path() -> std::path::PathBuf {
    super::data_dir().join("settings.json")
}

fn load() -> Settings {
    let mut s: Settings = super::read_json(&path()).unwrap_or_default();
    if s.unused_months <= 0 {
        s.unused_months = 6;
    }
    s
}

pub fn get() -> RwLockReadGuard<'static, Settings> {
    CURRENT.read().unwrap_or_else(|e| e.into_inner())
}

/// Changes the settings and saves them.
pub fn update(f: impl FnOnce(&mut Settings)) {
    let snapshot = {
        let mut s = CURRENT.write().unwrap_or_else(|e| e.into_inner());
        f(&mut s);
        s.clone()
    };
    if let Err(e) = super::write_json(&path(), &snapshot) {
        super::log_error("settings", &e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_csharp_format_and_keeps_unknown_fields() {
        let json = r#"{
          "Theme": "dark", "UseMica": true, "Exclusions": ["D:\\Games"],
          "CleanupChecks": {"browser-cache": false},
          "LastAutoClean": "2026-10-01T03:00:00.1234567+03:00",
          "UnusedMonths": 0, "Window": {"Left": 1, "Top": 2, "Width": 3, "Height": 4, "Maximized": true}
        }"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.theme, "dark");
        assert!(s.is_excluded(r"d:\games\steam"));
        assert!(!s.is_excluded(r"D:\Gamesx"));
        assert!(!s.cleanup_checked("Browser-Cache", true));
        assert!(s.cleanup_checked("other", true));
        assert!(s.last_auto_clean.is_some());
        assert!(s.restore_point_before_uninstall);
        assert_eq!(s.update_repo, "GuiltHC/Prostor");
        let out = serde_json::to_string(&s).unwrap();
        assert!(out.contains("\"UseMica\":true"), "{out}");
        assert!(out.contains("\"Maximized\":true"));
    }
}
