//! NTFS (WOF) compression of folders — the same as `compact /exe:lzx` (port of Compression in FileTools.cs).

use crate::sys::cmd;
use crate::sys::drives;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

/// File types that are already compressed — WOF would gain nothing.
const INCOMPRESSIBLE: &[&str] = &[
    "zip", "7z", "rar", "gz", "xz", "bz2", "zst", "cab", "jpg", "jpeg", "png", "gif", "webp", "avif", "heic", "mp4", "mkv", "avi", "mov",
    "webm", "mp3", "ogg", "opus", "flac", "aac", "m4a", "wma", "bik", "bk2", "usm", "pdf", "docx", "xlsx", "pptx", "jar", "apk", "msi",
    "vpk", "pak", "utoc", "ucas", "iso", "wim", "esd",
];

fn ext(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub fn is_supported(folder: &Path) -> bool {
    let s = folder.to_string_lossy();
    let Some(root) = s.get(..3) else { return false };
    drives::list().iter().any(|d| d.root.eq_ignore_ascii_case(root) && d.is_ntfs())
}

/// Space the folder really takes (compressed files count with their compressed size).
pub fn physical_size(folder: &Path, cancel: &AtomicBool) -> u64 {
    use windows_sys::Win32::Storage::FileSystem::GetCompressedFileSizeW;
    let mut total = 0u64;
    super::walk_files(folder, cancel, |p, md| {
        let w = crate::sys::wide_path(p);
        let mut high = 0u32;
        let low = unsafe { GetCompressedFileSizeW(w.as_ptr(), &mut high) };
        if low == u32::MAX && unsafe { windows_sys::Win32::Foundation::GetLastError() } != 0 {
            total += md.len();
        } else {
            total += (high as u64) << 32 | low as u64;
        }
    });
    total
}

/// Rough share of space compression would save, measured on a sample (0..0.85).
pub fn estimate_saving(folder: &Path, cancel: &AtomicBool) -> f64 {
    let mut files: Vec<(std::path::PathBuf, u64)> = Vec::new();
    super::walk_files(folder, cancel, |p, md| {
        if md.len() > 4096 {
            files.push((p.to_path_buf(), md.len()));
        }
    });
    if files.is_empty() {
        return 0.0;
    }
    let total_bytes: u64 = files.iter().map(|f| f.1).sum();
    // weight the sample by size: the biggest files decide the result
    let mut by_size = files.clone();
    by_size.sort_by_key(|f| std::cmp::Reverse(f.1));
    let step = (files.len() / 60).max(1);
    let mut sample: Vec<(std::path::PathBuf, u64)> = by_size.into_iter().take(60).collect();
    for f in files.iter().step_by(step).take(60) {
        if !sample.iter().any(|s| s.0 == f.0) {
            sample.push(f.clone());
        }
    }
    let (mut sampled, mut saved) = (0f64, 0f64);
    let mut buf = vec![0u8; 256 * 1024];
    for (path, len) in sample {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if INCOMPRESSIBLE.contains(&ext(&path).as_str()) {
            sampled += len as f64;
            continue;
        }
        let Ok(mut f) = std::fs::File::open(&path) else { continue };
        let Ok(n) = f.read(&mut buf) else { continue };
        if n == 0 {
            continue;
        }
        let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        let _ = z.write_all(&buf[..n]);
        let out = z.finish().unwrap_or_default();
        let ratio = (out.len() as f64 / n as f64).min(1.0);
        sampled += len as f64;
        saved += len as f64 * (1.0 - ratio) * 0.95;
    }
    if sampled <= 0.0 {
        return 0.0;
    }
    let saving = saved / sampled;
    (saving * (sampled / (total_bytes as f64).max(1.0) * 4.0 + 0.75).min(1.0)).clamp(0.0, 0.85)
}

fn run_compact(folder: &Path, args: &[String], progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let total = super::list_files(folder, cancel).len().max(1);
    let mut child = Command::new(cmd::system_tool("compact.exe"))
        .args(args)
        .creation_flags(0x0800_0000)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut done = 0usize;
    if let Some(out) = child.stdout.take() {
        // every processed file prints one line; headers end with ':' or are empty
        for line in BufReader::new(out).split(b'\n').map_while(Result::ok) {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                break;
            }
            let l = String::from_utf8_lossy(&line);
            let t = l.trim();
            if t.is_empty() || t.ends_with(':') {
                continue;
            }
            done = (done + 1).min(total);
            progress(done as f32 / total as f32);
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    progress(1.0);
    if cancel.load(Ordering::Relaxed) {
        return Err("Остановлено".into());
    }
    // compact returns non-zero when some files were skipped (in use): not a failure of the whole run
    let _ = status;
    Ok(())
}

pub fn compress(folder: &Path, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<(), String> {
    let f = folder.to_string_lossy().trim_end_matches('\\').to_string();
    run_compact(folder, &["/c".into(), format!("/s:{f}"), "/a".into(), "/i".into(), "/q".into(), "/exe:lzx".into()], progress, cancel)
}

pub fn decompress(folder: &Path, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<(), String> {
    let f = folder.to_string_lossy().trim_end_matches('\\').to_string();
    run_compact(folder, &["/u".into(), format!("/s:{f}"), "/a".into(), "/i".into(), "/q".into(), "/exe".into()], progress, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compresses_and_restores_a_folder() {
        let dir = std::env::temp_dir().join(format!("prostor-wof-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = "Простор ".repeat(200_000); // very compressible
        std::fs::write(dir.join("a.txt"), &text).unwrap();
        let never = AtomicBool::new(false);
        if !is_supported(&dir) {
            return; // %TEMP% on a non-NTFS drive
        }
        let before = physical_size(&dir, &never);
        assert!(estimate_saving(&dir, &never) > 0.3);
        compress(&dir, &|_| {}, &never).unwrap();
        let after = physical_size(&dir, &never);
        assert!(after < before / 2, "{after} vs {before}");
        decompress(&dir, &|_| {}, &never).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), text);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
