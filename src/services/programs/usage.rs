//! When each program was last started: UserAssist, Prefetch and Steam (port of Services/UsageTracker.cs).

use super::games;
use super::inventory::App;
use crate::sys::{fileinfo, shell};
use chrono::{DateTime, Local};
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

const USER_ASSIST: [&str; 2] = [
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist\{CEBFF5CD-ACE2-4F4F-9178-9926F41749EA}\Count",
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist\{F4E57C4B-2036-45F0-A9AB-443BCFE33D9F}\Count",
];

static HELPER_EXE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)unins|uninst|setup|install|update|updater|crash|report|helper|elevat|vc_?redist|dotnet").unwrap());

#[derive(Default, Debug)]
pub struct UsageData {
    /// Full exe path (lower-case) → last launch (Start menu, taskbar, Explorer).
    pub by_path: HashMap<String, DateTime<Local>>,
    /// Package family name (lower-case) → last launch.
    pub by_package: HashMap<String, DateTime<Local>>,
    /// EXE name as in Prefetch (upper-case) → last launch. Any launch; needs admin rights.
    pub by_exe_name: HashMap<String, DateTime<Local>>,
    /// App user model IDs that could not be tied to a file.
    pub unresolved: Vec<(String, DateTime<Local>)>,
    /// Steam app id → last played.
    pub steam: HashMap<String, DateTime<Local>>,
}

impl UsageData {
    pub fn has_prefetch(&self) -> bool {
        !self.by_exe_name.is_empty()
    }
}

fn put(map: &mut HashMap<String, DateTime<Local>>, key: String, t: DateTime<Local>) {
    if map.get(&key).is_none_or(|old| *old < t) {
        map.insert(key, t);
    }
}

pub fn rot13(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' => (((c as u8 - b'a' + 13) % 26) + b'a') as char,
            'A'..='Z' => (((c as u8 - b'A' + 13) % 26) + b'A') as char,
            _ => c,
        })
        .collect()
}

fn parse_guid(s: &str) -> Option<windows_sys::core::GUID> {
    let h = |a: usize, b: usize| u64::from_str_radix(s.get(a..b)?, 16).ok();
    if s.len() != 36 {
        return None;
    }
    let d4: Option<Vec<u8>> = [(19, 21), (21, 23), (24, 26), (26, 28), (28, 30), (30, 32), (32, 34), (34, 36)]
        .iter()
        .map(|&(a, b)| h(a, b).map(|v| v as u8))
        .collect();
    Some(windows_sys::core::GUID {
        data1: h(0, 8)? as u32,
        data2: h(9, 13)? as u16,
        data3: h(14, 18)? as u16,
        data4: d4?.try_into().ok()?,
    })
}

static KNOWN: LazyLock<Mutex<HashMap<String, Option<String>>>> = LazyLock::new(Default::default);

/// "{6D809377-…}\App\app.exe" → "C:\Program Files\App\app.exe"; plain paths pass through.
fn expand_known_folder(name: &str) -> Option<String> {
    let b = name.as_bytes();
    if b.len() > 3 && b[1] == b':' && b[2] == b'\\' {
        return Some(name.to_string());
    }
    if b.len() > 39 && b[0] == b'{' && b[37] == b'}' {
        let key = name[1..37].to_string();
        let folder = {
            let mut cache = KNOWN.lock().ok()?;
            cache
                .entry(key.clone())
                .or_insert_with(|| parse_guid(&key).and_then(|g| shell::known_folder(&g)).map(|p| p.to_string_lossy().to_string()))
                .clone()
        }?;
        return Some(format!("{folder}{}", &name[38..]));
    }
    None
}

/// Start menu, taskbar and desktop shortcuts: target exe and AppUserModelID.
fn shortcut_index() -> HashMap<String, (Option<String>, Option<String>)> {
    let mut out = HashMap::new();
    let home = shell::home();
    let roots = [
        shell::program_data().join(r"Microsoft\Windows\Start Menu\Programs"),
        shell::roaming_app_data().join(r"Microsoft\Windows\Start Menu\Programs"),
        shell::roaming_app_data().join(r"Microsoft\Internet Explorer\Quick Launch"),
        home.join("Desktop"),
        PathBuf::from(std::env::var("PUBLIC").unwrap_or_else(|_| r"C:\Users\Public".into())).join("Desktop"),
    ];
    for root in roots {
        let mut stack = vec![root];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk"))
                    && let Some(s) = fileinfo::read_shortcut(&p) {
                        let target = s
                            .target
                            .map(|t| t.to_string_lossy().to_string())
                            .filter(|t| t.to_lowercase().ends_with(".exe"));
                        out.insert(p.to_string_lossy().to_lowercase(), (target, s.app_id));
                    }
            }
        }
    }
    out
}

pub fn load() -> UsageData {
    let mut data = UsageData::default();
    let shortcuts = shortcut_index();
    let by_aumid: HashMap<String, String> = shortcuts
        .values()
        .filter_map(|(t, a)| Some((a.clone()?.to_lowercase(), t.clone()?)))
        .collect();
    let now = Local::now();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for key in USER_ASSIST {
        let Ok(k) = hkcu.open_subkey(key) else { continue };
        for (raw, value) in k.enum_values().flatten() {
            if value.bytes.len() < 68 {
                continue;
            }
            let ft = u64::from_le_bytes(value.bytes[60..68].try_into().unwrap_or_default());
            let Some(t) = super::inventory::filetime_to_local(ft) else { continue };
            if t.year_ce().1 < 2000 || t > now + chrono::Duration::days(1) {
                continue;
            }
            add_entry(&mut data, &rot13(&raw), t, &shortcuts, &by_aumid);
        }
    }
    if shell::is_admin() {
        read_prefetch(&mut data);
    }
    for (id, t) in games::steam_last_played() {
        if let Some(dt) = chrono::DateTime::from_timestamp(t, 0) {
            data.steam.insert(id, dt.with_timezone(&Local));
        }
    }
    data
}

use chrono::Datelike;

fn add_entry(
    data: &mut UsageData,
    name: &str,
    t: DateTime<Local>,
    shortcuts: &HashMap<String, (Option<String>, Option<String>)>,
    by_aumid: &HashMap<String, String>,
) {
    if name.starts_with("UEME_") {
        return;
    }
    if let Some(mut path) = expand_known_folder(name) {
        if path.to_lowercase().ends_with(".lnk") {
            match shortcuts.get(&path.to_lowercase()).and_then(|(t, _)| t.clone()) {
                Some(target) => path = target,
                None => return,
            }
        }
        put(&mut data.by_path, path.to_lowercase(), t);
        return;
    }
    if let Some(bang) = name.find('!')
        && bang > 0 && name.contains('_') {
            put(&mut data.by_package, name[..bang].to_lowercase(), t); // PackageFamilyName!AppId
            return;
        }
    if let Some(target) = by_aumid.get(&name.to_lowercase()) {
        put(&mut data.by_path, target.to_lowercase(), t);
        return;
    }
    if !name.starts_with("Microsoft.AutoGenerated") {
        data.unresolved.push((name.to_string(), t));
    }
}

fn read_prefetch(data: &mut UsageData) {
    let Ok(rd) = std::fs::read_dir(shell::windows_dir().join("Prefetch")) else { return };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if !n.to_lowercase().ends_with(".pf") {
            continue;
        }
        let Some(dash) = n.rfind('-').filter(|&d| d > 0) else { continue };
        if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
            put(&mut data.by_exe_name, n[..dash].to_uppercase(), DateTime::<Local>::from(m));
        }
    }
}

static EXE_CACHE: LazyLock<Mutex<HashMap<String, Vec<String>>>> = LazyLock::new(Default::default);

/// EXE names in the program folder (two levels deep), helpers and uninstallers excluded.
fn executables_in(folder: &Path) -> Vec<String> {
    let key = folder.to_string_lossy().to_lowercase();
    if let Some(v) = EXE_CACHE.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return v;
    }
    let mut list = Vec::new();
    fn scan(dir: &Path, depth: u32, list: &mut Vec<String>) {
        if list.len() > 300 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if depth < 2 {
                    scan(&p, depth + 1, list);
                }
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) {
                let n = e.file_name().to_string_lossy().to_string();
                if !HELPER_EXE.is_match(&n) {
                    list.push(n);
                }
            }
        }
    }
    scan(folder, 0, &mut list);
    if let Ok(mut c) = EXE_CACHE.lock() {
        c.insert(key, list.clone());
    }
    list
}

fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

/// Last launch of the program, or None when Windows has no record of it.
pub fn last_used(app: &App, data: &UsageData) -> Option<DateTime<Local>> {
    let mut best: Option<DateTime<Local>> = None;
    let mut take = |t: DateTime<Local>| {
        if best.is_none_or(|b| t > b) {
            best = Some(t);
        }
    };
    if app.is_store {
        if let Some(t) = app.package_family_name.as_ref().and_then(|p| data.by_package.get(&p.to_lowercase())) {
            take(*t);
        }
        return best;
    }
    if let Some(t) = app.steam_app_id.as_ref().and_then(|id| data.steam.get(id)) {
        take(*t);
    }
    if let Some(loc) = app.install_location.as_ref() {
        let prefix = format!("{}\\", loc.to_string_lossy().trim_end_matches('\\').to_lowercase());
        for (path, t) in &data.by_path {
            if path.starts_with(&prefix) {
                let file = path.rsplit('\\').next().unwrap_or_default();
                if !HELPER_EXE.is_match(file) {
                    take(*t);
                }
            }
        }
        if data.has_prefetch() {
            for exe in executables_in(loc) {
                let upper = exe.to_uppercase();
                if let Some(t) = data.by_exe_name.get(&upper) {
                    take(*t);
                } else if upper.chars().count() > 29 {
                    // Prefetch keeps only the first 29 characters of long names
                    let short: String = upper.chars().take(29).collect();
                    if let Some(t) = data.by_exe_name.get(&short) {
                        take(*t);
                    }
                }
            }
        }
    }
    let norm = normalize(&app.name);
    if norm.chars().count() >= 4 {
        for (id, t) in &data.unresolved {
            let n = normalize(id);
            if n.contains(&norm) || (n.chars().count() >= 6 && norm.contains(&n)) {
                take(*t);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rot13_roundtrip() {
        assert_eq!(rot13("Uryyb, Jbeyq!"), "Hello, World!");
        assert_eq!(rot13(&rot13("C:\\Program Files")), "C:\\Program Files");
    }

    #[test]
    fn known_folder_names() {
        // FOLDERID_ProgramFilesX64 = {6D809377-6AF0-444B-8957-A3773F02200E}
        let p = expand_known_folder(r"{6D809377-6AF0-444B-8957-A3773F02200E}\App\app.exe").unwrap();
        assert!(p.to_lowercase().ends_with(r"\app\app.exe") && p.contains(':'), "{p}");
        assert_eq!(expand_known_folder(r"C:\x.exe").as_deref(), Some(r"C:\x.exe"));
        assert!(expand_known_folder("Microsoft.Windows.Explorer").is_none());
    }

    #[test]
    fn loads_usage() {
        let d = load();
        assert!(!d.by_path.is_empty() || !d.by_package.is_empty(), "UserAssist has entries on a used PC");
    }
}
