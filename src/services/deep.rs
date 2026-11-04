//! System-level cleanup through Windows' own tools: DISM, powercfg, shadow copies, pnputil
//! (port of Services/DeepCleanup.cs).
//!
//! Fixed compared to the C# version: deleting old drivers no longer crashes when measuring them failed
//! first (the folder map was left null), and restore-point queries go through CIM in PowerShell,
//! which also works where the WMI .NET wrapper is missing.

use super::cleanup::{CleanResult, Measure, Size, folder_size};
use crate::sys::cmd::{self, Encoding};
use crate::sys::{drives, shell};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

fn system_root() -> String {
    drives::system_root()
}

pub fn hiberfil_size() -> u64 {
    std::fs::metadata(PathBuf::from(system_root()).join("hiberfil.sys")).map(|m| m.len()).unwrap_or(0)
}

fn free_on_system() -> u64 {
    drives::space(&system_root()).map(|s| s.free).unwrap_or(0)
}

pub fn clean_winsxs(cancel: &AtomicBool) -> CleanResult {
    let before = free_on_system();
    let mut c = std::process::Command::new(cmd::system_tool("Dism.exe"));
    c.args(["/Online", "/Cleanup-Image", "/StartComponentCleanup"]);
    let r = cmd::run_command(c, Encoding::Oem, Duration::from_secs(2 * 3600), Some(cancel));
    let ok = r.as_ref().is_ok_and(|r| r.ok());
    let freed = free_on_system().saturating_sub(before);
    CleanResult { freed, deleted: ok as u64, failed: (!ok) as u64 }
}

pub fn disable_hibernation() -> CleanResult {
    let before = hiberfil_size();
    let r = cmd::run(cmd::system_tool("powercfg.exe"), &["/hibernate", "off"], Encoding::Oem, Duration::from_secs(60));
    let after = hiberfil_size();
    let ok = r.is_ok_and(|r| r.ok());
    CleanResult {
        freed: if ok { before.saturating_sub(after) } else { 0 },
        deleted: (after == 0) as u64,
        failed: (after != 0) as u64,
    }
}

pub fn enable_hibernation() -> Result<(), String> {
    match cmd::run(cmd::system_tool("powercfg.exe"), &["/hibernate", "on"], Encoding::Oem, Duration::from_secs(60)) {
        Ok(r) if r.ok() => Ok(()),
        _ => Err("Не удалось включить гибернацию: нужны права администратора.".into()),
    }
}

// ------------------------------------------------------------------ restore points (shadow copies)

fn shadows_script(action: &str) -> String {
    let letter = system_root().trim_end_matches('\\').to_string();
    format!(
        "$v=(Get-CimInstance Win32_Volume -Filter \"DriveLetter='{letter}'\").DeviceID; \
         $s=@(Get-CimInstance Win32_ShadowCopy | Where-Object {{ $_.VolumeName -eq $v }} | Sort-Object InstallDate); \
         $g=([regex]'\\{{[0-9a-fA-F-]+\\}}').Match($v).Value; \
         $used=0; if($g){{ Get-CimInstance Win32_ShadowStorage | ForEach-Object {{ if(\"$($_.Volume)\" -like \"*$g*\"){{ $used+=[int64]$_.UsedSpace }} }} }}; \
         {action}"
    )
}

/// (shadow count, storage used)
fn query_shadows() -> Option<(u64, u64)> {
    let r = cmd::powershell(&shadows_script("Write-Output \"$($s.Count) $used\""), Duration::from_secs(60)).ok()?;
    let line = r.output.lines().rev().find(|l| !l.trim().is_empty())?;
    let mut it = line.split_whitespace();
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

pub fn measure_shadows() -> Measure {
    if !shell::is_admin() {
        return Measure { size: Size::Bytes(0), count: 0 };
    }
    match query_shadows() {
        Some((count, used)) if count > 1 => Measure { size: Size::Bytes(used * (count - 1) / count), count: count - 1 },
        _ => Measure { size: Size::Bytes(0), count: 0 },
    }
}

pub fn delete_old_shadows() -> CleanResult {
    let before = query_shadows().map(|q| q.1).unwrap_or(0);
    let script = shadows_script(
        "$ok=0; $bad=0; if($s.Count -gt 1){ foreach($x in $s[0..($s.Count-2)]){ try { Remove-CimInstance -InputObject $x -ErrorAction Stop; $ok++ } catch { $bad++ } } }; Write-Output \"$ok $bad\"",
    );
    let (ok, bad) = cmd::powershell(&script, Duration::from_secs(600))
        .ok()
        .and_then(|r| {
            let line = r.output.lines().rev().find(|l| !l.trim().is_empty())?.to_string();
            let mut it = line.split_whitespace();
            Some((it.next()?.parse::<u64>().ok()?, it.next()?.parse::<u64>().ok()?))
        })
        .unwrap_or((0, 1));
    let after = query_shadows().map(|q| q.1).unwrap_or(before);
    CleanResult { freed: before.saturating_sub(after), deleted: ok, failed: bad }
}

// ------------------------------------------------------------------ driver store

#[derive(Clone, Debug, PartialEq)]
pub struct DriverPackage {
    /// oemNN.inf
    pub published: String,
    /// original .inf name
    pub original: String,
    /// (year, month, day)
    pub date: (u32, u32, u32),
    pub version: Vec<u32>,
}

/// Finds "MM/DD/YYYY a.b.c.d" in a value.
fn version_line(v: &str) -> Option<((u32, u32, u32), Vec<u32>)> {
    for (i, _) in v.char_indices() {
        let rest = &v[i..];
        if rest.len() < 10 {
            break;
        }
        let b = &rest.as_bytes()[..10];
        if !b.iter().enumerate().all(|(k, c)| if k == 2 || k == 5 { *c == b'/' } else { c.is_ascii_digit() }) {
            continue;
        }
        let d = &rest[..10];
        let m: u32 = d[0..2].parse().ok()?;
        let day: u32 = d[3..5].parse().ok()?;
        let y: u32 = d[6..10].parse().ok()?;
        let tail = rest[10..].trim_start();
        let ver_str: String = tail.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let parts: Vec<u32> = ver_str.split('.').filter_map(|p| p.parse().ok()).collect();
        if parts.len() >= 2 {
            return Some(((y, m, day), parts));
        }
    }
    None
}

/// Parses `pnputil /enum-drivers` (labels are localized, so values are recognized by their shape).
pub fn parse_pnputil(output: &str) -> Vec<DriverPackage> {
    let text = output.replace('\r', "");
    let mut list = Vec::new();
    for block in text.split("\n\n").flat_map(|b| b.split("\n \n")) {
        let values: Vec<String> = block
            .lines()
            .filter_map(|l| l.find(':').filter(|&i| i > 0).map(|i| l[i + 1..].trim().to_string()))
            .filter(|v| !v.is_empty())
            .collect();
        let is_oem = |v: &str| {
            let l = v.to_ascii_lowercase();
            l.starts_with("oem") && l.ends_with(".inf") && l[3..l.len() - 4].chars().all(|c| c.is_ascii_digit()) && l.len() > 7
        };
        let Some(published) = values.iter().find(|v| is_oem(v)).cloned() else { continue };
        let Some(original) = values
            .iter()
            .find(|v| v.to_ascii_lowercase().ends_with(".inf") && !v.eq_ignore_ascii_case(&published))
            .cloned()
        else {
            continue;
        };
        let Some((date, version)) = values.iter().find_map(|v| version_line(v)) else { continue };
        list.push(DriverPackage { published, original, date, version });
    }
    list
}

/// All but the newest package of every driver.
pub fn outdated(packages: &[DriverPackage]) -> Vec<DriverPackage> {
    let mut groups: std::collections::BTreeMap<String, Vec<&DriverPackage>> = Default::default();
    for p in packages {
        groups.entry(p.original.to_lowercase()).or_default().push(p);
    }
    let mut out = Vec::new();
    for (_, mut g) in groups {
        if g.len() < 2 {
            continue;
        }
        g.sort_by(|a, b| b.version.cmp(&a.version).then(b.date.cmp(&a.date)));
        out.extend(g.into_iter().skip(1).cloned());
    }
    out
}

/// oemNN.inf → its folder in DriverStore\FileRepository (same .inf content).
fn repository_folders(packages: &[DriverPackage]) -> Vec<(String, Option<PathBuf>)> {
    let repo = cmd::system_tool(r"DriverStore\FileRepository");
    let inf_dir = shell::windows_dir().join("INF");
    packages
        .iter()
        .map(|p| {
            let found = (|| {
                let oem = std::fs::read(inf_dir.join(&p.published)).ok()?;
                let stem = p.original.rsplit_once('.').map(|(s, _)| s).unwrap_or(&p.original).to_lowercase();
                let prefix = format!("{stem}.inf_");
                for e in std::fs::read_dir(&repo).ok()?.flatten() {
                    if !e.file_name().to_string_lossy().to_lowercase().starts_with(&prefix) {
                        continue;
                    }
                    let inf = e.path().join(&p.original);
                    if std::fs::read(&inf).ok().is_some_and(|b| b == oem) {
                        return Some(e.path());
                    }
                }
                None
            })();
            (p.published.clone(), found)
        })
        .collect()
}

/// Published name of an outdated driver package and its folder in the driver store.
type Outdated = Vec<(String, Option<PathBuf>)>;
static OUTDATED: Mutex<Option<Outdated>> = Mutex::new(None);

fn scan_old_drivers() -> Vec<(String, Option<PathBuf>)> {
    let Ok(r) = cmd::run(cmd::system_tool("pnputil.exe"), &["/enum-drivers"], Encoding::Oem, Duration::from_secs(60)) else {
        return Vec::new();
    };
    repository_folders(&outdated(&parse_pnputil(&r.output)))
}

pub fn measure_old_drivers() -> Measure {
    if !shell::is_admin() {
        return Measure { size: Size::Bytes(0), count: 0 };
    }
    let list = scan_old_drivers();
    let size = list.iter().filter_map(|(_, f)| f.as_ref()).map(|f| folder_size(f)).sum();
    let count = list.len() as u64;
    *OUTDATED.lock().unwrap_or_else(|e| e.into_inner()) = Some(list);
    Measure { size: Size::Bytes(size), count }
}

pub fn delete_old_drivers() -> CleanResult {
    let list = OUTDATED.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_else(scan_old_drivers);
    let mut r = CleanResult::default();
    for (published, folder) in list {
        let size = folder.as_ref().map(|f| folder_size(f)).unwrap_or(0);
        // without /force pnputil refuses to remove a package a device uses
        let res = cmd::run(cmd::system_tool("pnputil.exe"), &["/delete-driver", &published], Encoding::Oem, Duration::from_secs(120));
        if res.is_ok_and(|x| x.ok()) {
            r.deleted += 1;
            r.freed += size;
        } else {
            r.failed += 1;
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Microsoft PnP Utility\r\n\r\n\
Опубликованное имя:     oem12.inf\r\n\
Исходное имя:          nvlddmkm.inf\r\n\
Имя поставщика:        NVIDIA\r\n\
Имя класса:            Видеоадаптеры\r\n\
Версия драйвера:       03/14/2025 32.0.15.7247\r\n\
Имя подписавшего:      Microsoft Windows Hardware Compatibility Publisher\r\n\r\n\
Published Name:     oem40.inf\r\n\
Original Name:      nvlddmkm.inf\r\n\
Driver Version:     10/01/2025 32.0.15.8129\r\n\r\n\
Published Name:     oem3.inf\r\n\
Original Name:      realtek.inf\r\n\
Driver Version:     01/01/2024 1.0.0.1\r\n";

    #[test]
    fn parses_localized_pnputil() {
        let p = parse_pnputil(SAMPLE);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].published, "oem12.inf");
        assert_eq!(p[0].original, "nvlddmkm.inf");
        assert_eq!(p[0].date, (2025, 3, 14));
        assert_eq!(p[0].version, vec![32, 0, 15, 7247]);
        let old = outdated(&p);
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].published, "oem12.inf");
    }

    #[test]
    fn ignores_non_ascii_values() {
        assert_eq!(version_line("хЄ фрээ√ї"), None);
        assert_eq!(version_line("Версия: 01/02/2024 5.6"), Some(((2024, 1, 2), vec![5, 6])));
        assert!(parse_pnputil("Имя: значение\n\nещё: что-то").is_empty());
    }
}
