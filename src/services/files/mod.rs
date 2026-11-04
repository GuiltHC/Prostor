//! Working with files and folders: NTFS compression, moving folders to another drive, duplicates,
//! scan snapshots ("what grew").

pub mod compression;
pub mod duplicates;
pub mod mover;
pub mod snapshots;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Every file under `folder` (links are not followed). Stops early when `cancel` is set.
pub fn walk_files(folder: &Path, cancel: &AtomicBool, mut f: impl FnMut(&Path, &std::fs::Metadata)) {
    let mut stack = vec![folder.to_path_buf()];
    while let Some(d) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(e.path());
            } else if let Ok(md) = e.metadata() {
                f(&e.path(), &md);
            }
        }
    }
}

pub fn list_files(folder: &Path, cancel: &AtomicBool) -> Vec<PathBuf> {
    let mut v = Vec::new();
    walk_files(folder, cancel, |p, _| v.push(p.to_path_buf()));
    v
}

/// (bytes, files)
pub fn measure(folder: &Path, cancel: &AtomicBool) -> (u64, u64) {
    let (mut b, mut n) = (0, 0);
    walk_files(folder, cancel, |_, md| {
        b += md.len();
        n += 1;
    });
    (b, n)
}
