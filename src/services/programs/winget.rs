//! Updates and export through the Windows Package Manager (port of Services/WingetService.cs).

use super::inventory::App;
use crate::sys::cmd::{self, CmdResult, Encoding};
use crate::sys::shell;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct Upgrade {
    pub name: String,
    pub id: String,
    pub version: String,
    pub available: String,
    pub source: String,
}

static EXE: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    let alias = shell::local_app_data().join(r"Microsoft\WindowsApps\winget.exe");
    if alias.exists() {
        return Some(alias);
    }
    let r = cmd::run(cmd::system_tool("where.exe"), &["winget"], Encoding::Utf8, Duration::from_secs(10)).ok()?;
    let first = r.output.lines().next()?.trim().to_string();
    (r.ok() && std::path::Path::new(&first).exists()).then(|| PathBuf::from(first))
});

pub fn exe() -> Option<&'static PathBuf> {
    EXE.as_ref()
}

pub fn available() -> bool {
    exe().is_some()
}

pub fn upgrades() -> Vec<Upgrade> {
    let Some(exe) = exe() else { return Vec::new() };
    match cmd::run(exe, &["upgrade", "--include-unknown", "--accept-source-agreements", "--disable-interactivity"], Encoding::Utf8, Duration::from_secs(180)) {
        Ok(r) => parse_table(&r.output),
        Err(_) => Vec::new(),
    }
}

/// winget's fixed-width table: column headers are localized, so positions come from the header line.
/// Positions are counted in characters (names may contain Cyrillic or CJK).
pub fn parse_table(output: &str) -> Vec<Upgrade> {
    let lines: Vec<Vec<char>> = output
        .replace("\r\n", "\n")
        .split('\n')
        .map(|l| l.rsplit('\r').next().unwrap_or(l).chars().collect())
        .collect();
    let mut list = Vec::new();
    let Some(sep) = lines.iter().position(|l| {
        let s: String = l.iter().collect();
        let t = s.trim();
        t.len() > 20 && t.chars().all(|c| c == '-')
    }) else {
        return list;
    };
    if sep == 0 {
        return list;
    }
    let header = &lines[sep - 1];
    let starts: Vec<usize> = (0..header.len()).filter(|&i| header[i] != ' ' && (i == 0 || header[i - 1] == ' ')).collect();
    if starts.len() < 4 {
        return list;
    }
    for l in &lines[sep + 1..] {
        if l.iter().all(|c| c.is_whitespace()) {
            break;
        }
        if l.len() < *starts.last().unwrap() {
            continue; // summary line ("45 upgrades available.")
        }
        let col = |c: usize| -> String {
            let a = starts[c];
            let b = if c + 1 < starts.len() { starts[c + 1] } else { l.len() };
            if a >= l.len() {
                return String::new();
            }
            l[a..b.min(l.len())].iter().collect::<String>().trim().to_string()
        };
        let (name, id, version, available) = (col(0), col(1), col(2), col(3));
        let source = if starts.len() > 4 { col(4) } else { String::new() };
        if id.is_empty() || available.is_empty() || id.contains(' ') {
            continue;
        }
        list.push(Upgrade { name, id, version, available, source });
    }
    list
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

/// Matches winget rows to installed programs by name (winget shortens long names with "…").
pub fn apply(apps: &mut [App], upgrades: &[Upgrade]) {
    for a in apps.iter_mut() {
        a.update_version = None;
        a.winget_id = None;
    }
    for u in upgrades {
        let truncated = u.name.ends_with('…');
        let un = norm(u.name.trim_end_matches('…'));
        let version_matches = |a: &App| {
            a.version.as_deref().is_none_or(|v| v.is_empty() || u.version.starts_with('<') || v.to_lowercase().starts_with(&u.version.trim_end_matches('…').to_lowercase()))
        };
        let candidates = |f: &dyn Fn(&App) -> bool| apps.iter().position(|a| !a.is_game() && a.update_version.is_none() && f(a));
        let ix = candidates(&|a| norm(&a.name) == un && version_matches(a))
            .or_else(|| candidates(&|a| norm(&a.name) == un))
            .or_else(|| if truncated && un.chars().count() >= 8 { candidates(&|a| norm(&a.name).starts_with(&un)) } else { None });
        if let Some(i) = ix {
            apps[i].update_version = Some(u.available.clone());
            apps[i].winget_id = Some(u.id.clone());
        }
    }
}

/// winget exit codes that mean "nothing to do" are not errors.
pub fn is_success(r: &CmdResult) -> bool {
    r.code == 0 || r.code as u32 == 0x8A15_002B || r.code as u32 == 0x8A15_0011
}

/// Installs the newest version. `progress` gets (percent 0..1 when printed, text line).
pub fn upgrade(id: &str, progress: impl Fn(Option<f32>, &str)) -> std::io::Result<CmdResult> {
    use std::os::windows::process::CommandExt;
    let exe = exe().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "winget не найден"))?;
    let mut child = Command::new(exe)
        .args(["upgrade", "--id", id, "--exact", "--silent", "--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"])
        .creation_flags(0x0800_0000)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut all = String::new();
    if let Some(out) = child.stdout.take() {
        // winget redraws progress with \r: split on both
        let mut reader = BufReader::new(out);
        let mut buf = Vec::new();
        while reader.read_until(b'\r', &mut buf)? > 0 {
            for line in String::from_utf8_lossy(&buf).split('\n') {
                let l = line.trim_matches(['\r', ' ']);
                if l.is_empty() || l.trim_matches(['-', '\\', '|', '/']).is_empty() {
                    continue;
                }
                let pct = l.find('%').and_then(|i| {
                    let digits: String = l[..i].trim_end().chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
                    digits.parse::<f32>().ok().map(|v| (v / 100.0).clamp(0.0, 1.0))
                });
                let text: String = l.chars().filter(|c| !"█▒░▓■".contains(*c)).collect();
                progress(pct, text.trim());
                all.push_str(l);
                all.push('\n');
            }
            buf.clear();
        }
    }
    let status = child.wait()?;
    Ok(CmdResult { code: status.code().unwrap_or(-1), output: all })
}

pub fn export(file: &std::path::Path) -> std::io::Result<CmdResult> {
    let exe = exe().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "winget не найден"))?;
    cmd::run(exe, &["export", "-o", &file.to_string_lossy(), "--accept-source-agreements", "--disable-interactivity"], Encoding::Utf8, Duration::from_secs(300))
}

/// CSV of the program list (Excel-friendly: UTF-8 with BOM, semicolons).
pub fn build_csv(apps: &[App]) -> String {
    let esc = |s: &str| if s.contains([';', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
    let mut out = String::from("\u{feff}Название;Издатель;Версия;Тип;Размер (байт);Установлена;Последний запуск;Папка\n");
    for a in apps {
        let row = [
            esc(&a.name),
            esc(a.publisher.as_deref().unwrap_or_default()),
            esc(a.version.as_deref().unwrap_or_default()),
            esc(&a.kind_text()),
            a.size.map(|s| s.to_string()).unwrap_or_default(),
            a.install_date.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default(),
            a.last_used.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default(),
            esc(&a.install_location.as_ref().map(|l| l.to_string_lossy().to_string()).unwrap_or_default()),
        ];
        out.push_str(&row.join(";"));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\r   - \r\
Имя                               ИД                         Версия        Доступно      Источник\r\n\
-------------------------------------------------------------------------------------------------\r\n\
Microsoft Edge                    Microsoft.Edge             130.0.1       131.0.2       winget\r\n\
Очень длинное название програ…    Some.Program               1.0           2.0           winget\r\n\
Telegram Desktop                  Telegram.TelegramDesktop   5.0           5.1           winget\r\n\
3 обновлений доступно.\r\n";

    #[test]
    fn parses_localized_table() {
        let u = parse_table(SAMPLE);
        assert_eq!(u.len(), 3, "{u:?}");
        assert_eq!(u[0].id, "Microsoft.Edge");
        assert_eq!(u[0].available, "131.0.2");
        assert_eq!(u[1].name, "Очень длинное название програ…");
        assert_eq!(u[2].source, "winget");
    }

    #[test]
    fn applies_to_apps() {
        let mut apps = vec![
            App { name: "Telegram Desktop".into(), version: Some("5.0".into()), ..Default::default() },
            App { name: "Очень длинное название программы".into(), ..Default::default() },
        ];
        apply(&mut apps, &parse_table(SAMPLE));
        assert_eq!(apps[0].update_version.as_deref(), Some("5.1"));
        assert_eq!(apps[1].winget_id.as_deref(), Some("Some.Program"));
    }

    #[test]
    fn csv_escapes() {
        let csv = build_csv(&[App { name: "A; B".into(), ..Default::default() }]);
        assert!(csv.contains("\"A; B\""));
    }
}
