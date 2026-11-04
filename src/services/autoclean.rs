//! Scheduled cleanup (port of Services/AutoClean.cs).
//!
//! Fixed compared to the C# version: the unattended run used to take every item the user had ticked,
//! including ones with side effects (hibernation, restore points, old drivers, DISM) — so a scheduled
//! run could silently switch off hibernation. Now it runs only safe items, and honours the
//! `AutoCleanItems` list from the settings when it is not empty (that list was ignored before).

use super::cleanup::{self, Item};
use super::tasks;
use crate::data::{journal, settings};
use crate::fmt;
use std::sync::atomic::AtomicBool;

pub const TASK_PATH: &str = r"\Prostor\AutoClean";

pub fn is_registered() -> bool {
    tasks::exists(TASK_PATH)
}

/// `mode`: "off", "weekly" or "monthly".
pub fn apply(mode: &str) -> Result<(), String> {
    if mode != "weekly" && mode != "monthly" {
        tasks::delete(TASK_PATH);
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let xml = tasks::calendar_task_xml(
        "Простор: автоматическая очистка временных файлов и кэшей. Программа запускается, чистит и сразу закрывается.",
        mode == "monthly",
        &exe.to_string_lossy(),
        "--auto-clean",
    );
    tasks::register(TASK_PATH, &xml)
}

/// The task starts a different copy of the program (it was moved).
pub fn points_elsewhere() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    tasks::export_xml(TASK_PATH).is_some_and(|xml| !xml.to_lowercase().contains(&exe.to_string_lossy().to_lowercase()))
}

/// Items the unattended run cleans.
pub fn selected_items(items: &[Item]) -> Vec<&Item> {
    let s = settings::get();
    items
        .iter()
        .filter(|i| !i.locked() && i.safe_for_auto_clean())
        .filter(|i| {
            if s.auto_clean_items.is_empty() {
                s.cleanup_checked(i.id, i.default_checked)
            } else {
                s.auto_clean_items.iter().any(|x| x.eq_ignore_ascii_case(i.id))
            }
        })
        .collect()
}

/// Headless run started by Task Scheduler. Returns the bytes freed.
pub fn run() -> u64 {
    let items = cleanup::build_items();
    let never = AtomicBool::new(false);
    let (mut freed, mut categories) = (0u64, 0u64);
    for item in selected_items(&items) {
        let r = cleanup::clean(item, &never);
        if r.freed > 0 {
            categories += 1;
        }
        freed += r.freed;
    }
    settings::update(|s| {
        s.last_auto_clean = Some(chrono::Local::now());
        s.last_auto_clean_freed = freed as i64;
    });
    journal::add(
        journal::Entry::new("autoclean", "Автоматическая очистка")
            .detail(format!("{} · по расписанию", fmt::count(categories, "категория", "категории", "категорий")))
            .bytes(freed),
    );
    freed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_picks_items_with_side_effects() {
        let items = cleanup::build_items();
        for i in selected_items(&items) {
            assert!(i.warning.is_none() && !i.slow, "{} must not run unattended", i.id);
            assert_ne!(i.id, "recycle");
        }
    }
}
