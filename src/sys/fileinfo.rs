//! File version resources, shortcuts (.lnk) and indirect resource strings.

use super::{from_wide, wide};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VersionInfo {
    pub company: Option<String>,
    pub description: Option<String>,
    pub product: Option<String>,
    pub version: Option<String>,
}

/// CompanyName / FileDescription / ProductName / FileVersion of an executable.
pub fn version_info(path: &Path) -> VersionInfo {
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    let w = super::wide_path(path);
    unsafe {
        let mut handle = 0u32;
        let size = GetFileVersionInfoSizeW(w.as_ptr(), &mut handle);
        if size == 0 {
            return VersionInfo::default();
        }
        let mut buf = vec![0u8; size as usize];
        if GetFileVersionInfoW(w.as_ptr(), 0, size, buf.as_mut_ptr().cast()) == 0 {
            return VersionInfo::default();
        }
        // language/code page pairs
        let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let trans = wide(r"\VarFileInfo\Translation");
        let mut langs: Vec<(u16, u16)> = Vec::new();
        if VerQueryValueW(buf.as_ptr().cast(), trans.as_ptr(), &mut ptr, &mut len) != 0 && len >= 4 {
            let n = len as usize / 4;
            let pairs = std::slice::from_raw_parts(ptr as *const u16, n * 2);
            for i in 0..n {
                langs.push((pairs[i * 2], pairs[i * 2 + 1]));
            }
        }
        langs.extend([(0x0409, 0x04B0), (0x0409, 0x04E4), (0x0419, 0x04B0), (0x0000, 0x04B0)]);
        let query = |name: &str| -> Option<String> {
            for (lang, cp) in &langs {
                let q = wide(&format!(r"\StringFileInfo\{lang:04x}{cp:04x}\{name}"));
                let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
                let mut l = 0u32;
                if VerQueryValueW(buf.as_ptr().cast(), q.as_ptr(), &mut p, &mut l) != 0 && l > 0 && !p.is_null() {
                    let s = from_wide(std::slice::from_raw_parts(p as *const u16, l as usize));
                    let s = s.trim().to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
            None
        };
        VersionInfo {
            company: query("CompanyName"),
            description: query("FileDescription"),
            product: query("ProductName"),
            version: query("FileVersion"),
        }
    }
}

/// Resolves "@%SystemRoot%\system32\x.dll,-101" style strings.
pub fn indirect_string(s: &str) -> Option<String> {
    if !s.starts_with('@') {
        return Some(s.to_string());
    }
    let w = wide(s);
    let mut buf = [0u16; 512];
    let hr = unsafe {
        windows_sys::Win32::UI::Shell::SHLoadIndirectString(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32, std::ptr::null_mut())
    };
    (hr == 0).then(|| from_wide(&buf)).filter(|s| !s.is_empty())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shortcut {
    pub target: Option<PathBuf>,
    pub arguments: Option<String>,
    pub working_dir: Option<String>,
    pub icon_location: Option<String>,
    /// System.AppUserModel.ID from the shortcut's property store (how the taskbar groups it).
    pub app_id: Option<String>,
}

/// PKEY_AppUserModel_ID = {9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}, 5
const AUMID_FMTID: [u8; 16] = [0x55, 0x28, 0x4C, 0x9F, 0x79, 0x9F, 0x39, 0x4B, 0xA8, 0xD0, 0xE1, 0xD4, 0x2D, 0xE1, 0xD5, 0xF3];

/// Reads System.AppUserModel.ID from the ExtraData blocks that start at `o`.
fn read_app_id(b: &[u8], mut o: usize) -> Option<String> {
    let u32le = |o: usize| -> Option<u32> { Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?)) };
    loop {
        let size = u32le(o)? as usize;
        if size < 8 {
            return None;
        }
        let sig = u32le(o + 4)?;
        if sig == 0xA000_0009 {
            // serialized property stores
            let end = o + size;
            let mut s = o + 8;
            while s + 24 <= end {
                let storage_size = u32le(s)? as usize;
                if storage_size == 0 {
                    break;
                }
                let fmtid = b.get(s + 8..s + 24)?;
                if fmtid == AUMID_FMTID {
                    let mut v = s + 24;
                    while v + 9 <= s + storage_size {
                        let value_size = u32le(v)? as usize;
                        if value_size == 0 {
                            break;
                        }
                        let id = u32le(v + 4)?;
                        let vt = u16::from_le_bytes(b.get(v + 9..v + 11)?.try_into().ok()?);
                        if id == 5 && vt == 0x1F {
                            let chars = u32le(v + 13)? as usize;
                            let bytes = b.get(v + 17..v + 17 + chars * 2)?;
                            let units: Vec<u16> =
                                bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
                            return Some(String::from_utf16_lossy(&units)).filter(|s| !s.is_empty());
                        }
                        v += value_size;
                    }
                }
                s += storage_size;
            }
        }
        o += size;
    }
}

/// Minimal parser of the Shell Link (.lnk) format ([MS-SHLLINK]): the target path from LinkInfo
/// (or the relative path), plus arguments. Much faster than going through COM for every file.
pub fn read_shortcut(path: &Path) -> Option<Shortcut> {
    let b = std::fs::read(path).ok()?;
    parse_shortcut(&b, path.parent())
}

pub fn parse_shortcut(b: &[u8], base_dir: Option<&Path>) -> Option<Shortcut> {
    let u16le = |o: usize| -> Option<u16> { Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?)) };
    let u32le = |o: usize| -> Option<u32> { Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?)) };
    if u32le(0)? != 0x4C {
        return None;
    }
    let flags = u32le(20)?;
    let has_idlist = flags & 0x1 != 0;
    let has_linkinfo = flags & 0x2 != 0;
    let has_name = flags & 0x4 != 0;
    let has_relpath = flags & 0x8 != 0;
    let has_workdir = flags & 0x10 != 0;
    let has_args = flags & 0x20 != 0;
    let has_icon = flags & 0x40 != 0;
    let unicode = flags & 0x80 != 0;
    let mut o = 0x4C;
    if has_idlist {
        o += 2 + u16le(o)? as usize;
    }
    let mut target: Option<PathBuf> = None;
    if has_linkinfo {
        let start = o;
        let size = u32le(start)? as usize;
        let header = u32le(start + 4)? as usize;
        let li_flags = u32le(start + 8)?;
        if li_flags & 0x1 != 0 {
            let read_ansi = |off: usize| -> Option<String> {
                let s = b.get(off..)?;
                let end = s.iter().position(|&c| c == 0)?;
                Some(String::from_utf8_lossy(&s[..end]).into_owned())
            };
            let read_uni = |off: usize| -> Option<String> {
                let s = b.get(off..)?;
                let units: Vec<u16> = s.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
                Some(String::from_utf16_lossy(&units))
            };
            let base = if header >= 0x24 {
                read_uni(start + u32le(start + 0x1C)? as usize)
            } else {
                read_ansi(start + u32le(start + 16)? as usize)
            };
            let suffix = if header >= 0x24 {
                read_uni(start + u32le(start + 0x20)? as usize)
            } else {
                read_ansi(start + u32le(start + 24)? as usize)
            };
            if let Some(base) = base.filter(|s| !s.is_empty()) {
                let mut p = base;
                if let Some(sfx) = suffix.filter(|s| !s.is_empty()) {
                    if !p.ends_with('\\') {
                        p.push('\\');
                    }
                    p.push_str(&sfx);
                }
                target = Some(PathBuf::from(p));
            }
        }
        o = start + size;
    }
    // StringData: NAME, RELATIVE_PATH, WORKING_DIR, COMMAND_LINE_ARGUMENTS, ICON_LOCATION
    let mut read_string = |present: bool| -> Option<Option<String>> {
        if !present {
            return Some(None);
        }
        let count = u16le(o)? as usize;
        o += 2;
        let s = if unicode {
            let bytes = b.get(o..o + count * 2)?;
            o += count * 2;
            String::from_utf16_lossy(&bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
        } else {
            let bytes = b.get(o..o + count)?;
            o += count;
            String::from_utf8_lossy(bytes).into_owned()
        };
        Some(Some(s))
    };
    let _name = read_string(has_name)?;
    let rel = read_string(has_relpath)?;
    let workdir = read_string(has_workdir)?;
    let args = read_string(has_args)?;
    let icon = read_string(has_icon)?;
    let app_id = read_app_id(b, o);
    if target.is_none()
        && let (Some(rel), Some(dir)) = (rel.as_ref(), base_dir) {
            let p = dir.join(rel);
            target = Some(std::fs::canonicalize(&p).map(|c| PathBuf::from(c.to_string_lossy().trim_start_matches(r"\\?\"))).unwrap_or(p));
        }
    let target = target.map(|t| PathBuf::from(super::cmdline::expand_env(&t.to_string_lossy())));
    Some(Shortcut {
        target,
        arguments: args.filter(|s| !s.trim().is_empty()),
        working_dir: workdir.filter(|s| !s.is_empty()),
        icon_location: icon.filter(|s| !s.is_empty()),
        app_id,
    })
}

/// Startup folder of the current user / of all users.
pub fn startup_folder(all_users: bool) -> Option<PathBuf> {
    let id = if all_users {
        &windows_sys::Win32::UI::Shell::FOLDERID_CommonStartup
    } else {
        &windows_sys::Win32::UI::Shell::FOLDERID_Startup
    };
    super::shell::known_folder(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_version_of_notepad() {
        let v = version_info(&super::super::cmd::system_tool("notepad.exe"));
        assert!(v.company.unwrap_or_default().contains("Microsoft"));
    }

    #[test]
    fn resolves_indirect_strings() {
        assert_eq!(indirect_string("plain").as_deref(), Some("plain"));
        // a well-known service description string
        let s = indirect_string(r"@%SystemRoot%\system32\wuaueng.dll,-105");
        assert!(s.is_some_and(|s| !s.is_empty()));
    }

    #[test]
    fn parses_start_menu_shortcuts() {
        // every shortcut in the Start Menu that points to a file must resolve to an absolute path
        let dirs = [
            crate::sys::shell::program_data().join(r"Microsoft\Windows\Start Menu\Programs"),
            crate::sys::shell::roaming_app_data().join(r"Microsoft\Windows\Start Menu\Programs"),
        ];
        let mut parsed = 0;
        let mut with_app_id = 0;
        let mut stack: Vec<PathBuf> = dirs.to_vec();
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
                    let s = read_shortcut(&p).expect("valid shell link");
                    if s.app_id.is_some() {
                        with_app_id += 1;
                    }
                    if let Some(t) = s.target {
                        assert!(t.is_absolute(), "{} -> {}", p.display(), t.display());
                        parsed += 1;
                    }
                }
            }
        }
        assert!(parsed > 0);
        assert!(with_app_id > 0, "some Start Menu shortcuts carry an AppUserModelID");
    }
}
