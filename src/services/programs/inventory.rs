//! Installed desktop programs (registry) and Microsoft Store apps (port of Services/AppInventory.cs
//! and Models/InstalledApp.cs).

use super::advisor::{self, Hint};
use super::games;
use crate::sys::{cmdline, shell};
use chrono::{DateTime, Local, NaiveDate, TimeZone};
use std::path::{Path, PathBuf};
use winreg::RegKey;
use winreg::enums::*;

pub const UNINSTALL_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hive {
    LocalMachine,
    CurrentUser,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Wow64,
    Wow32,
}

#[derive(Clone, Debug, Default)]
pub struct App {
    pub id: String,
    pub name: String,
    pub publisher: Option<String>,
    pub version: Option<String>,
    pub install_date: Option<DateTime<Local>>,
    pub install_location: Option<PathBuf>,
    pub uninstall_string: Option<String>,
    pub quiet_uninstall_string: Option<String>,
    pub modify_path: Option<String>,
    pub display_icon: Option<String>,
    pub help_link: Option<String>,
    pub is_msi: bool,
    pub product_code: Option<String>,
    pub no_remove: bool,

    // registry origin (desktop programs)
    pub hive: Option<Hive>,
    pub view: Option<View>,
    pub registry_subkey: Option<String>,

    // Microsoft Store
    pub is_store: bool,
    pub package_full_name: Option<String>,
    pub package_family_name: Option<String>,
    pub logo_path: Option<PathBuf>,

    // games
    pub steam_app_id: Option<String>,
    pub steam_manifest: Option<PathBuf>,
    pub extra_locations: Vec<PathBuf>,
    /// "Epic Games", "GOG", "EA", "Ubisoft Connect", "Battle.net", "Xbox" (Steam games use `steam_app_id`).
    pub game_source: Option<String>,
    /// Launcher manifest that disappears once the launcher removed the game (Epic).
    pub launcher_manifest: Option<PathBuf>,
    /// Set for games removed through a launcher (see launcher_name()).
    pub launcher_name: Option<String>,

    /// Bytes; None — unknown.
    pub size: Option<u64>,
    pub size_measured: bool,
    pub hint: Hint,
    pub last_used: Option<DateTime<Local>>,
    pub unused: bool,
    /// Newer version available through winget, and its package id.
    pub update_version: Option<String>,
    pub winget_id: Option<String>,
}

impl App {
    pub fn is_steam_game(&self) -> bool {
        self.steam_app_id.is_some()
    }
    pub fn is_game(&self) -> bool {
        self.is_steam_game() || self.game_source.is_some()
    }
    pub fn game_label(&self) -> Option<&str> {
        if self.is_steam_game() { Some("Steam") } else { self.game_source.as_deref() }
    }
    /// The program that performs the uninstall (Steam, Epic Games Launcher, Ubisoft Connect).
    pub fn launcher_name(&self) -> Option<&str> {
        if let Some(l) = &self.launcher_name {
            return Some(l);
        }
        let steam_cmd = self.uninstall_string.as_ref().is_some_and(|u| u.to_lowercase().contains("steam://"));
        (self.is_steam_game() || steam_cmd).then_some("Steam")
    }
    pub fn set_launcher(&mut self, name: &str) {
        self.launcher_name = Some(name.to_string());
    }
    pub fn is_launcher_uninstall(&self) -> bool {
        self.launcher_name().is_some()
    }
    pub fn can_uninstall(&self) -> bool {
        !self.no_remove && (self.is_store || self.is_msi || self.uninstall_string.as_ref().is_some_and(|u| !u.trim().is_empty()))
    }
    pub fn has_quiet(&self) -> bool {
        !self.is_launcher_uninstall() && (self.quiet_uninstall_string.as_ref().is_some_and(|q| !q.trim().is_empty()) || self.is_msi)
    }
    pub fn can_modify(&self) -> bool {
        !self.is_store && self.modify_path.as_ref().is_some_and(|m| !m.trim().is_empty())
    }
    pub fn has_registry_key(&self) -> bool {
        !self.is_store && self.registry_subkey.is_some()
    }
    pub fn has_update(&self) -> bool {
        self.update_version.as_ref().is_some_and(|v| !v.is_empty())
    }

    /// "HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\…" for regedit and reg.exe.
    pub fn registry_full_path(&self) -> Option<String> {
        let sub = self.registry_subkey.as_ref()?;
        if self.is_store {
            return None;
        }
        Some(match (self.hive?, self.view?) {
            (Hive::LocalMachine, View::Wow32) => {
                format!(r"HKEY_LOCAL_MACHINE\{}", sub.replacen(r"SOFTWARE\Microsoft", r"SOFTWARE\WOW6432Node\Microsoft", 1))
            }
            (Hive::LocalMachine, View::Wow64) => format!(r"HKEY_LOCAL_MACHINE\{sub}"),
            (Hive::CurrentUser, _) => format!(r"HKEY_CURRENT_USER\{sub}"),
        })
    }

    pub fn source_text(&self) -> String {
        if self.is_store {
            return match &self.game_source {
                Some(g) => format!("{g} · Microsoft Store"),
                None => "Microsoft Store".into(),
            };
        }
        if let Some(id) = &self.steam_app_id {
            return format!("Steam · ID {id}");
        }
        if self.launcher_manifest.is_some() {
            return self.game_source.clone().unwrap_or_default();
        }
        match (self.hive, self.view) {
            (Some(Hive::CurrentUser), _) => "Только для текущего пользователя".into(),
            (_, Some(View::Wow32)) => "Для всех · 32-бит".into(),
            _ => "Для всех · 64-бит".into(),
        }
    }

    pub fn kind_text(&self) -> String {
        if let Some(l) = self.game_label() {
            format!("Игра {l}")
        } else if self.is_store {
            "Приложение Store".into()
        } else if self.is_msi {
            "Установщик Windows (MSI)".into()
        } else {
            "Классическая программа".into()
        }
    }

    pub fn subtitle(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(g) = self.game_label() {
            parts.push(g.to_string());
        } else if self.is_store {
            parts.push("Microsoft Store".into());
        }
        if let Some(p) = self.publisher.as_ref().filter(|p| !p.trim().is_empty() && Some(p.as_str()) != self.game_label()) {
            parts.push(p.clone());
        }
        if let Some(v) = self.version.as_ref().filter(|v| !v.trim().is_empty()) {
            parts.push(v.clone());
        }
        parts.join("  ·  ")
    }

    /// Path whose icon represents the program (exe, icon file or Store logo).
    pub fn icon_source(&self) -> Option<PathBuf> {
        if self.is_store {
            return self.logo_path.clone();
        }
        if let Some(f) = self.display_icon.as_deref().and_then(icon_file) {
            return Some(f);
        }
        if let Some(loc) = self.install_location.as_ref().filter(|l| l.is_dir())
            && let Some(exe) = main_exe(loc, &self.name) {
                return Some(exe);
            }
        let un = cmdline::extract_executable(self.uninstall_string.as_deref().unwrap_or_default())?;
        let l = un.to_lowercase();
        (!l.ends_with("msiexec.exe") && !l.ends_with("rundll32.exe")).then(|| PathBuf::from(un))
    }
}

/// "C:\x\app.exe,0" → the file, when it exists.
fn icon_file(icon: &str) -> Option<PathBuf> {
    let mut s = icon.trim();
    if let Some(comma) = s.rfind(',')
        && comma > 2 {
            s = &s[..comma];
        }
    let p = PathBuf::from(s.trim().trim_matches('"'));
    p.is_file().then_some(p)
}

/// The program's own executable inside its folder (helpers and uninstallers skipped).
fn main_exe(dir: &Path, name: &str) -> Option<PathBuf> {
    let exes: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")))
        .filter(|p| {
            let n = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            !["unins", "uninst", "update", "crash", "helper", "setup"].iter().any(|b| n.contains(b))
        })
        .collect();
    let key: String = name.chars().filter(|c| c.is_alphanumeric()).take(6).collect::<String>().to_lowercase();
    exes.iter()
        .find(|p| key.chars().count() > 2 && p.file_stem().unwrap_or_default().to_string_lossy().to_lowercase().contains(&key))
        .cloned()
        .or_else(|| exes.iter().max_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)).cloned())
}

// ------------------------------------------------------------------ loading

pub fn load_all() -> Vec<App> {
    let mut list = load_desktop();
    list.extend(load_store());
    games::tag_xbox(&mut list);
    for a in &mut list {
        a.hint = advisor::classify(a);
    }
    list
}

pub fn load_desktop() -> Vec<App> {
    let mut result = Vec::new();
    read(Hive::LocalMachine, View::Wow64, &mut result);
    read(Hive::LocalMachine, View::Wow32, &mut result);
    read(Hive::CurrentUser, View::Wow64, &mut result);
    // the same product can be registered several times (per view / per user): keep the richest entry
    let mut groups: std::collections::BTreeMap<(String, String), Vec<App>> = Default::default();
    for a in result {
        groups.entry((a.name.to_lowercase(), a.version.clone().unwrap_or_default().to_lowercase())).or_default().push(a);
    }
    let mut list: Vec<App> = groups
        .into_values()
        .filter_map(|mut g| {
            g.sort_by_key(|a| std::cmp::Reverse((a.size.unwrap_or(0), a.install_location.is_some())));
            g.into_iter().next()
        })
        .collect();
    games::attach_steam(&mut list);
    games::attach_launchers(&mut list);
    list
}

fn root(hive: Hive, view: View) -> Option<RegKey> {
    let base = RegKey::predef(match hive {
        Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        Hive::CurrentUser => HKEY_CURRENT_USER,
    });
    let flags = KEY_READ | if view == View::Wow32 { KEY_WOW64_32KEY } else { KEY_WOW64_64KEY };
    base.open_subkey_with_flags(UNINSTALL_KEY, flags).ok()
}

fn read(hive: Hive, view: View, out: &mut Vec<App>) {
    let Some(root) = root(hive, view) else { return };
    for key in root.enum_keys().flatten() {
        if let Ok(k) = root.open_subkey(&key)
            && let Some(app) = from_key(&k, &key, hive, view) {
                out.push(app);
            }
    }
}

fn get_str(k: &RegKey, name: &str) -> Option<String> {
    k.get_value::<String, _>(name).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn get_num(k: &RegKey, name: &str) -> u64 {
    if let Ok(v) = k.get_value::<u32, _>(name) {
        return v as u64;
    }
    if let Ok(v) = k.get_value::<u64, _>(name) {
        return v;
    }
    get_str(k, name).and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn clean(s: Option<String>) -> Option<String> {
    let s = cmdline::expand_env(s?.trim().trim_matches('"').trim());
    (!s.is_empty()).then_some(s)
}

fn is_guid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 38
        && b[0] == b'{'
        && b[37] == b'}'
        && s[1..37].char_indices().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() })
}

fn is_update_name(name: &str) -> bool {
    let l = name.to_lowercase();
    if l.starts_with("update for ") || l.starts_with("security update for ") {
        return true;
    }
    // "(KB1234567)"
    l.find("(kb").is_some_and(|i| {
        let digits: String = l[i + 3..].chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.len() >= 6 && l[i + 3 + digits.len()..].starts_with(')')
    })
}

fn from_key(k: &RegKey, key_name: &str, hive: Hive, view: View) -> Option<App> {
    let name = get_str(k, "DisplayName")?;
    if get_num(k, "SystemComponent") == 1 || get_str(k, "ParentKeyName").is_some() {
        return None;
    }
    if matches!(get_str(k, "ReleaseType").as_deref(), Some("Update" | "Hotfix" | "Security Update" | "Service Pack")) {
        return None;
    }
    if is_update_name(&name) {
        return None;
    }
    let uninstall = get_str(k, "UninstallString");
    let is_msi = get_num(k, "WindowsInstaller") == 1 && is_guid(key_name);
    if uninstall.is_none() && !is_msi {
        return None;
    }
    let icon = clean(get_str(k, "DisplayIcon"));
    let mut location = clean(get_str(k, "InstallLocation")).map(PathBuf::from).filter(|p| is_usable_location(p));
    if location.is_none() {
        location = guess_location(icon.as_deref(), uninstall.as_deref());
    }
    let date = get_str(k, "InstallDate").and_then(|s| parse_date(&s)).or_else(|| key_write_time(k));
    let size_kb = get_num(k, "EstimatedSize");
    Some(App {
        id: format!("{hive:?}|{view:?}|{key_name}"),
        name,
        publisher: get_str(k, "Publisher"),
        version: get_str(k, "DisplayVersion"),
        install_date: date,
        install_location: location,
        uninstall_string: uninstall,
        quiet_uninstall_string: get_str(k, "QuietUninstallString"),
        modify_path: if get_num(k, "NoModify") == 1 { None } else { get_str(k, "ModifyPath") },
        display_icon: icon,
        help_link: get_str(k, "URLInfoAbout").or_else(|| get_str(k, "HelpLink")),
        is_msi,
        product_code: is_msi.then(|| key_name.to_string()),
        no_remove: get_num(k, "NoRemove") == 1,
        hive: Some(hive),
        view: Some(view),
        registry_subkey: Some(format!(r"{UNINSTALL_KEY}\{key_name}")),
        size: (size_kb > 0).then_some(size_kb * 1024),
        ..Default::default()
    })
}

fn key_write_time(k: &RegKey) -> Option<DateTime<Local>> {
    let info = k.query_info().ok()?;
    let ft = (info.last_write_time.dwHighDateTime as u64) << 32 | info.last_write_time.dwLowDateTime as u64;
    filetime_to_local(ft)
}

pub fn filetime_to_local(ft: u64) -> Option<DateTime<Local>> {
    let secs = (ft / 10_000_000).checked_sub(11_644_473_600)? as i64;
    Local.timestamp_opt(secs, 0).single()
}

/// "20240131", "2024-01-31", "31.01.2024", "1/31/2024", …
pub fn parse_date(s: &str) -> Option<DateTime<Local>> {
    let s = s.trim();
    let formats = ["%Y%m%d", "%Y-%m-%d", "%Y.%m.%d", "%d.%m.%Y", "%m/%d/%Y", "%d/%m/%Y", "%Y/%m/%d"];
    let d = formats.iter().find_map(|f| NaiveDate::parse_from_str(s, f).ok())?;
    let dt = Local.from_local_datetime(&d.and_hms_opt(12, 0, 0)?).single()?;
    (d.year_ce().1 >= 1995 && dt <= Local::now() + chrono::Duration::days(1)).then_some(dt)
}

use chrono::Datelike;

/// Well-known folders that are never a program's own folder.
fn protected() -> Vec<String> {
    let home = shell::home();
    let local = shell::local_app_data();
    [
        shell::windows_dir(),
        crate::sys::cmd::system_tool(""),
        shell::program_files(),
        shell::program_files_x86(),
        shell::program_files().join("Common Files"),
        shell::program_files_x86().join("Common Files"),
        shell::program_data(),
        shell::roaming_app_data(),
        local.clone(),
        home.clone(),
        home.join("Documents"),
        home.join("Desktop"),
        local.join("Programs"),
        home.join("AppData"),
        home.join(r"AppData\LocalLow"),
    ]
    .iter()
    .map(|p| p.to_string_lossy().trim_end_matches('\\').to_lowercase())
    .collect()
}

/// A real program folder path (not a drive root or a well-known system folder); it may not exist.
pub fn is_usable_location_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    let full = s.trim_end_matches('\\');
    if !p.is_absolute() || full.len() <= 3 {
        return false;
    }
    let l = full.to_lowercase();
    if protected().contains(&l) {
        return false;
    }
    let win = shell::windows_dir().to_string_lossy().to_lowercase();
    !(l.starts_with(&format!("{win}\\")) || l.contains(r"\package cache\") || l.contains(r"\installer\"))
}

pub fn is_usable_location(p: &Path) -> bool {
    is_usable_location_path(p) && p.is_dir()
}

fn guess_location(icon: Option<&str>, uninstall: Option<&str>) -> Option<PathBuf> {
    let candidates = [icon.and_then(icon_file), uninstall.and_then(cmdline::extract_executable).map(PathBuf::from)];
    candidates.into_iter().flatten().filter_map(|c| c.parent().map(|p| p.to_path_buf())).find(|d| is_usable_location(d))
}

// ------------------------------------------------------------------ Microsoft Store

const HIDDEN_STORE_PREFIXES: &[&str] = &[
    "Microsoft.WindowsStore",
    "Microsoft.DesktopAppInstaller",
    "Microsoft.StorePurchaseApp",
    "Microsoft.SecHealthUI",
    "Microsoft.Windows.",
    "MicrosoftWindows.",
    "Windows.",
    "Microsoft.AAD.",
    "Microsoft.AccountsControl",
    "Microsoft.LockApp",
    "Microsoft.ECApp",
    "Microsoft.CredDialogHost",
    "Microsoft.Win32WebViewHost",
    "Microsoft.XboxGameCallableUI",
    "NcsiUwpApp",
    "Microsoft.AsyncTextService",
    "Microsoft.BioEnrollment",
    "Microsoft.MicrosoftEdgeDevToolsClient",
    "Microsoft.WindowsAppRuntime",
    "MicrosoftCorporationII.WinAppRuntime",
    "Microsoft.WinAppRuntime",
    "Microsoft.ApplicationCompatibilityEnhancements",
    "Microsoft.VCLibs",
    "Microsoft.UI.Xaml",
    "Microsoft.NET.",
    "Microsoft.Services.Store",
    "Microsoft.Advertising",
    "Microsoft.Ink.",
    "Microsoft.WidgetsPlatformRuntime",
    "Microsoft.Winget",
    "Microsoft.OneDriveSync",
    "c5e2524a-ea46-4f67-841f-6a9465d9d515",
    "1527c705-839a-4832-9118-54d4Bd6a0c89",
    "E2A4F912-2574-4A75-9BB0-0D023378592B",
    "F46D4000-FD22-4DB4-AC8E-4E1DDDE828FE",
    "windows.immersivecontrolpanel",
];

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.is_char_boundary(prefix.len()) && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

pub fn com_init() {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
}

/// Store apps of the current user through the WinRT PackageManager.
pub fn load_store() -> Vec<App> {
    com_init();
    let mut list = Vec::new();
    let Ok(pm) = windows::Management::Deployment::PackageManager::new() else { return list };
    let Ok(packages) = pm.FindPackagesByUserSecurityId(&windows::core::HSTRING::new()) else { return list };
    for p in packages {
        let ok = (|| -> windows::core::Result<Option<App>> {
            if p.IsFramework()? || p.IsResourcePackage()? || p.IsBundle()? {
                return Ok(None);
            }
            if p.SignatureKind()? == windows::ApplicationModel::PackageSignatureKind::System {
                return Ok(None);
            }
            let id = p.Id()?;
            let pkg_name = id.Name()?.to_string();
            if HIDDEN_STORE_PREFIXES.iter().any(|x| starts_with_ci(&pkg_name, x)) {
                return Ok(None);
            }
            let name = p.DisplayName().map(|s| s.to_string()).unwrap_or_default();
            if name.trim().is_empty() || starts_with_ci(&name, "ms-resource:") {
                return Ok(None);
            }
            let location = p.InstalledPath().map(|s| s.to_string()).ok().filter(|s| !s.is_empty());
            let publisher = p.PublisherDisplayName().map(|s| s.to_string()).ok().filter(|s| !s.is_empty());
            let date = p.InstalledDate().ok().and_then(|d| {
                // 100 ns ticks since 1601
                filetime_to_local(d.UniversalTime as u64)
            });
            let logo = p.Logo().ok().and_then(|u| u.RawUri().ok()).and_then(|u| resolve_logo(&u.to_string()));
            let v = id.Version()?;
            let full = id.FullName()?.to_string();
            Ok(Some(App {
                id: format!("store|{full}"),
                name: name.trim().to_string(),
                publisher,
                version: Some(format!("{}.{}.{}.{}", v.Major, v.Minor, v.Build, v.Revision)),
                install_date: date.filter(|d| d.year() > 2000),
                install_location: location.map(PathBuf::from),
                is_store: true,
                package_full_name: Some(full),
                package_family_name: Some(id.FamilyName()?.to_string()),
                logo_path: logo,
                ..Default::default()
            }))
        })();
        if let Ok(Some(app)) = ok {
            list.push(app);
        }
    }
    list
}

/// Store logos are declared without the scale qualifier: pick the best real file.
fn resolve_logo(uri: &str) -> Option<PathBuf> {
    let path = uri.strip_prefix("file:///").unwrap_or(uri).replace('/', "\\");
    let path = percent_decode(&path);
    let p = PathBuf::from(&path);
    if p.is_file() {
        return Some(p);
    }
    let dir = p.parent()?;
    let stem = p.file_stem()?.to_string_lossy().to_lowercase();
    let ext = p.extension()?.to_string_lossy().to_lowercase();
    let files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|f| {
            let n = f.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            n.starts_with(&format!("{stem}.")) && n.ends_with(&format!(".{ext}")) && !n.contains("contrast")
        })
        .collect();
    let pick = |t: &str| files.iter().find(|f| f.to_string_lossy().to_lowercase().contains(t)).cloned();
    pick("scale-200").or_else(|| pick("scale-150")).or_else(|| pick("targetsize-48")).or_else(|| pick("scale-100")).or_else(|| files.first().cloned())
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Folder size when the installer did not report one. Folders of other programs nested inside are skipped.
pub fn measure_size(app: &App, other_locations: &[String]) -> u64 {
    use std::sync::atomic::AtomicBool;
    let never = AtomicBool::new(false);
    let mut total = 0;
    if let Some(loc) = app.install_location.as_ref().filter(|l| l.is_dir()) {
        let own = loc.to_string_lossy().trim_end_matches('\\').to_lowercase();
        let nested: Vec<&String> = other_locations.iter().filter(|o| o.starts_with(&format!("{own}\\"))).collect();
        total += dir_size(loc, &nested, &never);
    }
    for extra in app.extra_locations.iter().filter(|p| p.is_dir()) {
        total += dir_size(extra, &[], &never);
    }
    if let Some(pfn) = app.package_family_name.as_ref().filter(|_| app.is_store) {
        let data = shell::local_app_data().join("Packages").join(pfn);
        if data.is_dir() {
            total += dir_size(&data, &[], &never);
        }
    }
    total
}

fn dir_size(dir: &Path, skip: &[&String], cancel: &std::sync::atomic::AtomicBool) -> u64 {
    use std::os::windows::fs::MetadataExt;
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                let p = e.path();
                let l = p.to_string_lossy().to_lowercase();
                if !skip.iter().any(|s| **s == l) {
                    stack.push(p);
                }
            } else if let Ok(md) = e.metadata()
                && md.file_attributes() & 0x0040_0000 == 0 {
                    total += md.len();
                }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_guids() {
        assert!(parse_date("20240131").is_some());
        assert!(parse_date("31.01.2024").is_some());
        assert!(parse_date("1990-01-01").is_none());
        assert!(parse_date("garbage").is_none());
        assert!(is_guid("{90160000-008C-0000-1000-0000000FF1CE}"));
        assert!(!is_guid("Steam App 570"));
        assert!(is_update_name("Security Update for Microsoft Office (KB1234567)"));
        assert!(!is_update_name("Telegram Desktop"));
    }

    #[test]
    fn protects_system_folders() {
        assert!(!is_usable_location_path(&shell::program_files()));
        assert!(!is_usable_location_path(Path::new(r"C:\")));
        assert!(!is_usable_location_path(&shell::windows_dir().join("System32")));
        assert!(is_usable_location_path(&shell::program_files().join("SomeApp")));
    }

    #[test]
    fn loads_installed_programs() {
        let apps = load_all();
        assert!(apps.len() > 5, "found {}", apps.len());
        assert!(apps.iter().all(|a| !a.name.is_empty()));
        let mut ids: Vec<&str> = apps.iter().map(|a| a.id.as_str()).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "ids are unique");
        assert!(apps.iter().any(|a| a.is_store), "store apps are listed");
    }
}
