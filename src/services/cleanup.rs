//! Known junk: temporary files and caches that applications re-create on demand
//! (port of Services/CleanupService.cs + Models/CleanupItem.cs).
//!
//! Fixed compared to the C# version:
//! * empty sub-folders are removed only when they are as old as the item's minimum age, so a folder an
//!   installer has just created in %TEMP% is left alone;
//! * cleaning can be cancelled between files.

use crate::data::settings;
use crate::services::deep;
use crate::sys::{self, recycle, shell};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

pub const GROUP_SYSTEM: &str = "Windows";
pub const GROUP_BROWSERS: &str = "Браузеры";
pub const GROUP_APPS: &str = "Приложения";
pub const GROUP_DEV: &str = "Для разработчиков";
pub const GROUP_DEEP: &str = "Глубокая очистка Windows";

pub const GROUPS: [&str; 5] = [GROUP_SYSTEM, GROUP_BROWSERS, GROUP_APPS, GROUP_DEV, GROUP_DEEP];

pub fn group_hint(group: &str) -> Option<&'static str> {
    match group {
        GROUP_DEV => Some("Кэши инструментов разработки. Они пересоздаются, но потребуют повторной загрузки."),
        GROUP_DEEP => Some(
            "Системная очистка встроенными средствами Windows. Нужны права администратора; у некоторых пунктов есть последствия — они указаны.",
        ),
        _ => None,
    }
}

const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
const DAY: Duration = Duration::from_secs(86_400);

#[derive(Clone, Debug)]
pub struct Target {
    pub path: PathBuf,
    /// File name mask ("thumbcache_*.db").
    pub pattern: &'static str,
    pub recursive: bool,
    /// Only files older than this are removed (protects files in use).
    pub min_age: Duration,
    /// The target is a single file.
    pub is_file: bool,
}

impl Target {
    fn dir(path: PathBuf) -> Self {
        Self { path, pattern: "*", recursive: true, min_age: Duration::ZERO, is_file: false }
    }
    fn age(mut self, d: Duration) -> Self {
        self.min_age = d;
        self
    }
    fn mask(mut self, pattern: &'static str) -> Self {
        self.pattern = pattern;
        self
    }
    fn flat(mut self) -> Self {
        self.recursive = false;
        self
    }
    fn file(path: PathBuf) -> Self {
        Self { is_file: true, ..Self::dir(path) }
    }
}

/// Items that are not plain folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Special {
    RecycleBin,
    WinSxS,
    Hibernation,
    RestorePoints,
    OldDrivers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Trash,
    Clock,
    Image,
    Download,
    Cloud,
    Bug,
    Monitor,
    Logs,
    Package,
    Moon,
    Restore,
    Chip,
    Browser,
    Chat,
    Send,
    Game,
    Music,
    Code,
    Users,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: &'static str,
    pub group: &'static str,
    pub title: String,
    pub description: &'static str,
    pub icon: Icon,
    pub targets: Vec<Target>,
    pub requires_admin: bool,
    /// Checked unless the user changed it.
    pub default_checked: bool,
    pub special: Option<Special>,
    /// Takes minutes rather than seconds.
    pub slow: bool,
    /// The size is an estimate.
    pub estimate: bool,
    /// A side effect the user should know about before cleaning.
    pub warning: Option<&'static str>,
    /// Word forms for the count instead of "N файлов".
    pub count_forms: Option<[&'static str; 3]>,
}

impl Item {
    pub fn locked(&self) -> bool {
        self.requires_admin && !shell::is_admin()
    }

    /// Never run unattended: items with side effects or that take minutes.
    pub fn safe_for_auto_clean(&self) -> bool {
        self.warning.is_none() && !self.slow && self.special != Some(Special::RecycleBin) && self.group != GROUP_DEEP
    }

    pub fn count_text(&self, n: u64) -> String {
        if n == 0 && self.special != Some(Special::RecycleBin) {
            return String::new();
        }
        if self.special == Some(Special::RecycleBin) {
            return crate::fmt::count(n, "объект", "объекта", "объектов");
        }
        match self.count_forms {
            Some([a, b, c]) => crate::fmt::count(n, a, b, c),
            None => crate::fmt::files(n),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    #[default]
    Unmeasured,
    /// Known only after cleaning (DISM).
    Unknown,
    Bytes(u64),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Measure {
    pub size: Size,
    pub count: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CleanResult {
    pub freed: u64,
    pub deleted: u64,
    pub failed: u64,
}

// ------------------------------------------------------------------ building the list

fn local() -> PathBuf {
    shell::local_app_data()
}
fn roaming() -> PathBuf {
    shell::roaming_app_data()
}

#[allow(clippy::too_many_arguments)]
fn folder(
    id: &'static str,
    group: &'static str,
    icon: Icon,
    title: &str,
    description: &'static str,
    check: bool,
    admin: bool,
    targets: Vec<Target>,
) -> Item {
    Item {
        id,
        group,
        title: title.into(),
        description,
        icon,
        targets: targets.into_iter().filter(exists).collect(),
        requires_admin: admin,
        default_checked: check && (!admin || shell::is_admin()),
        special: None,
        slow: false,
        estimate: false,
        warning: None,
        count_forms: None,
    }
}

fn special(id: &'static str, icon: Icon, title: &str, description: &'static str, special: Special) -> Item {
    Item {
        id,
        group: GROUP_DEEP,
        title: title.into(),
        description,
        icon,
        targets: Vec::new(),
        requires_admin: true,
        default_checked: false,
        special: Some(special),
        slow: false,
        estimate: false,
        warning: None,
        count_forms: None,
    }
}

/// %TEMP%, accepted only when it really is a dedicated temp folder.
fn user_temp() -> PathBuf {
    let t = shell::temp();
    let name = t.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name == "temp" || name == "tmp" { t } else { local().join("Temp") }
}

const ELECTRON_CACHES: [&str; 7] =
    ["Cache", "Code Cache", "GPUCache", "DawnCache", "DawnGraphiteCache", "DawnWebGPUCache", r"Service Worker\CacheStorage"];
const CHROMIUM_PROFILE_CACHES: [&str; 7] =
    ["Cache", "Code Cache", "GPUCache", "DawnCache", "DawnGraphiteCache", "DawnWebGPUCache", "Media Cache"];
const CHROMIUM_ROOT_CACHES: [&str; 4] = ["GrShaderCache", "ShaderCache", "GraphiteDawnCache", r"Crashpad\reports"];

fn electron(root: PathBuf) -> Vec<Target> {
    ELECTRON_CACHES.iter().map(|c| Target::dir(root.join(c))).collect()
}

fn chromium(items: &mut Vec<Item>, id: &'static str, name: &str, user_data_dirs: &[PathBuf]) {
    let mut targets = Vec::new();
    for ud in user_data_dirs.iter().filter(|d| d.is_dir()) {
        targets.extend(CHROMIUM_ROOT_CACHES.iter().map(|c| Target::dir(ud.join(c))));
        let mut profiles = vec![ud.clone()];
        if let Ok(rd) = std::fs::read_dir(ud) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if e.file_type().is_ok_and(|t| t.is_dir())
                    && (n == "Default" || n.starts_with("Profile ") || n == "Guest Profile" || n == "System Profile")
                {
                    profiles.push(e.path());
                }
            }
        }
        for p in &profiles {
            targets.extend(CHROMIUM_PROFILE_CACHES.iter().map(|c| Target::dir(p.join(c))));
        }
    }
    if targets.is_empty() {
        return;
    }
    items.push(folder(
        id,
        GROUP_BROWSERS,
        Icon::Browser,
        &format!("{name} — кэш"),
        "Кэш страниц, скриптов и шейдеров. Пароли, история, закладки и вкладки не затрагиваются.",
        true,
        false,
        targets,
    ));
}

fn firefox(items: &mut Vec<Item>) {
    let profiles = local().join(r"Mozilla\Firefox\Profiles");
    let Ok(rd) = std::fs::read_dir(&profiles) else { return };
    let mut targets = Vec::new();
    for e in rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) {
        for sub in ["cache2", "startupCache", "thumbnails", "jumpListCache"] {
            targets.push(Target::dir(e.path().join(sub)));
        }
    }
    if !targets.is_empty() {
        items.push(folder(
            "firefox",
            GROUP_BROWSERS,
            Icon::Browser,
            "Mozilla Firefox — кэш",
            "Кэш страниц и миниатюр. Пароли, история и закладки не затрагиваются.",
            true,
            false,
            targets,
        ));
    }
}

pub fn build_items() -> Vec<Item> {
    let (local, roaming, win, pd, home) = (local(), roaming(), shell::windows_dir(), shell::program_data(), shell::home());
    let mut items = Vec::new();

    // ---------------- Windows
    items.push(Item {
        id: "recycle",
        group: GROUP_SYSTEM,
        title: "Корзина".into(),
        description: "Файлы, которые вы уже удалили. После очистки их нельзя будет восстановить.",
        icon: Icon::Trash,
        targets: Vec::new(),
        requires_admin: false,
        default_checked: true,
        special: Some(Special::RecycleBin),
        slow: false,
        estimate: false,
        warning: None,
        count_forms: None,
    });
    items.push(folder(
        "temp-user",
        GROUP_SYSTEM,
        Icon::Clock,
        "Временные файлы",
        "Папка TEMP вашей учётной записи. Удаляются только файлы старше суток.",
        true,
        false,
        vec![Target::dir(user_temp()).age(DAY)],
    ));
    items.push(folder(
        "temp-win",
        GROUP_SYSTEM,
        Icon::Clock,
        "Временные файлы Windows",
        "Системная папка Windows\\Temp. Удаляются только файлы старше суток.",
        true,
        true,
        vec![Target::dir(win.join("Temp")).age(DAY)],
    ));
    let explorer = local.join(r"Microsoft\Windows\Explorer");
    items.push(folder(
        "thumbs",
        GROUP_SYSTEM,
        Icon::Image,
        "Кэш эскизов",
        "Миниатюры картинок и видео в Проводнике. Windows создаст их заново.",
        true,
        false,
        vec![
            Target::dir(explorer.clone()).mask("thumbcache_*.db").flat(),
            Target::dir(explorer).mask("iconcache_*.db").flat(),
        ],
    ));
    items.push(folder(
        "wu",
        GROUP_SYSTEM,
        Icon::Download,
        "Загрузки обновлений Windows",
        "Уже установленные пакеты обновлений из SoftwareDistribution\\Download.",
        true,
        true,
        vec![Target::dir(win.join(r"SoftwareDistribution\Download")).age(DAY * 3)],
    ));
    items.push(folder(
        "do",
        GROUP_SYSTEM,
        Icon::Cloud,
        "Кэш оптимизации доставки",
        "Фрагменты обновлений, которыми Windows делится с другими компьютерами.",
        true,
        true,
        vec![Target::dir(
            win.join(r"ServiceProfiles\NetworkService\AppData\Local\Microsoft\Windows\DeliveryOptimization\Cache"),
        )],
    ));
    items.push(folder(
        "wer",
        GROUP_SYSTEM,
        Icon::Bug,
        "Отчёты об ошибках",
        "Отчёты Windows об ошибках программ, которые уже не нужны.",
        true,
        false,
        vec![
            Target::dir(local.join(r"Microsoft\Windows\WER")),
            Target::dir(pd.join(r"Microsoft\Windows\WER\ReportArchive")),
            Target::dir(pd.join(r"Microsoft\Windows\WER\ReportQueue")),
        ],
    ));
    items.push(folder(
        "dumps",
        GROUP_SYSTEM,
        Icon::Bug,
        "Дампы памяти",
        "Снимки памяти при сбоях программ и системы. Нужны только разработчикам для отладки.",
        true,
        false,
        vec![
            Target::dir(local.join("CrashDumps")),
            Target::dir(win.join("Minidump")),
            Target::file(win.join("MEMORY.DMP")),
            Target::dir(win.join("LiveKernelReports")).mask("*.dmp"),
        ],
    ));
    items.push(folder(
        "shaders",
        GROUP_SYSTEM,
        Icon::Monitor,
        "Кэш шейдеров видеокарты",
        "Кэш DirectX и драйверов NVIDIA / AMD / Intel. Игры пересоберут его сами, но первые запуски будут дольше.",
        false,
        false,
        [
            "D3DSCache",
            r"NVIDIA\DXCache",
            r"NVIDIA\GLCache",
            r"NVIDIA Corporation\NV_Cache",
            r"AMD\DxCache",
            r"AMD\DxcCache",
            r"AMD\GLCache",
            r"AMD\VkCache",
            r"Intel\ShaderCache",
        ]
        .iter()
        .map(|p| Target::dir(local.join(p)))
        .collect(),
    ));

    // ---------------- deep system cleanup (administrator)
    let root = PathBuf::from(sys::drives::system_root());
    items.push(folder(
        "gpu-installers",
        GROUP_DEEP,
        Icon::Chip,
        "Установщики драйверов видеокарты",
        "Распакованные и скачанные установщики NVIDIA и AMD. Драйверы уже установлены — эти копии не нужны.",
        true,
        true,
        vec![
            Target::dir(root.join("NVIDIA")),
            Target::dir(root.join("AMD")),
            Target::dir(pd.join(r"NVIDIA Corporation\Downloader")),
            Target::dir(shell::program_files().join(r"NVIDIA Corporation\Installer2")),
        ],
    ));
    items.push(folder(
        "winlogs",
        GROUP_DEEP,
        Icon::Logs,
        "Журналы обслуживания Windows",
        "Логи установки обновлений (CBS, DISM). Нужны только для разбора ошибок.",
        true,
        true,
        vec![Target::dir(win.join(r"Logs\CBS")).age(DAY), Target::dir(win.join(r"Logs\DISM")).age(DAY)],
    ));
    items.push(folder(
        "patchcache",
        GROUP_DEEP,
        Icon::Package,
        "Кэш исправлений установщика",
        "Копии файлов, которые установщик Windows хранит для будущих исправлений программ ($PatchCache$).",
        false,
        true,
        vec![Target::dir(win.join(r"Installer\$PatchCache$"))],
    ));
    items.push(Item {
        slow: true,
        warning: Some("После очистки установленные обновления нельзя будет удалить."),
        ..special(
            "winsxs",
            Icon::Package,
            "Устаревшие компоненты Windows (WinSxS)",
            "Старые версии системных файлов, оставшиеся после обновлений. Удаляет сама Windows (DISM). Займёт несколько минут.",
            Special::WinSxS,
        )
    });
    if deep::hiberfil_size() > 0 {
        items.push(Item {
            warning: Some("Гибернация и быстрый запуск Windows будут выключены. Вернуть можно в «Журнале»."),
            ..special(
                "hibernation",
                Icon::Moon,
                "Файл гибернации",
                "hiberfil.sys занимает столько, сколько оперативной памяти. Нужен только для гибернации и «быстрого запуска».",
                Special::Hibernation,
            )
        });
    }
    items.push(Item {
        estimate: true,
        count_forms: Some(["старая точка", "старые точки", "старых точек"]),
        warning: Some("Откатить систему можно будет только к последней точке восстановления."),
        ..special("restorepoints", Icon::Restore, "Старые точки восстановления", "Все точки восстановления системы, кроме самой новой.", Special::RestorePoints)
    });
    items.push(Item {
        count_forms: Some(["пакет", "пакета", "пакетов"]),
        warning: Some("Откатить драйвер к прошлой версии будет нельзя."),
        ..special(
            "olddrivers",
            Icon::Chip,
            "Старые версии драйверов",
            "Предыдущие версии драйверов, которые Windows хранит после обновления. Драйверы, которые сейчас используются, не удаляются.",
            Special::OldDrivers,
        )
    });

    // ---------------- browsers
    chromium(&mut items, "chrome", "Google Chrome", &[local.join(r"Google\Chrome\User Data")]);
    chromium(&mut items, "edge", "Microsoft Edge", &[local.join(r"Microsoft\Edge\User Data")]);
    chromium(&mut items, "yandex", "Яндекс Браузер", &[local.join(r"Yandex\YandexBrowser\User Data")]);
    chromium(&mut items, "opera", "Opera", &[local.join(r"Opera Software\Opera Stable"), roaming.join(r"Opera Software\Opera Stable")]);
    chromium(
        &mut items,
        "operagx",
        "Opera GX",
        &[local.join(r"Opera Software\Opera GX Stable"), roaming.join(r"Opera Software\Opera GX Stable")],
    );
    chromium(&mut items, "brave", "Brave", &[local.join(r"BraveSoftware\Brave-Browser\User Data")]);
    chromium(&mut items, "vivaldi", "Vivaldi", &[local.join(r"Vivaldi\User Data")]);
    firefox(&mut items);

    // ---------------- applications
    items.push(folder(
        "discord",
        GROUP_APPS,
        Icon::Chat,
        "Discord — кэш",
        "Кэш картинок и интерфейса Discord. Сообщения и настройки не затрагиваются.",
        true,
        false,
        electron(roaming.join("discord")),
    ));
    items.push(folder(
        "telegram",
        GROUP_APPS,
        Icon::Send,
        "Telegram — медиакэш",
        "Скачанные фото и видео из чатов. Telegram загрузит их снова при просмотре.",
        false,
        false,
        ["user_data", "user_data#2", "user_data#3"]
            .iter()
            .map(|d| Target::dir(roaming.join(r"Telegram Desktop\tdata").join(d)))
            .collect(),
    ));
    items.push(folder(
        "steam",
        GROUP_APPS,
        Icon::Game,
        "Steam — веб-кэш",
        "Кэш встроенного браузера Steam. Игры и сохранения не затрагиваются.",
        true,
        false,
        vec![Target::dir(local.join(r"Steam\htmlcache"))],
    ));
    items.push(folder(
        "spotify",
        GROUP_APPS,
        Icon::Music,
        "Spotify — кэш",
        "Временно сохранённые треки. Скачанные для офлайна плейлисты не удаляются.",
        true,
        false,
        vec![Target::dir(local.join(r"Spotify\Data")), Target::dir(local.join(r"Spotify\Browser\Cache"))],
    ));
    let mut code = electron(roaming.join("Code"));
    code.push(Target::dir(roaming.join(r"Code\CachedExtensionVSIXs")));
    code.push(Target::dir(roaming.join(r"Code\CachedData")));
    items.push(folder("vscode", GROUP_APPS, Icon::Code, "Visual Studio Code — кэш", "Кэш интерфейса и загруженных расширений VS Code.", true, false, code));
    items.push(folder(
        "teams",
        GROUP_APPS,
        Icon::Users,
        "Microsoft Teams — кэш",
        "Кэш классической версии Teams.",
        true,
        false,
        electron(roaming.join(r"Microsoft\Teams")),
    ));

    // ---------------- developers (off by default)
    let dev = |id, title: &str, desc, targets| folder(id, GROUP_DEV, Icon::Package, title, desc, false, false, targets);
    items.push(dev("npm", "npm — кэш пакетов", "Скачанные пакеты Node.js. Будут загружены заново при установке.", vec![Target::dir(local.join("npm-cache"))]));
    items.push(dev("yarn", "Yarn — кэш пакетов", "Кэш менеджера пакетов Yarn.", vec![Target::dir(local.join(r"Yarn\Cache"))]));
    items.push(dev("pip", "pip — кэш пакетов", "Скачанные пакеты Python.", vec![Target::dir(local.join(r"pip\Cache"))]));
    items.push(dev(
        "nuget",
        "NuGet — HTTP-кэш",
        "Временный кэш загрузок NuGet (установленные пакеты не трогаем).",
        vec![Target::dir(local.join(r"NuGet\v3-cache")), Target::dir(local.join(r"NuGet\plugins-cache"))],
    ));
    items.push(dev("gradle", "Gradle — кэш", "Кэш сборок и зависимостей Gradle.", vec![Target::dir(home.join(r".gradle\caches"))]));
    items.push(dev("gobuild", "Go — кэш сборки", "Промежуточные файлы компилятора Go.", vec![Target::dir(local.join("go-build"))]));
    items.push(dev("cargo", "Cargo — кэш реестра", "Скачанные архивы крейтов Rust.", vec![Target::dir(home.join(r".cargo\registry\cache"))]));
    items.push(dev("uv", "uv — кэш пакетов", "Кэш менеджера пакетов Python uv.", vec![Target::dir(local.join(r"uv\cache"))]));

    items.retain(|i| i.special.is_some() || !i.targets.is_empty());
    items
}

fn exists(t: &Target) -> bool {
    is_safe(t) && if t.is_file { t.path.is_file() } else { t.path.is_dir() }
}

/// Guards against misconfigured environment variables pointing at a drive root or a user/system folder.
fn is_safe(t: &Target) -> bool {
    let s = t.path.to_string_lossy();
    let full = s.trim_end_matches('\\');
    if !t.path.is_absolute() || full.len() <= 3 {
        return false;
    }
    if t.is_file {
        return true;
    }
    let forbidden = [
        shell::windows_dir(),
        crate::sys::cmd::system_tool(""),
        shell::home(),
        shell::local_app_data(),
        shell::roaming_app_data(),
        shell::program_data(),
        shell::program_files(),
        shell::program_files_x86(),
        shell::home().join("Documents"),
        shell::home().join("Desktop"),
        shell::downloads(),
    ];
    !forbidden.iter().any(|f| {
        let f = f.to_string_lossy();
        let f = f.trim_end_matches('\\');
        !f.is_empty() && f.to_lowercase() == full.to_lowercase()
    })
}

/// "*" and "?" wildcards, case-insensitive.
pub fn matches(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

struct FileInfo {
    size: u64,
    readonly: bool,
}

/// Visits every file of a target that matches its mask and age. Links are never followed.
fn walk(t: &Target, cancel: &AtomicBool, mut on_file: impl FnMut(&Path, &FileInfo)) {
    let settings = settings::get().clone();
    if t.is_file {
        if let Ok(md) = std::fs::metadata(&t.path) {
            on_file(&t.path, &FileInfo { size: md.len(), readonly: md.file_attributes() & FILE_ATTRIBUTE_READONLY != 0 });
        }
        return;
    }
    if settings.is_excluded(&t.path.to_string_lossy()) {
        return;
    }
    let cutoff = (t.min_age > Duration::ZERO).then(|| SystemTime::now() - t.min_age);
    let mut stack = vec![t.path.clone()];
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            let attrs = md.file_attributes();
            if md.is_dir() || (attrs & 0x10 != 0) {
                if t.recursive && attrs & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
                    let sub = e.path();
                    if !settings.is_excluded(&sub.to_string_lossy()) {
                        stack.push(sub);
                    }
                }
                continue;
            }
            let name = e.file_name();
            if !matches(t.pattern, &name.to_string_lossy()) {
                continue;
            }
            if let Some(c) = cutoff
                && md.modified().is_ok_and(|m| m > c) {
                    continue;
                }
            let size = if attrs & FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS != 0 { 0 } else { md.len() };
            on_file(&e.path(), &FileInfo { size, readonly: attrs & FILE_ATTRIBUTE_READONLY != 0 });
        }
    }
}

pub fn measure(item: &Item, cancel: &AtomicBool) -> Measure {
    if let Some(s) = item.special {
        return match s {
            Special::RecycleBin => {
                let (size, count) = recycle::query();
                Measure { size: Size::Bytes(size), count }
            }
            Special::WinSxS => Measure { size: Size::Unknown, count: 0 },
            Special::Hibernation => Measure { size: Size::Bytes(deep::hiberfil_size()), count: 1 },
            Special::RestorePoints => deep::measure_shadows(),
            Special::OldDrivers => deep::measure_old_drivers(),
        };
    }
    let (mut size, mut count) = (0u64, 0u64);
    for t in &item.targets {
        walk(t, cancel, |_, f| {
            size += f.size;
            count += 1;
        });
    }
    Measure { size: Size::Bytes(size), count }
}

fn delete_file(path: &Path, f: &FileInfo) -> bool {
    if f.readonly
        && let Ok(md) = std::fs::metadata(path) {
            let mut p = md.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            p.set_readonly(false);
            let _ = std::fs::set_permissions(path, p);
        }
    std::fs::remove_file(path).is_ok()
}

pub fn clean(item: &Item, cancel: &AtomicBool) -> CleanResult {
    if let Some(s) = item.special {
        return match s {
            Special::RecycleBin => {
                let (size, count) = recycle::query();
                if recycle::empty() { CleanResult { freed: size, deleted: count, failed: 0 } } else { CleanResult { failed: 1, ..Default::default() } }
            }
            Special::WinSxS => deep::clean_winsxs(cancel),
            Special::Hibernation => deep::disable_hibernation(),
            Special::RestorePoints => deep::delete_old_shadows(),
            Special::OldDrivers => deep::delete_old_drivers(),
        };
    }
    let mut r = CleanResult::default();
    for t in &item.targets {
        walk(t, cancel, |path, f| {
            if delete_file(path, f) {
                r.freed += f.size;
                r.deleted += 1;
            } else {
                r.failed += 1;
            }
        });
        if !t.is_file && t.recursive && !cancel.load(Ordering::Relaxed) {
            remove_empty_dirs(&t.path, true, t.min_age);
        }
    }
    r
}

/// Removes sub-folders that became empty; the target itself is kept. Folders younger than
/// `min_age` stay — a running program may have just created them.
fn remove_empty_dirs(dir: &Path, is_root: bool, min_age: Duration) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if let Ok(md) = e.metadata()
            && md.is_dir() && md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
                remove_empty_dirs(&e.path(), false, min_age);
            }
    }
    if is_root {
        return;
    }
    let old_enough = min_age.is_zero()
        || std::fs::metadata(dir).and_then(|m| m.modified()).is_ok_and(|m| m.elapsed().is_ok_and(|age| age >= min_age));
    if old_enough && std::fs::read_dir(dir).is_ok_and(|mut r| r.next().is_none()) {
        let _ = std::fs::remove_dir(dir);
    }
}

/// Size of Windows.old left after a feature update (informational only).
pub fn windows_old_size() -> u64 {
    let p = PathBuf::from(sys::drives::system_root()).join("Windows.old");
    if p.is_dir() { folder_size(&p) } else { 0 }
}

/// Total size of a folder; links are not followed.
pub fn folder_size(dir: &Path) -> u64 {
    let never = AtomicBool::new(false);
    let mut total = 0;
    walk(&Target::dir(dir.to_path_buf()), &never, |_, f| total += f.size);
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard() {
        assert!(matches("thumbcache_*.db", "thumbcache_1024.db"));
        assert!(matches("thumbcache_*.db", "THUMBCACHE_idx.DB"));
        assert!(!matches("thumbcache_*.db", "iconcache_1.db"));
        assert!(matches("*.dmp", "a.dmp"));
        assert!(matches("a?c", "abc"));
        assert!(!matches("a?c", "abbc"));
    }

    #[test]
    fn unsafe_targets_are_dropped() {
        assert!(!is_safe(&Target::dir(PathBuf::from(r"C:\"))));
        assert!(!is_safe(&Target::dir(shell::home())));
        assert!(!is_safe(&Target::dir(shell::local_app_data())));
        assert!(!is_safe(&Target::dir(PathBuf::from("relative"))));
        assert!(is_safe(&Target::dir(shell::local_app_data().join("Temp"))));
    }

    #[test]
    fn builds_items() {
        let items = build_items();
        assert!(items.iter().any(|i| i.id == "recycle"));
        assert!(items.iter().all(|i| i.special.is_some() || !i.targets.is_empty()));
        let mut ids: Vec<_> = items.iter().map(|i| i.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), items.len(), "ids are unique");
    }

    #[test]
    fn cleans_old_files_and_keeps_new_folders() {
        let root = std::env::temp_dir().join(format!("prostor-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub").join("deep")).unwrap();
        std::fs::create_dir_all(root.join("fresh-empty")).unwrap();
        std::fs::write(root.join("a.tmp"), vec![0u8; 100]).unwrap();
        std::fs::write(root.join("sub").join("deep").join("b.tmp"), vec![0u8; 50]).unwrap();
        let item = Item { targets: vec![Target::dir(root.clone())], ..folder("t", GROUP_SYSTEM, Icon::Clock, "t", "", true, false, vec![]) };
        let never = AtomicBool::new(false);
        let m = measure(&item, &never);
        assert_eq!(m.size, Size::Bytes(150));
        assert_eq!(m.count, 2);
        let r = clean(&item, &never);
        assert_eq!(r.freed, 150);
        assert_eq!(r.deleted, 2);
        assert!(root.is_dir(), "the target itself stays");
        assert!(!root.join("sub").exists(), "emptied folders go");

        // with a minimum age, fresh files and fresh empty folders stay
        std::fs::create_dir_all(root.join("new-dir")).unwrap();
        std::fs::write(root.join("new.tmp"), b"x").unwrap();
        let aged = Item { targets: vec![Target::dir(root.clone()).age(DAY)], ..item };
        let r = clean(&aged, &never);
        assert_eq!(r.deleted, 0);
        assert!(root.join("new-dir").is_dir());
        assert!(root.join("new.tmp").is_file());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
