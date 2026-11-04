//! Recycle Bin: move to, query, empty and restore by original path (port of Core/RecycleBin.cs).

use std::path::{Path, PathBuf};
use windows_sys::Win32::UI::Shell::{
    SHEmptyRecycleBinW, SHFileOperationW, SHQUERYRBINFO, SHQueryRecycleBinW, SHFILEOPSTRUCTW,
};

const FO_DELETE: u32 = 3;
const FOF_SILENT: u16 = 0x0004;
const FOF_NOCONFIRMATION: u16 = 0x0010;
const FOF_ALLOWUNDO: u16 = 0x0040;
const FOF_NOERRORUI: u16 = 0x0400;
const FOF_WANTNUKEWARNING: u16 = 0x4000;
const SHERB_NOCONFIRMATION: u32 = 1;
const SHERB_NOPROGRESSUI: u32 = 2;
const SHERB_NOSOUND: u32 = 4;

/// Moves a file or folder to the Recycle Bin. True when the item no longer exists.
///
/// C# bug fixed here: paths longer than MAX_PATH failed silently; they are now passed with the
/// `\\?\` prefix stripped (SHFileOperation does not accept it) and, when that fails, reported as not deleted.
pub fn move_to_bin(path: &Path) -> bool {
    let s = path.to_string_lossy();
    let s = s.trim_end_matches('\\');
    let s = s.strip_prefix(r"\\?\").unwrap_or(s);
    if s.is_empty() {
        return false;
    }
    // double zero-terminated list
    let mut from: Vec<u16> = s.encode_utf16().collect();
    from.extend([0, 0]);
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = from.as_ptr();
    op.fFlags = FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING | FOF_SILENT | FOF_NOERRORUI;
    unsafe { SHFileOperationW(&mut op) };
    let p = Path::new(s);
    !p.exists() && p.symlink_metadata().is_err()
}

/// (bytes, items) in all Recycle Bins.
pub fn query() -> (u64, u64) {
    let mut info: SHQUERYRBINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHQUERYRBINFO>() as u32;
    let hr = unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut info) };
    if hr == 0 { (info.i64Size as u64, info.i64NumItems as u64) } else { (0, 0) }
}

pub fn empty() -> bool {
    let hr = unsafe {
        SHEmptyRecycleBinW(std::ptr::null_mut(), std::ptr::null(), SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND)
    };
    hr == 0 || hr == 0x8000_FFFFu32 as i32 // E_UNEXPECTED: already empty
}

/// Current user's SID as a string ("S-1-5-21-…").
pub fn current_sid() -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
        CloseHandle(token);
        if ok == 0 {
            return None;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut s) == 0 {
            return None;
        }
        let mut n = 0;
        while *s.add(n) != 0 {
            n += 1;
        }
        let out = String::from_utf16_lossy(std::slice::from_raw_parts(s, n));
        LocalFree(s.cast());
        Some(out)
    }
}

/// Parses a `$I` metadata file (version 1: Vista+, version 2: Windows 10+). (original path, deletion FILETIME)
pub fn read_info(bytes: &[u8]) -> Option<(String, i64)> {
    if bytes.len() < 24 {
        return None;
    }
    let version = i64::from_le_bytes(bytes[0..8].try_into().ok()?);
    let deleted = i64::from_le_bytes(bytes[16..24].try_into().ok()?);
    let utf16 = |b: &[u8]| {
        let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        super::from_wide(&u)
    };
    let path = if version == 2 && bytes.len() >= 28 {
        let chars = i32::from_le_bytes(bytes[24..28].try_into().ok()?);
        if chars <= 0 || 28 + chars as usize * 2 > bytes.len() {
            return None;
        }
        utf16(&bytes[28..28 + chars as usize * 2])
    } else if version == 1 && bytes.len() >= 24 + 520 {
        utf16(&bytes[24..24 + 520])
    } else {
        return None;
    };
    (!path.is_empty()).then_some((path, deleted))
}

/// Puts an item back from the Recycle Bin by its original path. The newest deletion wins.
pub fn restore(original: &Path) -> bool {
    let full = original.to_string_lossy().trim_end_matches('\\').to_string();
    let full_path = PathBuf::from(&full);
    if full_path.exists() {
        return false; // something else is there now
    }
    let Some(root) = full.get(..3) else { return false };
    let Some(sid) = current_sid() else { return false };
    let bin = PathBuf::from(root).join("$Recycle.Bin").join(sid);
    let Ok(entries) = std::fs::read_dir(&bin) else { return false };

    let mut best: Option<(PathBuf, i64)> = None;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with("$I") {
            continue;
        }
        let Ok(bytes) = std::fs::read(e.path()) else { continue };
        let Some((path, deleted)) = read_info(&bytes) else { continue };
        if !path.trim_end_matches('\\').eq_ignore_ascii_case(&full) && path.trim_end_matches('\\').to_lowercase() != full.to_lowercase() {
            continue;
        }
        if best.as_ref().is_none_or(|b| deleted > b.1) {
            best = Some((e.path(), deleted));
        }
    }
    let Some((info, _)) = best else { return false };
    let info_name = info.file_name().unwrap_or_default().to_string_lossy().to_string();
    let data = bin.join(format!("$R{}", &info_name[2..]));
    if let Some(parent) = full_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if !data.exists() || std::fs::rename(&data, &full_path).is_err() {
        return false;
    }
    let _ = std::fs::remove_file(&info);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v2_info() {
        let path = r"C:\Users\x\file.txt";
        let units: Vec<u16> = path.encode_utf16().chain([0]).collect();
        let mut b = Vec::new();
        b.extend(2i64.to_le_bytes());
        b.extend(123i64.to_le_bytes());
        b.extend(555i64.to_le_bytes());
        b.extend((units.len() as i32).to_le_bytes());
        for u in units {
            b.extend(u.to_le_bytes());
        }
        assert_eq!(read_info(&b), Some((path.to_string(), 555)));
    }

    #[test]
    fn sid_is_known() {
        assert!(current_sid().unwrap().starts_with("S-1-5-"));
    }

    #[test]
    fn recycle_and_restore_roundtrip() {
        let dir = std::env::temp_dir().join(format!("prostor-recycle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("roundtrip.txt");
        std::fs::write(&f, b"hello").unwrap();
        assert!(move_to_bin(&f));
        assert!(!f.exists());
        // %TEMP% may live on a drive without a Recycle Bin; only check restore when the bin kept it
        if restore(&f) {
            assert_eq!(std::fs::read(&f).unwrap(), b"hello");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
