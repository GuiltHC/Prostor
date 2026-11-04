//! Thin wrappers over Windows: processes, shell, Recycle Bin, drives, command lines.

pub mod cmd;
pub mod cmdline;
pub mod drives;
pub mod fileinfo;
pub mod icons;
pub mod instance;
pub mod recycle;
pub mod shell;

/// UTF-16, zero-terminated.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 path, zero-terminated.
pub fn wide_path(p: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// Reads a zero-terminated UTF-16 buffer.
pub fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Case-insensitive "is `path` equal to or inside `dir`" for Windows paths.
pub fn path_within(path: &str, dir: &str) -> bool {
    let p = path.trim_end_matches('\\');
    let d = dir.trim_end_matches('\\');
    if d.is_empty() {
        return false;
    }
    if p.len() == d.len() {
        return p.eq_ignore_ascii_case(d) || p.to_lowercase() == d.to_lowercase();
    }
    if p.len() > d.len() && p.as_bytes().get(d.len()) == Some(&b'\\') {
        let head = &p[..d.len()];
        return head.eq_ignore_ascii_case(d) || head.to_lowercase() == d.to_lowercase();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within() {
        assert!(path_within(r"C:\Users\a\b", r"c:\users\A"));
        assert!(path_within(r"C:\Users\a", r"C:\Users\a\"));
        assert!(!path_within(r"C:\Users\ab", r"C:\Users\a"));
        assert!(path_within(r"C:\Пользователи\Тест\x", r"c:\пользователи\тест"));
        assert!(!path_within(r"C:\x", ""));
    }
}
