//! Moves a folder to another drive and leaves a junction in its place, so programs keep finding it
//! (port of FolderMover in FileTools.cs).
//!
//! Fixed compared to the C# version: any folder whose path merely started with "C:\Windows"
//! (C:\Windows.old, C:\WindowsApps-backup) was refused as a system folder.

use crate::sys::cmd::{self, Encoding};
use crate::sys::{drives, shell};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// None when the folder may be moved, otherwise the reason.
pub fn check_can_move(folder: &Path) -> Option<String> {
    use std::os::windows::fs::MetadataExt;
    let s = folder.to_string_lossy();
    let full = s.trim_end_matches('\\');
    if full.len() <= 3 {
        return Some("Нельзя перенести корень диска.".into());
    }
    let Ok(md) = std::fs::symlink_metadata(folder) else { return Some("Папка не найдена.".into()) };
    if !md.is_dir() && md.file_attributes() & 0x10 == 0 {
        return Some("Папка не найдена.".into());
    }
    if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Some("Это уже ссылка на другую папку.".into());
    }
    if crate::sys::path_within(full, &shell::windows_dir().to_string_lossy()) {
        return Some("Системные папки Windows переносить нельзя.".into());
    }
    let home = shell::home();
    let keep = [
        shell::program_files(),
        shell::program_files_x86(),
        shell::program_data(),
        home.clone(),
        shell::roaming_app_data(),
        shell::local_app_data(),
        home.join("AppData"),
        home.join(r"AppData\LocalLow"),
        home.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from(r"C:\Users")),
    ];
    if keep.iter().any(|k| k.to_string_lossy().trim_end_matches('\\').eq_ignore_ascii_case(full)) {
        return Some("Эту папку Windows ожидает на прежнем месте — перенесите вложенную папку.".into());
    }
    let l = full.to_lowercase();
    if l.contains(r"\$recycle.bin") || l.contains("system volume information") {
        return Some("Служебные папки переносить нельзя.".into());
    }
    None
}

/// Other fixed NTFS drives with enough free space.
pub fn target_drives(folder: &Path, size: u64) -> Vec<drives::Drive> {
    let s = folder.to_string_lossy();
    let root = s.get(..3).unwrap_or_default().to_lowercase();
    drives::list()
        .into_iter()
        .filter(|d| !d.removable && d.is_ntfs() && d.root.to_lowercase() != root && d.free > size + (512 << 20))
        .collect()
}

fn copy_tree(src: &Path, dst: &Path, copied: &mut u64, total: u64, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<(), String> {
    use std::io::{Read, Write};
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 20];
    for e in std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Остановлено".into());
        }
        let ft = e.file_type().map_err(|e| e.to_string())?;
        if ft.is_symlink() {
            continue;
        }
        let target = dst.join(e.file_name());
        if ft.is_dir() {
            copy_tree(&e.path(), &target, copied, total, progress, cancel)?;
            continue;
        }
        let md = e.metadata().map_err(|e| e.to_string())?;
        let mut input = std::fs::File::open(e.path()).map_err(|er| format!("{}: {er}", e.path().display()))?;
        let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(&target).map_err(|e| e.to_string())?;
        loop {
            let n = input.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            if cancel.load(Ordering::Relaxed) {
                return Err("Остановлено".into());
            }
            output.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            *copied += n as u64;
            if total > 0 {
                progress((*copied as f32 / total as f32).min(1.0));
            }
        }
        let times = std::fs::FileTimes::new().set_modified(md.modified().unwrap_or(std::time::SystemTime::now()));
        let _ = output.set_times(times);
        drop(output);
        let _ = std::fs::set_permissions(&target, md.permissions());
    }
    Ok(())
}

fn try_delete(folder: &Path) {
    if !folder.exists() {
        return;
    }
    let never = AtomicBool::new(false);
    super::walk_files(folder, &never, |p, md| {
        if md.permissions().readonly() {
            let mut perm = md.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            let _ = std::fs::set_permissions(p, perm);
        }
    });
    let _ = std::fs::remove_dir_all(folder);
}

fn junction(link: &Path, target: &Path) -> Result<(), String> {
    let r = cmd::run_raw(
        cmd::system_tool("cmd.exe"),
        &format!("/c mklink /J \"{}\" \"{}\"", link.display(), target.display()),
        Encoding::Oem,
        Duration::from_secs(30),
    )
    .map_err(|e| e.to_string())?;
    if r.ok() && link.exists() { Ok(()) } else { Err(r.output.trim().to_string()) }
}

/// Moves `source` into `target_parent` and leaves a junction. Returns the new location.
pub fn move_folder(source: &Path, target_parent: &Path, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<PathBuf, String> {
    let src = PathBuf::from(source.to_string_lossy().trim_end_matches('\\'));
    if let Some(reason) = check_can_move(&src) {
        return Err(reason);
    }
    let name = src.file_name().ok_or("Неверный путь")?.to_string_lossy().to_string();
    let mut target = target_parent.join(&name);
    let mut i = 2;
    while target.exists() {
        target = target_parent.join(format!("{name} ({i})"));
        i += 1;
    }
    // a folder some program keeps open cannot be renamed — find out before copying gigabytes
    let probe = PathBuf::from(format!("{}.prostor-check", src.display()));
    if std::fs::rename(&src, &probe).is_err() {
        return Err("Папка сейчас используется какой-то программой. Закройте её и повторите.".into());
    }
    std::fs::rename(&probe, &src).map_err(|e| e.to_string())?;

    let (bytes, files) = super::measure(&src, cancel);
    let mut copied = 0;
    let copy = copy_tree(&src, &target, &mut copied, bytes, progress, cancel).and_then(|_| {
        let (b2, f2) = super::measure(&target, cancel);
        if b2 != bytes || f2 != files { Err("Копия не совпадает с оригиналом — перенос отменён.".into()) } else { Ok(()) }
    });
    if let Err(e) = copy {
        try_delete(&target);
        return Err(e);
    }
    let old = PathBuf::from(format!("{}.prostor-old", src.display()));
    std::fs::rename(&src, &old).map_err(|e| e.to_string())?;
    if let Err(e) = junction(&src, &target) {
        let _ = std::fs::rename(&old, &src);
        try_delete(&target);
        return Err(format!("Не удалось создать ссылку на новое место: {e}"));
    }
    try_delete(&old);
    Ok(target)
}

/// Copies the folder back and removes the junction.
pub fn move_back(original: &Path, current: &Path, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    let link = std::fs::symlink_metadata(original).ok();
    if link.as_ref().is_some_and(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0) {
        return Err("На старом месте уже лежит обычная папка.".into());
    }
    if !current.is_dir() {
        return Err(format!("Перенесённая папка не найдена: {}", current.display()));
    }
    let (bytes, files) = super::measure(current, cancel);
    if link.is_some() {
        std::fs::remove_dir(original).map_err(|e| e.to_string())?; // removes only the junction
    }
    let mut copied = 0;
    let r = copy_tree(current, original, &mut copied, bytes, progress, cancel).and_then(|_| {
        let (b2, f2) = super::measure(original, cancel);
        if b2 != bytes || f2 != files { Err("Копия не совпадает с оригиналом.".into()) } else { Ok(()) }
    });
    if let Err(e) = r {
        try_delete(original);
        let _ = junction(original, current);
        return Err(e);
    }
    try_delete(current);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_system_folders_but_not_lookalikes() {
        assert!(check_can_move(Path::new(r"C:\")).is_some());
        assert!(check_can_move(&shell::windows_dir().join("System32")).is_some());
        assert!(check_can_move(&shell::program_files()).is_some());
        let lookalike = std::env::temp_dir().join(format!("Windows-like-{}", std::process::id()));
        std::fs::create_dir_all(&lookalike).unwrap();
        assert!(check_can_move(&lookalike).is_none());
        std::fs::remove_dir_all(&lookalike).unwrap();
    }

    #[test]
    fn moves_and_moves_back_on_the_same_drive() {
        // the move logic does not care whether the target is another drive
        let base = std::env::temp_dir().join(format!("prostor-move-{}", std::process::id()));
        let src = base.join("data");
        let dst = base.join("target");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a.bin"), vec![7u8; 5000]).unwrap();
        std::fs::write(src.join("sub").join("b.txt"), "привет").unwrap();
        let never = AtomicBool::new(false);
        let new = move_folder(&src, &dst, &|_| {}, &never).unwrap();
        assert_eq!(new, dst.join("data"));
        // the old place is now a junction that still shows the files
        assert_eq!(std::fs::read_to_string(src.join("sub").join("b.txt")).unwrap(), "привет");
        move_back(&src, &new, &|_| {}, &never).unwrap();
        assert!(!new.exists());
        assert_eq!(std::fs::read(src.join("a.bin")).unwrap().len(), 5000);
        std::fs::remove_dir_all(&base).unwrap();
    }
}
