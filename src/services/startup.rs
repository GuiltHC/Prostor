//! Autostart entries: Run keys and Startup folders (toggled the way Task Manager does), logon/boot
//! tasks of Task Scheduler and third-party services (port of Services/StartupService.cs).
//!
//! Fixed compared to the C# version:
//! * two programs whose names differ only in symbols got the same delayed-start task and overwrote
//!   each other — task names now carry a short hash of the entry;
//! * a delayed-start task whose program entry was removed stayed hidden forever — it is now listed
//!   as an ordinary task, so it can be switched off;
//! * shortcuts are read by a native .lnk parser instead of one COM call per file.

use super::tasks;
use crate::data::settings;
use crate::sys::cmd::{self, Encoding};
use crate::sys::{cmdline, fileinfo, shell};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use winreg::RegKey;
use winreg::enums::*;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_ROOT: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\";
pub const DELAYED_FOLDER: &str = r"\Prostor\Delayed";
const KEY_MARKER: &str = "Ключ: ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Run,
    Folder,
    Task,
    Service,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
}

impl Hive {
    fn name(self) -> &'static str {
        match self {
            Hive::CurrentUser => "CurrentUser",
            Hive::LocalMachine => "LocalMachine",
        }
    }
    fn parse(s: &str) -> Option<Hive> {
        match s {
            "CurrentUser" => Some(Hive::CurrentUser),
            "LocalMachine" => Some(Hive::LocalMachine),
            _ => None,
        }
    }
    fn root(self) -> RegKey {
        RegKey::predef(match self {
            Hive::CurrentUser => HKEY_CURRENT_USER,
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub kind: Kind,
    pub name: String,
    pub command: String,
    pub arguments: Option<String>,
    pub file_path: Option<PathBuf>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub source: &'static str,
    // Run / Folder
    pub approved_hive: Hive,
    /// "Run", "Run32" or "StartupFolder".
    pub approved_key: &'static str,
    pub approved_value: String,
    // task
    pub task_path: Option<String>,
    pub task_owned_by_user: bool,
    // service
    pub service_name: Option<String>,
    pub service_delayed: bool,
    pub enabled: bool,
    /// Set when the program starts from a Простор task a minute after logon instead.
    pub delay_task: Option<String>,
}

impl Item {
    /// Stable identity used by the journal.
    pub fn key(&self) -> String {
        match self.kind {
            Kind::Task => format!("task|{}", self.task_path.as_deref().unwrap_or_default()),
            Kind::Service => format!("service|{}", self.service_name.as_deref().unwrap_or_default()),
            _ => format!("{}|{}|{}", self.approved_hive.name(), self.approved_key, self.approved_value),
        }
    }

    pub fn group(&self) -> &'static str {
        match self.kind {
            Kind::Task => "Задачи планировщика",
            Kind::Service => "Службы",
            _ => "Программы",
        }
    }

    pub fn can_modify(&self) -> bool {
        match self.kind {
            Kind::Service => shell::is_admin(),
            Kind::Task => self.task_owned_by_user || shell::is_admin(),
            _ => self.approved_hive == Hive::CurrentUser || shell::is_admin(),
        }
    }

    pub fn lock_hint(&self) -> Option<&'static str> {
        if self.can_modify() {
            return None;
        }
        Some(match self.kind {
            Kind::Service => "Изменять службы можно только в режиме администратора",
            Kind::Task => "Задача создана для всех пользователей — нужны права администратора",
            _ => "Запись для всех пользователей — нужны права администратора",
        })
    }

    pub fn can_delay(&self) -> bool {
        matches!(self.kind, Kind::Run | Kind::Folder) && self.file_path.is_some()
    }

    pub fn is_delayed(&self) -> bool {
        self.delay_task.is_some()
    }

    pub fn title(&self) -> String {
        match &self.description {
            Some(d) if !d.trim().is_empty() => d.clone(),
            _ => self.name.clone(),
        }
    }

    pub fn subtitle(&self) -> String {
        let mut parts = Vec::new();
        if self.description.as_ref().is_some_and(|d| !d.trim().is_empty() && !d.eq_ignore_ascii_case(&self.name)) {
            parts.push(self.name.clone());
        }
        if let Some(p) = self.publisher.as_ref().filter(|p| !p.trim().is_empty()) {
            parts.push(p.clone());
        }
        parts.join("  ·  ")
    }

    pub fn state_text(&self) -> &'static str {
        if !self.enabled {
            "Выкл."
        } else if self.is_delayed() {
            "Через 1 мин"
        } else if self.kind == Kind::Service && self.service_delayed {
            "Отложенно"
        } else {
            "Вкл."
        }
    }

    fn base(kind: Kind, name: String, command: String, source: &'static str) -> Self {
        Item {
            kind,
            name,
            command,
            arguments: None,
            file_path: None,
            publisher: None,
            description: None,
            source,
            approved_hive: Hive::CurrentUser,
            approved_key: "Run",
            approved_value: String::new(),
            task_path: None,
            task_owned_by_user: false,
            service_name: None,
            service_delayed: false,
            enabled: true,
            delay_task: None,
        }
    }
}

fn version(path: Option<&Path>) -> (Option<String>, Option<String>) {
    match path.filter(|p| p.is_file()) {
        Some(p) => {
            let v = fileinfo::version_info(p);
            (v.company, v.description)
        }
        None => (None, None),
    }
}

pub fn load() -> Vec<Item> {
    let mut list = Vec::new();
    read_run(&mut list, Hive::CurrentUser, false, "Run", "Реестр · текущий пользователь");
    read_run(&mut list, Hive::LocalMachine, false, "Run", "Реестр · все пользователи");
    read_run(&mut list, Hive::LocalMachine, true, "Run32", "Реестр · все пользователи (32-бит)");
    if let Some(f) = fileinfo::startup_folder(false) {
        read_folder(&mut list, &f, Hive::CurrentUser, "Папка автозагрузки");
    }
    if let Some(f) = fileinfo::startup_folder(true) {
        read_folder(&mut list, &f, Hive::LocalMachine, "Папка автозагрузки · все");
    }
    let all_tasks = list_tasks();
    let orphans = attach_delayed(&mut list, &all_tasks);
    read_tasks(&mut list, &all_tasks, &orphans);
    read_services(&mut list);

    let rank = |k: Kind| match k {
        Kind::Run | Kind::Folder => 0,
        Kind::Task => 1,
        Kind::Service => 2,
    };
    list.sort_by(|a, b| {
        rank(a.kind)
            .cmp(&rank(b.kind))
            .then(b.enabled.cmp(&a.enabled))
            .then_with(|| a.title().to_lowercase().cmp(&b.title().to_lowercase()))
    });
    list
}

fn read_run(list: &mut Vec<Item>, hive: Hive, wow32: bool, approved: &'static str, source: &'static str) {
    let flags = KEY_READ | if wow32 { KEY_WOW64_32KEY } else { KEY_WOW64_64KEY };
    let Ok(k) = hive.root().open_subkey_with_flags(RUN_KEY, flags) else { return };
    for (name, value) in k.enum_values().flatten() {
        if name.is_empty() {
            continue;
        }
        let command = value.to_string();
        let command = command.trim().to_string();
        if command.is_empty() {
            continue;
        }
        let exe = cmdline::extract_executable(&command).map(PathBuf::from);
        let (company, desc) = version(exe.as_deref());
        let mut it = Item::base(Kind::Run, name.clone(), command, source);
        it.file_path = exe;
        it.publisher = company;
        it.description = desc;
        it.approved_hive = hive;
        it.approved_key = approved;
        it.approved_value = name.clone();
        it.enabled = read_approved(hive, approved, &name);
        list.push(it);
    }
}

fn read_folder(list: &mut Vec<Item>, folder: &Path, hive: Hive, source: &'static str) {
    let Ok(rd) = std::fs::read_dir(folder) else { return };
    for e in rd.flatten() {
        let path = e.path();
        if !path.is_file() {
            continue;
        }
        let file_name = e.file_name().to_string_lossy().to_string();
        if file_name.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        let (target, args) = if file_name.to_lowercase().ends_with(".lnk") {
            match fileinfo::read_shortcut(&path) {
                Some(s) => (s.target, s.arguments),
                None => (None, None),
            }
        } else {
            (Some(path.clone()), None)
        };
        let (company, desc) = version(target.as_deref());
        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let mut it = Item::base(
            Kind::Folder,
            stem,
            target.as_ref().unwrap_or(&path).to_string_lossy().to_string(),
            source,
        );
        it.arguments = args;
        it.file_path = Some(target.unwrap_or(path));
        it.publisher = company;
        it.description = desc;
        it.approved_hive = hive;
        it.approved_key = "StartupFolder";
        it.approved_value = file_name.clone();
        it.enabled = read_approved(hive, "StartupFolder", &file_name);
        list.push(it);
    }
}

// ------------------------------------------------------------------ scheduled tasks

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct TaskInfo {
    pub path: String,
    pub name: String,
    pub enabled: bool,
    pub command: Option<String>,
    pub arguments: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub logon: bool,
    pub boot: bool,
    pub user: Option<String>,
}

/// All tasks outside \Microsoft\, through PowerShell's ScheduledTasks module.
pub fn list_tasks() -> Vec<TaskInfo> {
    let script = r#"
$list = @(Get-ScheduledTask -ErrorAction SilentlyContinue | Where-Object { $_.TaskPath -notlike '\Microsoft\*' } | ForEach-Object {
  $a = @($_.Actions | Where-Object { $_.CimClass.CimClassName -eq 'MSFT_TaskExecAction' })[0]
  $tr = @($_.Triggers | ForEach-Object { $_.CimClass.CimClassName })
  [pscustomobject]@{
    Path = $_.TaskPath + $_.TaskName; Name = $_.TaskName; Enabled = ($_.State -ne 'Disabled')
    Command = $a.Execute; Arguments = $a.Arguments; Author = $_.Author; Description = $_.Description
    Logon = ($tr -contains 'MSFT_TaskLogonTrigger'); Boot = ($tr -contains 'MSFT_TaskBootTrigger'); User = $_.Principal.UserId
  }
})
ConvertTo-Json -InputObject $list -Compress -Depth 3
"#;
    let Ok(r) = cmd::powershell(script, Duration::from_secs(60)) else { return Vec::new() };
    let text = r.output.trim();
    let start = text.find('[').unwrap_or(0);
    serde_json::from_str(&text[start..]).unwrap_or_default()
}

fn resolve_exe(path: &str) -> Option<PathBuf> {
    let p = cmdline::expand_env(path.trim().trim_matches('"'));
    if Path::new(&p).is_file() {
        return Some(PathBuf::from(p));
    }
    if !p.to_lowercase().ends_with(".exe") && Path::new(&format!("{p}.exe")).is_file() {
        return Some(PathBuf::from(format!("{p}.exe")));
    }
    cmdline::extract_executable(&p).map(PathBuf::from)
}

/// Programs moved to a delayed start: their entry is off, a Простор task starts them after logon.
/// Returns the delayed tasks that no longer match any entry.
fn attach_delayed(list: &mut [Item], all: &[TaskInfo]) -> Vec<String> {
    let mut orphans = Vec::new();
    let prefix = format!("{DELAYED_FOLDER}\\").to_lowercase();
    for t in all.iter().filter(|t| t.path.to_lowercase().starts_with(&prefix)) {
        let key = t.description.as_deref().and_then(|d| d.rfind(KEY_MARKER).map(|i| d[i + KEY_MARKER.len()..].trim().to_string()));
        match key.and_then(|k| list.iter_mut().find(|i| i.key() == k)) {
            Some(item) => {
                if t.enabled {
                    item.delay_task = Some(t.path.clone());
                    item.enabled = true;
                }
            }
            None => orphans.push(t.path.clone()),
        }
    }
    orphans
}

fn read_tasks(list: &mut Vec<Item>, all: &[TaskInfo], orphans: &[String]) {
    let me = tasks::current_user().to_lowercase();
    let short = std::env::var("USERNAME").unwrap_or_default().to_lowercase();
    let win = shell::windows_dir().to_string_lossy().to_lowercase();
    for t in all {
        let Some(command) = t.command.as_ref().filter(|c| !c.trim().is_empty()) else { continue };
        if !(t.logon || t.boot) {
            continue;
        }
        let ours = t.path.to_lowercase().starts_with(r"\prostor\");
        if ours && !orphans.iter().any(|o| o == &t.path) {
            continue;
        }
        let exe = resolve_exe(command);
        let (company, desc) = version(exe.as_deref());
        if company.as_ref().is_some_and(|c| c.contains("Microsoft"))
            && exe.as_ref().is_some_and(|e| e.to_string_lossy().to_lowercase().starts_with(&win))
        {
            continue; // part of Windows
        }
        let own = t.user.as_ref().is_some_and(|u| {
            let u = u.to_lowercase();
            u == me || u == short || u.ends_with(&format!("\\{short}"))
        });
        let mut it = Item::base(
            Kind::Task,
            t.name.clone(),
            cmdline::expand_env(command.trim_matches('"')),
            if t.boot { "Планировщик · при включении" } else { "Планировщик · при входе" },
        );
        it.arguments = t.arguments.clone();
        it.file_path = exe.or_else(|| Some(PathBuf::from(command)));
        it.publisher = company.or_else(|| t.author.clone().filter(|a| !a.trim().is_empty() && !a.starts_with('$')));
        it.description = if ours { Some(format!("{} (отложенный запуск удалённой программы)", t.name)) } else { desc };
        it.task_path = Some(t.path.clone());
        it.task_owned_by_user = own || ours;
        it.enabled = t.enabled;
        list.push(it);
    }
}

// ------------------------------------------------------------------ services

fn read_services(list: &mut Vec<Item>) {
    let win = shell::windows_dir().to_string_lossy().to_lowercase();
    let ours: Vec<String> = settings::get().manual_services.iter().map(|s| s.to_lowercase()).collect();
    let Ok(root) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"SYSTEM\CurrentControlSet\Services") else { return };
    for name in root.enum_keys().flatten() {
        let Ok(k) = root.open_subkey(&name) else { continue };
        let ty: u32 = k.get_value("Type").unwrap_or(0);
        if ty & 0x30 == 0 {
            continue; // drivers
        }
        let start: u32 = k.get_value("Start").unwrap_or(3);
        if start != 2 && !(start == 3 && ours.contains(&name.to_lowercase())) {
            continue;
        }
        let Ok(image) = k.get_value::<String, _>("ImagePath") else { continue };
        if image.trim().is_empty() {
            continue;
        }
        let expanded = cmdline::expand_env(&image);
        let Some(exe) = cmdline::extract_executable(&expanded).map(PathBuf::from).or_else(|| resolve_exe(&expanded)) else {
            continue;
        };
        if exe.to_string_lossy().to_lowercase().starts_with(&win) {
            continue; // Windows' own (svchost and friends)
        }
        let (company, _) = version(Some(&exe));
        if company.as_ref().is_some_and(|c| c.starts_with("Microsoft")) && exe.to_string_lossy().contains("Windows Defender") {
            continue;
        }
        let display = k
            .get_value::<String, _>("DisplayName")
            .ok()
            .and_then(|d| fileinfo::indirect_string(&d))
            .unwrap_or_else(|| name.clone());
        let delayed: u32 = k.get_value("DelayedAutostart").unwrap_or(0);
        let mut it = Item::base(Kind::Service, display, image, "Служба");
        it.file_path = Some(exe);
        it.publisher = company;
        it.service_name = Some(name.clone());
        it.service_delayed = delayed == 1;
        it.enabled = start == 2;
        list.push(it);
    }
}

// ------------------------------------------------------------------ toggling

/// 02/06 = enabled, 03/07 = disabled.
fn read_approved(hive: Hive, sub: &str, value: &str) -> bool {
    let path = format!("{APPROVED_ROOT}{sub}");
    hive.root()
        .open_subkey_with_flags(&path, KEY_READ | KEY_WOW64_64KEY)
        .and_then(|k| k.get_raw_value(value))
        .map(|v| v.bytes.first().is_none_or(|b| b & 1 == 0))
        .unwrap_or(true)
}

fn set_approved(hive: Hive, sub: &str, value: &str, enabled: bool) -> Result<(), String> {
    let path = format!("{APPROVED_ROOT}{sub}");
    let (k, _) = hive.root().create_subkey_with_flags(&path, KEY_WRITE | KEY_WOW64_64KEY).map_err(|e| e.to_string())?;
    let mut data = vec![0u8; 12];
    data[0] = if enabled { 0x02 } else { 0x03 };
    if !enabled {
        let ft = filetime_now();
        data[4..12].copy_from_slice(&ft.to_le_bytes());
    }
    k.set_raw_value(value, &winreg::RegValue { vtype: REG_BINARY, bytes: data.into() }).map_err(|e| e.to_string())
}

fn filetime_now() -> u64 {
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() / 100).unwrap_or(0) as u64;
    unix + 116_444_736_000_000_000
}

pub fn set_enabled(item: &mut Item, enabled: bool) -> Result<(), String> {
    match item.kind {
        Kind::Task => tasks::set_enabled(item.task_path.as_deref().unwrap_or_default(), enabled),
        Kind::Service => {
            let name = item.service_name.clone().unwrap_or_default();
            let mode = if enabled { if item.service_delayed { "delayed-auto" } else { "auto" } } else { "demand" };
            let r = cmd::run(cmd::system_tool("sc.exe"), &["config", &name, "start=", mode], Encoding::Oem, Duration::from_secs(30))
                .map_err(|e| e.to_string())?;
            if !r.ok() {
                return Err(r.output.trim().to_string());
            }
            settings::update(|s| {
                s.manual_services.retain(|n| !n.eq_ignore_ascii_case(&name));
                if !enabled {
                    s.manual_services.push(name.clone());
                }
            });
            Ok(())
        }
        _ => {
            if !enabled
                && let Some(t) = item.delay_task.take() {
                    tasks::delete(&t);
                }
            set_approved(item.approved_hive, item.approved_key, &item.approved_value, enabled)
        }
    }?;
    item.enabled = enabled;
    Ok(())
}

/// Task name for a delayed start: readable name plus a short hash of the entry, so entries with
/// similar names never share a task.
fn delayed_task_path(item: &Item) -> String {
    let safe: String = item
        .name
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_.".contains(c) { c } else { '_' })
        .collect::<String>()
        .trim()
        .to_string();
    let safe = if safe.is_empty() { "item".to_string() } else { safe };
    let hash = item.key().bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    format!("{DELAYED_FOLDER}\\{safe} {:08x}", hash as u32)
}

/// Starts the program a minute after logon from a Простор task and switches its entry off.
/// Returns the task path (for the journal).
pub fn make_delayed(item: &mut Item, delay: &str) -> Result<String, String> {
    let (file, args) = if item.kind == Kind::Folder {
        (item.file_path.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(), item.arguments.clone().unwrap_or_default())
    } else {
        cmdline::split(&item.command)
    };
    if file.is_empty() {
        return Err("Не удалось определить команду запуска.".into());
    }
    let path = delayed_task_path(item);
    let xml = tasks::logon_task_xml(&format!("Простор: отложенный запуск «{}». {KEY_MARKER}{}", item.title(), item.key()), delay, &file, &args);
    tasks::register(&path, &xml)?;
    if let Err(e) = set_approved(item.approved_hive, item.approved_key, &item.approved_value, false) {
        tasks::delete(&path);
        return Err(e);
    }
    item.delay_task = Some(path.clone());
    item.enabled = true;
    Ok(path)
}

/// Back to the normal start: removes the task and switches the original entry on.
pub fn undo_delayed(key: &str, task_path: &str) -> Result<(), String> {
    tasks::delete(task_path);
    let parts: Vec<&str> = key.splitn(3, '|').collect();
    if parts.len() == 3
        && let Some(h) = Hive::parse(parts[0]) {
            return set_approved(h, parts[1], parts[2], true);
        }
    Ok(())
}

// ------------------------------------------------------------------ boot time

#[derive(Clone, Debug, PartialEq)]
pub struct BootRecord {
    pub time: chrono::DateTime<chrono::Local>,
    /// Until the desktop is usable, seconds.
    pub main_path_seconds: f64,
    /// Including programs that start after logon, seconds.
    pub total_seconds: f64,
    pub startup_apps: u32,
}

/// Boot durations recorded by Windows (the log needs administrator rights). None when unreadable.
pub fn load_boot_history(count: u32) -> Option<Vec<BootRecord>> {
    let q = format!(
        "qe Microsoft-Windows-Diagnostics-Performance/Operational \"/q:*[System[(EventID=100)]]\" /f:xml /c:{count} /rd:true"
    );
    let r = cmd::run_raw(cmd::system_tool("wevtutil.exe"), &q, Encoding::Utf8, Duration::from_secs(20)).ok()?;
    r.ok().then(|| parse_boot_events(&r.output))
}

pub fn parse_boot_events(xml: &str) -> Vec<BootRecord> {
    let mut list = Vec::new();
    if !xml.contains("<Event") {
        return list;
    }
    let wrapped = format!("<root>{}</root>", xml.trim_start_matches('\u{feff}'));
    let Ok(doc) = roxmltree::Document::parse(&wrapped) else { return list };
    for ev in doc.root_element().children().filter(|n| n.has_tag_name("Event")) {
        let data = |name: &str| {
            ev.descendants()
                .find(|n| n.has_tag_name("Data") && n.attribute("Name") == Some(name))
                .and_then(|n| n.text())
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let Some(time) = ev
            .descendants()
            .find(|n| n.has_tag_name("TimeCreated"))
            .and_then(|n| n.attribute("SystemTime"))
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        else {
            continue;
        };
        let main: f64 = data("MainPathBootTime").parse().unwrap_or(0.0);
        let total: f64 = data("BootTime").parse().unwrap_or(0.0);
        if main <= 0.0 && total <= 0.0 {
            continue;
        }
        list.push(BootRecord {
            time: time.with_timezone(&chrono::Local),
            main_path_seconds: main / 1000.0,
            total_seconds: total / 1000.0,
            startup_apps: data("BootNumStartupApps").parse().unwrap_or(0),
        });
    }
    list.sort_by_key(|b| b.time);
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_startup_items() {
        let items = load();
        for i in &items {
            assert!(!i.name.is_empty());
            if matches!(i.kind, Kind::Run | Kind::Folder) {
                assert!(!i.approved_value.is_empty());
            }
        }
        let mut keys: Vec<String> = items.iter().map(|i| i.key()).collect();
        keys.sort();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "keys are unique");
    }

    #[test]
    fn delayed_names_differ_for_similar_entries() {
        let mut a = Item::base(Kind::Run, "My:App".into(), "x".into(), "");
        a.approved_value = "My:App".into();
        let mut b = Item::base(Kind::Run, "My*App".into(), "x".into(), "");
        b.approved_value = "My*App".into();
        assert_ne!(delayed_task_path(&a), delayed_task_path(&b));
        assert!(delayed_task_path(&a).starts_with(r"\Prostor\Delayed\My_App "));
    }

    #[test]
    fn approved_roundtrip_in_test_key() {
        // a value under StartupApproved\Run that no program uses
        let name = format!("ProstorTest{}", std::process::id());
        set_approved(Hive::CurrentUser, "Run", &name, false).unwrap();
        assert!(!read_approved(Hive::CurrentUser, "Run", &name));
        set_approved(Hive::CurrentUser, "Run", &name, true).unwrap();
        assert!(read_approved(Hive::CurrentUser, "Run", &name));
        let k = Hive::CurrentUser.root().open_subkey_with_flags(format!("{APPROVED_ROOT}Run"), KEY_WRITE).unwrap();
        k.delete_value(&name).unwrap();
    }

    #[test]
    fn parses_boot_events() {
        let xml = r#"<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><TimeCreated SystemTime='2026-11-01T08:00:00.000000000Z'/></System><EventData><Data Name='BootTime'>45000</Data><Data Name='MainPathBootTime'>20000</Data><Data Name='BootNumStartupApps'>12</Data></EventData></Event>"#;
        let r = parse_boot_events(xml);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].main_path_seconds, 20.0);
        assert_eq!(r[0].total_seconds, 45.0);
        assert_eq!(r[0].startup_apps, 12);
    }
}
