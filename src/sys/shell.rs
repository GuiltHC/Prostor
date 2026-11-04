//! Shell helpers: admin rights, Explorer, URLs, regedit, known folders (port of Core/ShellHelper.cs).

use super::wide;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

static IS_ADMIN: LazyLock<bool> = LazyLock::new(|| unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 });

pub fn is_admin() -> bool {
    *IS_ADMIN
}

fn shell_execute(verb: &str, file: &str, params: &str) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let (v, f, p) = (wide(verb), wide(file), wide(params));
    let h = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            if verb.is_empty() { std::ptr::null() } else { v.as_ptr() },
            f.as_ptr(),
            if params.is_empty() { std::ptr::null() } else { p.as_ptr() },
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    h as isize > 32
}

/// Starts this program elevated with `args`. False when the user declined UAC.
pub fn run_self_as_admin(args: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    shell_execute("runas", &exe.to_string_lossy(), args)
}

/// Opens a file, folder or URL with its default handler.
pub fn open(target: &str) -> bool {
    shell_execute("open", target, "")
}

pub fn open_folder(path: &Path) {
    if path.is_dir() {
        let _ = Command::new("explorer.exe").raw_arg(format!("\"{}\"", path.display())).spawn();
    } else {
        show_in_explorer(path);
    }
}

/// Opens Explorer with the item selected (or its parent when the item is gone).
pub fn show_in_explorer(path: &Path) {
    if path.exists() {
        let _ = Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
    } else if let Some(parent) = path.parent().filter(|p| p.is_dir()) {
        open_folder(parent);
    }
}

pub fn search_web(query: &str) {
    let q: String = query
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    open(&format!("https://www.google.com/search?q={q}"));
}

/// Opens regedit at `full_key` ("HKEY_LOCAL_MACHINE\...").
pub fn open_regedit(full_key: &str) {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    if let Ok((k, _)) = hkcu.create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Applets\Regedit") {
        let _ = k.set_value("LastKey", &full_key.to_string());
    }
    shell_execute("open", "regedit.exe", "-m");
}

pub fn known_folder(id: &windows_sys::core::GUID) -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;
    let mut p: windows_sys::core::PWSTR = std::ptr::null_mut();
    let hr = unsafe { SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut p) };
    if hr != 0 || p.is_null() {
        return None;
    }
    let s = unsafe {
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
        CoTaskMemFree(p.cast());
        s
    };
    (!s.is_empty()).then(|| PathBuf::from(s))
}

pub fn downloads() -> PathBuf {
    known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Downloads).unwrap_or_else(|| home().join("Downloads"))
}

pub fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Users\Default"))
}

pub fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home().join(r"AppData\Local"))
}

pub fn roaming_app_data() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| home().join(r"AppData\Roaming"))
}

pub fn program_data() -> PathBuf {
    std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
}

pub fn windows_dir() -> PathBuf {
    std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

pub fn program_files() -> PathBuf {
    std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
}

pub fn program_files_x86() -> PathBuf {
    std::env::var_os("ProgramFiles(x86)").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"))
}

pub fn temp() -> PathBuf {
    std::env::temp_dir()
}

/// "Windows 11 Pro 25H2".
pub fn windows_version_text() -> String {
    let hklm = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE);
    let Ok(k) = hklm.open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion") else { return "Windows".into() };
    let mut product: String = k.get_value("ProductName").unwrap_or_else(|_| "Windows".into());
    let display: String = k.get_value("DisplayVersion").unwrap_or_default();
    let build: String = k.get_value("CurrentBuildNumber").unwrap_or_default();
    if build.parse::<u32>().unwrap_or(0) >= 22000 {
        product = product.replace("Windows 10", "Windows 11");
    }
    if display.is_empty() { product } else { format!("{product} {display}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_exist() {
        assert!(downloads().is_absolute());
        assert!(windows_dir().is_dir());
        assert!(windows_version_text().starts_with("Windows"));
    }
}
