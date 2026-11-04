//! What a program leaves behind: folders, shortcuts, registry keys, autostart, tasks, services, firewall
//! rules — and their removal with backups (port of Services/Leftovers.cs).
//!
//! Fixed compared to the C# version: a raw publisher name with characters that are illegal in paths made
//! the folder search throw and silently skip every candidate; a task counted as "own" when the user name
//! merely ended with the current user's name.

use super::inventory::{self, App};
use super::uninstall;
use crate::data::settings;
use crate::services::startup;
use crate::services::tasks;
use crate::sys::cmd::{self, Encoding};
use crate::sys::{cmdline, fileinfo, recycle, shell};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;
use winreg::RegKey;
use winreg::enums::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Shortcut,
    RegistryKey,
    StartupEntry,
    Task,
    Service,
    FirewallRule,
}

impl Kind {
    pub fn text(self) -> &'static str {
        match self {
            Kind::Folder => "Папка",
            Kind::Shortcut => "Ярлык",
            Kind::RegistryKey => "Реестр",
            Kind::StartupEntry => "Автозагрузка",
            Kind::Task => "Задача планировщика",
            Kind::Service => "Служба",
            Kind::FirewallRule => "Брандмауэр",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Leftover {
    pub kind: Kind,
    /// Folder/file path, "HKEY_…\key", task path, service name or program path of firewall rules.
    pub target: String,
    /// Run value name for `StartupEntry`.
    pub value_name: Option<String>,
    pub title: String,
    pub size: u64,
    pub needs_admin: bool,
}

#[derive(Default, Debug)]
pub struct Removal {
    pub removed: u32,
    pub failed: u32,
    pub bytes: u64,
    pub recycled: Vec<String>,
    pub reg_backups: Vec<String>,
    pub task_backups: Vec<String>,
}

static VERSION_RX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*(v(er(sion)?)?\.?\s*)?\d+(\.\d+)+.*$").unwrap());
static PARENS_RX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[\(\[].*?[\)\]]").unwrap());
static ARCH_RX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(x64|x86|64-bit|32-bit|64 bit|32 bit|amd64|win64|win32)\b").unwrap());
static COMPANY_RX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[,\.]?\s*\b(inc|llc|ltd|limited|corp|corporation|co|gmbh|s\.?r\.?o|ag|sa|bv|oy|ab|ооо)\b\.?").unwrap()
});

/// Vendor / system names shared by many programs — never offered for removal.
const SHARED_NAMES: &[&str] = &[
    "Microsoft", "Google", "Packages", "Temp", "Programs", "Common Files", "Windows", "Intel", "AMD", "NVIDIA",
    "NVIDIA Corporation", "Mozilla", "Adobe", "Apple", "Oracle", "Package Cache", "Installer", "Downloaded Installations",
    "Microsoft Office", "Windows Kits", "dotnet", "Steam", "Epic Games", "JetBrains", "Yandex", "Valve", "Realtek",
    "Classes", "Policies", "Wow6432Node", "Clients", "RegisteredApplications", "ODBC", "Khronos", "Python", "Wow6432",
    "Intel Corporation", "Advanced Micro Devices, Inc.", "Logitech", "Razer", "Corsair", "Ubisoft", "Electronic Arts",
    "GOG.com", "Blizzard Entertainment", "Battle.net", "Origin", "EA Games", "Unity", "Unreal Engine", "Epic Games Launcher",
    "Prostor", "Простор",
];

fn shared(name: &str) -> bool {
    SHARED_NAMES.iter().any(|s| s.eq_ignore_ascii_case(name) || s.to_lowercase() == name.to_lowercase())
}

/// Names under which a program may have left folders and keys: without version, architecture and brackets.
pub fn clean_names(name: &str) -> Vec<String> {
    let mut list = Vec::new();
    if name.trim().is_empty() {
        return list;
    }
    let n = PARENS_RX.replace_all(name, "");
    let n = ARCH_RX.replace_all(&n, "");
    let n = VERSION_RX.replace_all(&n, "");
    let n = n.trim_matches([' ', '-', '–', '_', '.', ',']).to_string();
    if n.chars().count() >= 3 {
        list.push(n.clone());
    }
    let no_spaces = n.replace(' ', "");
    if no_spaces.chars().count() >= 3 && no_spaces != n {
        list.push(no_spaces);
    }
    list
}

pub fn clean_publisher(p: &str) -> Option<String> {
    let p = COMPANY_RX.replace_all(p, "").trim_matches([' ', ',', '.', '-']).to_string();
    (p.chars().count() >= 2).then_some(p)
}

/// A path segment that is safe to put into a folder path.
fn path_safe(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && !s.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*'])).then(|| s.to_string())
}

fn same_or_parent(parent: &str, child: &str) -> bool {
    crate::sys::path_within(child, parent)
}

fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

pub fn find(app: &App, remaining: &[App], include_install_folder: bool) -> Vec<Leftover> {
    let mut result = Vec::new();
    let names = clean_names(&app.name);
    let publisher = app.publisher.as_deref().and_then(clean_publisher);
    let location: Option<String> = app
        .install_location
        .as_ref()
        .filter(|l| inventory::is_usable_location_path(l))
        .map(|l| l.to_string_lossy().trim_end_matches('\\').to_string());
    let others: Vec<&App> = remaining.iter().filter(|a| a.id != app.id).collect();
    let other_locations: Vec<String> =
        others.iter().filter_map(|a| a.install_location.as_ref()).map(|l| l.to_string_lossy().trim_end_matches('\\').to_string()).collect();
    let other_names: Vec<String> = others.iter().flat_map(|a| clean_names(&a.name)).map(|n| n.to_lowercase()).collect();
    let mine = |p: &str| location.as_ref().is_some_and(|l| same_or_parent(l, p));

    find_folders(app, &names, publisher.as_deref(), if include_install_folder { location.as_deref() } else { None }, &other_locations, &mut result);
    find_shortcuts(&names, location.as_deref(), &mut result);
    find_registry(app, &names, publisher.as_deref(), &other_names, &mut result);
    if location.is_some() {
        find_startup(&mine, &mut result);
        find_tasks(&mine, &mut result);
        find_services(&mine, &mut result);
        find_firewall(&mine, &mut result);
    }
    let s = settings::get();
    result.retain(|r| !(matches!(r.kind, Kind::Folder | Kind::Shortcut) && s.is_excluded(&r.target)));
    result
}

fn find_folders(app: &App, names: &[String], publisher: Option<&str>, location: Option<&str>, other_locations: &[String], out: &mut Vec<Leftover>) {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(l) = location {
        candidates.push(PathBuf::from(l));
    }
    let local = shell::local_app_data();
    let roots = [
        shell::program_files(),
        shell::program_files_x86(),
        shell::roaming_app_data(),
        local.clone(),
        local.join("Programs"),
        shell::home().join(r"AppData\LocalLow"),
        shell::program_data(),
    ];
    let pubs: Vec<String> = [publisher.map(str::to_string), app.publisher.clone()].into_iter().flatten().filter_map(|p| path_safe(&p)).collect();
    for root in &roots {
        for n in names.iter().filter_map(|n| path_safe(n)) {
            candidates.push(root.join(&n));
            for p in &pubs {
                candidates.push(root.join(p).join(&n));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    for c in candidates {
        let full = c.to_string_lossy().trim_end_matches('\\').to_string();
        if !seen.insert(full.to_lowercase()) || !inventory::is_usable_location(&c) {
            continue;
        }
        let leaf = c.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        if shared(&leaf) {
            continue;
        }
        // never touch a folder that still belongs to (or contains) another installed program
        if other_locations.iter().any(|o| same_or_parent(&full, o) || same_or_parent(o, &full)) {
            continue;
        }
        if out.iter().any(|r| r.kind == Kind::Folder && same_or_parent(&r.target, &full)) {
            continue;
        }
        let size = crate::services::cleanup::folder_size(&c);
        out.push(Leftover { kind: Kind::Folder, target: full, value_name: None, title: leaf, size, needs_admin: false });
    }
}

fn find_shortcuts(names: &[String], location: Option<&str>, out: &mut Vec<Leftover>) {
    let roots: Vec<PathBuf> = [
        Some(shell::roaming_app_data().join(r"Microsoft\Windows\Start Menu\Programs")),
        Some(shell::program_data().join(r"Microsoft\Windows\Start Menu\Programs")),
        Some(shell::home().join("Desktop")),
        std::env::var("PUBLIC").ok().map(|p| PathBuf::from(p).join("Desktop")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let norm: Vec<String> = names.iter().map(|n| normalize(n)).filter(|n| n.chars().count() >= 3).collect();
    for root in roots.iter().filter(|r| r.is_dir()) {
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(p);
                    continue;
                }
                if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
                    continue;
                }
                let Some(target) = fileinfo::read_shortcut(&p).and_then(|s| s.target) else { continue };
                let t = target.to_string_lossy().to_string();
                let by_target = location.is_some_and(|l| same_or_parent(l, &t));
                let broken = !target.exists();
                let stem = p.file_stem().map(|s| normalize(&s.to_string_lossy())).unwrap_or_default();
                let by_name = broken && norm.iter().any(|n| stem.contains(n));
                if !(by_target || by_name) || (by_target && target.is_file()) {
                    continue; // the program is still there — keep its shortcut
                }
                out.push(Leftover {
                    kind: Kind::Shortcut,
                    target: p.to_string_lossy().to_string(),
                    value_name: None,
                    title: p.file_stem().unwrap_or_default().to_string_lossy().to_string(),
                    size: 0,
                    needs_admin: false,
                });
            }
        }
        // an empty Start menu folder named after the program
        for n in names.iter().filter_map(|n| path_safe(n)) {
            let dir = root.join(&n);
            if dir.is_dir() && std::fs::read_dir(&dir).is_ok_and(|mut r| r.next().is_none()) {
                out.push(Leftover {
                    kind: Kind::Folder,
                    target: dir.to_string_lossy().to_string(),
                    value_name: None,
                    title: format!("{n} (пустая папка меню «Пуск»)"),
                    size: 0,
                    needs_admin: false,
                });
            }
        }
    }
}

fn add_key(out: &mut Vec<Leftover>, full: String, title: String, admin: bool) {
    if out.iter().any(|r| r.kind == Kind::RegistryKey && (same_or_parent(&r.target, &full) || same_or_parent(&full, &r.target))) {
        return;
    }
    out.push(Leftover { kind: Kind::RegistryKey, target: full, value_name: None, title, size: 0, needs_admin: admin });
}

fn find_registry(app: &App, names: &[String], publisher: Option<&str>, other_names: &[String], out: &mut Vec<Leftover>) {
    let bases = [
        (HKEY_CURRENT_USER, KEY_WOW64_64KEY, "Software", r"HKEY_CURRENT_USER\Software", false),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY, "SOFTWARE", r"HKEY_LOCAL_MACHINE\SOFTWARE", true),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, "SOFTWARE", r"HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node", true),
    ];
    let mut pubs: Vec<String> = Vec::new();
    for p in [publisher.map(str::to_string), app.publisher.as_ref().map(|p| p.trim().to_string())].into_iter().flatten() {
        if !p.is_empty() && !pubs.iter().any(|x| x.eq_ignore_ascii_case(&p)) {
            pubs.push(p);
        }
    }
    for (hive, view, root, prefix, admin) in bases {
        let Ok(software) = RegKey::predef(hive).open_subkey_with_flags(root, KEY_READ | view) else { continue };
        for n in names {
            if shared(n) || other_names.contains(&n.to_lowercase()) {
                continue;
            }
            if software.open_subkey(n).is_ok() {
                add_key(out, format!(r"{prefix}\{n}"), n.clone(), admin);
            }
            for p in &pubs {
                if shared(p) || p.eq_ignore_ascii_case(n) {
                    continue;
                }
                let Ok(pk) = software.open_subkey(p) else { continue };
                let subs: Vec<String> = pk.enum_keys().flatten().collect();
                if !subs.iter().any(|s| s.eq_ignore_ascii_case(n)) {
                    continue;
                }
                let values = pk.query_info().map(|i| i.values).unwrap_or(1);
                let only_this = subs.len() == 1 && values == 0 && !other_names.contains(&p.to_lowercase());
                if only_this {
                    add_key(out, format!(r"{prefix}\{p}"), p.clone(), admin);
                } else {
                    add_key(out, format!(r"{prefix}\{p}\{n}"), format!(r"{p}\{n}"), admin);
                }
            }
        }
    }
}

/// The exe of a command line even when it no longer exists on disk.
fn raw_exe(cmd: &str) -> Option<String> {
    let c = cmd.trim();
    if let Some(rest) = c.strip_prefix('"') {
        let end = rest.find('"')?;
        return (end > 0).then(|| rest[..end].to_string());
    }
    let l = c.to_lowercase();
    l.find(".exe").map(|i| c[..i + 4].to_string())
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

fn find_startup(mine: &dyn Fn(&str) -> bool, out: &mut Vec<Leftover>) {
    let places = [
        (HKEY_CURRENT_USER, KEY_WOW64_64KEY, format!(r"HKEY_CURRENT_USER\{RUN_KEY}"), false),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY, format!(r"HKEY_LOCAL_MACHINE\{RUN_KEY}"), true),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, r"HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run".to_string(), true),
    ];
    for (hive, view, full, admin) in places {
        let Ok(k) = RegKey::predef(hive).open_subkey_with_flags(RUN_KEY, KEY_READ | view) else { continue };
        for (name, value) in k.enum_values().flatten() {
            let cmd = value.to_string();
            let Some(exe) = cmdline::extract_executable(&cmd).or_else(|| raw_exe(&cmdline::expand_env(&cmd))) else { continue };
            if !mine(&exe) || Path::new(&exe).is_file() {
                continue;
            }
            out.push(Leftover { kind: Kind::StartupEntry, target: full.clone(), value_name: Some(name.clone()), title: name, size: 0, needs_admin: admin });
        }
    }
}

fn find_tasks(mine: &dyn Fn(&str) -> bool, out: &mut Vec<Leftover>) {
    let me = tasks::current_user().to_lowercase();
    let short = std::env::var("USERNAME").unwrap_or_default().to_lowercase();
    for t in startup::list_tasks() {
        let Some(c) = t.command.as_ref().map(|c| cmdline::expand_env(c.trim_matches('"'))) else { continue };
        if c.is_empty() || !mine(&c) || Path::new(&c).is_file() || t.path.to_lowercase().starts_with(r"\prostor\") {
            continue;
        }
        let own = t.user.as_ref().is_some_and(|u| {
            let u = u.to_lowercase();
            u == me || u == short || u.ends_with(&format!("\\{short}"))
        });
        out.push(Leftover { kind: Kind::Task, target: t.path.clone(), value_name: None, title: t.name.clone(), size: 0, needs_admin: !own });
    }
}

fn find_services(mine: &dyn Fn(&str) -> bool, out: &mut Vec<Leftover>) {
    let Ok(root) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"SYSTEM\CurrentControlSet\Services") else { return };
    for name in root.enum_keys().flatten() {
        let Ok(k) = root.open_subkey(&name) else { continue };
        let Ok(image) = k.get_value::<String, _>("ImagePath") else { continue };
        let Some(exe) = raw_exe(&cmdline::expand_env(&image)) else { continue };
        if !mine(&exe) || Path::new(&exe).is_file() {
            continue;
        }
        let display = k.get_value::<String, _>("DisplayName").ok().and_then(|d| fileinfo::indirect_string(&d)).unwrap_or_else(|| name.clone());
        out.push(Leftover { kind: Kind::Service, target: name.clone(), value_name: None, title: display, size: 0, needs_admin: true });
    }
}

const FIREWALL_KEY: &str = r"SYSTEM\CurrentControlSet\Services\SharedAccess\Parameters\FirewallPolicy\FirewallRules";

fn find_firewall(mine: &dyn Fn(&str) -> bool, out: &mut Vec<Leftover>) {
    let Ok(k) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(FIREWALL_KEY) else { return };
    let mut programs: Vec<String> = Vec::new();
    for (_, v) in k.enum_values().flatten() {
        let rule = v.to_string();
        let Some(i) = rule.to_lowercase().find("|app=") else { continue };
        let rest = &rule[i + 5..];
        let app = cmdline::expand_env(rest.split('|').next().unwrap_or_default());
        if mine(&app) && !Path::new(&app).is_file() && !programs.iter().any(|p| p.eq_ignore_ascii_case(&app)) {
            programs.push(app);
        }
    }
    for p in programs {
        let file = Path::new(&p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        out.push(Leftover { kind: Kind::FirewallRule, target: p, value_name: None, title: format!("Правила для {file}"), size: 0, needs_admin: true });
    }
}

// ------------------------------------------------------------------ removal

pub fn remove(items: &[Leftover], label: &str, status: impl Fn(&str)) -> Removal {
    let mut r = Removal::default();
    for item in items {
        status(&item.target);
        if remove_one(item, label, &mut r) {
            r.removed += 1;
        } else {
            r.failed += 1;
        }
    }
    r
}

/// (root, view flag, subkey) of "HKEY_…\…".
fn split_key(full: &str) -> (winreg::HKEY, u32, String) {
    let hive = if full.to_uppercase().starts_with("HKEY_CURRENT_USER") { HKEY_CURRENT_USER } else { HKEY_LOCAL_MACHINE };
    let sub = full.split_once('\\').map(|x| x.1).unwrap_or_default().to_string();
    let wow = r"SOFTWARE\WOW6432Node\";
    if sub.to_uppercase().starts_with(&wow.to_uppercase()) {
        return (hive, KEY_WOW64_32KEY, format!(r"SOFTWARE\{}", &sub[wow.len()..]));
    }
    (hive, KEY_WOW64_64KEY, sub)
}

fn remove_one(item: &Leftover, label: &str, r: &mut Removal) -> bool {
    match item.kind {
        Kind::Folder | Kind::Shortcut => {
            if settings::get().is_excluded(&item.target) || !recycle::move_to_bin(Path::new(&item.target)) {
                return false;
            }
            r.recycled.push(item.target.clone());
            r.bytes += item.size;
            true
        }
        Kind::RegistryKey => {
            // never delete what could not be backed up
            let Some(backup) = uninstall::backup_key(&item.target, &format!("{label}_{}", item.title)) else { return false };
            let (hive, view, sub) = split_key(&item.target);
            let Some((parent, leaf)) = sub.rsplit_once('\\') else { return false };
            let Ok(p) = RegKey::predef(hive).open_subkey_with_flags(parent, KEY_ALL_ACCESS | view) else { return false };
            if p.delete_subkey_all(leaf).is_err() {
                return false;
            }
            r.reg_backups.push(backup.to_string_lossy().to_string());
            true
        }
        Kind::StartupEntry => {
            let backup = uninstall::backup_key(&item.target, &format!("{label}_Run"));
            let (hive, view, sub) = split_key(&item.target);
            let Ok(k) = RegKey::predef(hive).open_subkey_with_flags(&sub, KEY_ALL_ACCESS | view) else { return false };
            if k.delete_value(item.value_name.as_deref().unwrap_or_default()).is_err() {
                return false;
            }
            if let Some(b) = backup {
                r.reg_backups.push(b.to_string_lossy().to_string());
            }
            true
        }
        Kind::Task => {
            let Some(xml) = tasks::export_xml(&item.target) else { return false };
            let dir = crate::data::backups_dir();
            let _ = std::fs::create_dir_all(&dir);
            let safe: String = item.title.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
            let file = dir.join(format!("task_{safe}_{}.xml", chrono::Local::now().format("%Y%m%d_%H%M%S")));
            if std::fs::write(&file, xml).is_err() {
                return false;
            }
            tasks::delete(&item.target);
            if tasks::exists(&item.target) {
                return false;
            }
            r.task_backups.push(format!("{}|{}", item.target, file.display()));
            true
        }
        Kind::Service => {
            let backup = uninstall::backup_key(&format!(r"HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Services\{}", item.target), &format!("{label}_service"));
            let sc = cmd::system_tool("sc.exe");
            let _ = cmd::run(&sc, &["stop", &item.target], Encoding::Oem, Duration::from_secs(20));
            if !cmd::run(&sc, &["delete", &item.target], Encoding::Oem, Duration::from_secs(20)).is_ok_and(|x| x.ok()) {
                return false;
            }
            if let Some(b) = backup {
                r.reg_backups.push(b.to_string_lossy().to_string());
            }
            true
        }
        Kind::FirewallRule => cmd::run_raw(
            cmd::system_tool("netsh.exe"),
            &format!("advfirewall firewall delete rule name=all program=\"{}\"", item.target),
            Encoding::Oem,
            Duration::from_secs(30),
        )
        .is_ok_and(|x| x.ok()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(clean_names("7-Zip 24.08 (x64)"), vec!["7-Zip".to_string()]);
        assert_eq!(clean_names("Mozilla Firefox (x64 ru)"), vec!["Mozilla Firefox".to_string(), "MozillaFirefox".to_string()]);
        assert_eq!(clean_names("Go"), Vec::<String>::new());
        assert_eq!(clean_publisher("Valve Corporation").as_deref(), Some("Valve"));
        assert_eq!(clean_publisher("Telegram FZ-LLC").as_deref(), Some("Telegram FZ"));
        assert_eq!(path_safe("Foo: Bar"), None);
        assert_eq!(raw_exe(r#""C:\a b\x.exe" -s"#).as_deref(), Some(r"C:\a b\x.exe"));
        assert_eq!(raw_exe(r"C:\a\x.exe /run").as_deref(), Some(r"C:\a\x.exe"));
    }

    #[test]
    fn finds_and_removes_test_leftovers() {
        // a fake program under HKCU\Software and %LOCALAPPDATA%
        let name = format!("ProstorLeftoverTest{}", std::process::id());
        let folder = shell::local_app_data().join(&name);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("data.bin"), vec![0u8; 1234]).unwrap();
        let (k, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(format!(r"Software\{name}")).unwrap();
        k.set_value("x", &1u32).unwrap();
        drop(k);

        let app = App { id: "test".into(), name: name.clone(), ..Default::default() };
        let found = find(&app, &[], true);
        let f = found.iter().find(|l| l.kind == Kind::Folder).expect("folder found");
        assert_eq!(f.size, 1234);
        let key = found.iter().find(|l| l.kind == Kind::RegistryKey).expect("key found");
        assert!(!key.needs_admin);

        let r = remove(&[f.clone(), key.clone()], &name, |_| {});
        assert_eq!(r.removed, 2, "{r:?}");
        assert!(!folder.exists());
        assert!(RegKey::predef(HKEY_CURRENT_USER).open_subkey(format!(r"Software\{name}")).is_err());
        // undo the registry part and clean up
        uninstall::import_reg(&r.reg_backups).unwrap();
        RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(format!(r"Software\{name}")).unwrap();
        for b in &r.reg_backups {
            let _ = std::fs::remove_file(b);
        }
    }
}
