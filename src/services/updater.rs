//! Checks GitHub releases for a newer version and replaces the running exe — Windows allows renaming
//! a running file (port of Services/Updater.cs).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const ASSET: &str = "Prostor.exe";

#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    pub asset_url: Option<String>,
    pub asset_size: u64,
    pub notes: String,
}

pub fn parse_version(s: &str) -> Option<Vec<u64>> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let v: Option<Vec<u64>> = core.split('.').map(|p| p.parse().ok()).collect();
    let mut v = v?;
    if v.is_empty() {
        return None;
    }
    v.resize(4, 0);
    Some(v)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(30))).http_status_as_error(false).build().into()
}

/// The newer release, or None when this version is current (or there are no releases yet).
pub fn check(repo: &str) -> Result<Option<Release>, String> {
    let repo = repo.trim();
    if !repo.contains('/') {
        return Ok(None);
    }
    let mut resp = agent()
        .get(&format!("https://api.github.com/repos/{repo}/releases/latest"))
        .header("User-Agent", &format!("Prostor/{VERSION}"))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    match resp.status().as_u16() {
        404 => return Ok(None), // no releases published yet
        200 => {}
        s => return Err(format!("GitHub ответил {s}")),
    }
    let v: serde_json::Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    let tag = v.get("tag_name").and_then(|t| t.as_str()).unwrap_or_default().to_string();
    let Some(version) = parse_version(&tag) else { return Ok(None) };
    let asset = v
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|a| a.iter().find(|x| x.get("name").and_then(|n| n.as_str()).is_some_and(|n| n.eq_ignore_ascii_case(ASSET))));
    let release = Release {
        tag,
        asset_url: asset.and_then(|a| a.get("browser_download_url")).and_then(|u| u.as_str()).map(str::to_string),
        asset_size: asset.and_then(|a| a.get("size")).and_then(|s| s.as_u64()).unwrap_or(0),
        notes: v.get("body").and_then(|b| b.as_str()).unwrap_or_default().to_string(),
    };
    Ok((version > parse_version(VERSION).unwrap_or_default()).then_some(release))
}

fn old_path(exe: &Path) -> PathBuf {
    exe.with_file_name(format!("{}.old.exe", exe.file_stem().unwrap_or_default().to_string_lossy()))
}

/// Downloads the new exe next to the running one and swaps them. The caller restarts the app.
pub fn install(release: &Release, progress: impl Fn(f32)) -> Result<PathBuf, String> {
    let url = release.asset_url.as_ref().ok_or("В выпуске нет файла для этой версии программы.")?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let tmp = exe.with_extension("exe.download");
    let resp = agent().get(url).header("User-Agent", &format!("Prostor/{VERSION}")).call().map_err(|e| e.to_string())?;
    if resp.status().as_u16() != 200 {
        return Err(format!("Загрузка не удалась: {}", resp.status()));
    }
    let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(release.asset_size);
    let mut reader = resp.into_body().into_reader();
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if total > 0 {
            progress(done as f32 / total as f32);
        }
    }
    drop(file);
    let old = old_path(&exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&tmp, &exe) {
        let _ = std::fs::rename(&old, &exe); // put the working copy back
        return Err(e.to_string());
    }
    Ok(exe)
}

/// Removes the previous exe left after an update.
pub fn cleanup_after_update() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_path(&exe));
        let _ = std::fs::remove_file(exe.with_extension("exe.download"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), Some(vec![1, 2, 3, 0]));
        assert_eq!(parse_version("2.0"), Some(vec![2, 0, 0, 0]));
        assert_eq!(parse_version("v2.1.0-beta"), Some(vec![2, 1, 0, 0]));
        assert!(parse_version("latest").is_none());
        assert!(parse_version("v1.1.0") < parse_version(VERSION), "the Rust build must count as newer than C# 1.x");
    }
}
