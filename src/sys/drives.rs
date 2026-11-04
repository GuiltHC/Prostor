//! Logical drives with their labels and free space.

use super::{from_wide, wide};
use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;

#[derive(Clone, Debug)]
pub struct Drive {
    /// "C:\"
    pub root: String,
    pub label: String,
    pub file_system: String,
    pub total: u64,
    pub free: u64,
    pub removable: bool,
}

impl Drive {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }

    pub fn used_fraction(&self) -> f32 {
        if self.total == 0 { 0.0 } else { self.used() as f32 / self.total as f32 }
    }

    pub fn letter(&self) -> &str {
        self.root.trim_end_matches('\\')
    }

    pub fn title(&self) -> String {
        let letter = self.letter();
        if self.label.is_empty() {
            format!("{} ({letter})", if self.removable { "Съёмный диск" } else { "Локальный диск" })
        } else {
            format!("{} ({letter})", self.label)
        }
    }

    pub fn is_ntfs(&self) -> bool {
        self.file_system.eq_ignore_ascii_case("NTFS")
    }
}

pub struct Space {
    pub total: u64,
    pub free: u64,
}

pub fn space(root: &str) -> Option<Space> {
    let w = wide(root);
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, &mut total, &mut free) };
    (ok != 0).then_some(Space { total, free })
}

/// Fixed and removable drives that report a size.
pub fn list() -> Vec<Drive> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let root = format!("{}:\\", (b'A' + i as u8) as char);
        let w = wide(&root);
        let kind = unsafe { GetDriveTypeW(w.as_ptr()) };
        if kind != DRIVE_FIXED && kind != DRIVE_REMOVABLE {
            continue;
        }
        let Some(sp) = space(&root) else { continue };
        if sp.total == 0 {
            continue;
        }
        let mut name = [0u16; 261];
        let mut fs = [0u16; 64];
        let ok = unsafe {
            GetVolumeInformationW(
                w.as_ptr(),
                name.as_mut_ptr(),
                name.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        };
        let (label, file_system) = if ok != 0 { (from_wide(&name), from_wide(&fs)) } else { (String::new(), String::new()) };
        out.push(Drive { root, label, file_system, total: sp.total, free: sp.free, removable: kind == DRIVE_REMOVABLE });
    }
    out
}

/// The drive the system is installed on ("C:\").
pub fn system_root() -> String {
    let sys = super::cmd::system_tool("");
    let s = sys.to_string_lossy();
    if s.len() >= 3 { s[..3].to_string() } else { r"C:\".into() }
}
