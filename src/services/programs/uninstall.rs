//! Running uninstallers, removing Store packages and orphaned entries, restore points
//! (port of Services/Uninstaller.cs, RegistryCleaner and RestorePointService).

use super::inventory::{App, Hive, View, com_init};
use super::process;
use crate::data;
use crate::sys::cmd::{self, Encoding};
use crate::sys::{cmdline, shell, wide};
use std::path::{Path, PathBuf};
use std::time::Duration;
use winreg::RegKey;
use winreg::enums::*;

#[derive(Debug)]
pub enum StartError {
    /// The user declined the UAC prompt.
    Cancelled,
    /// The uninstaller file does not exist.
    Missing,
    Other(String),
}

/// What was started: a PID to follow, or nothing (the work went to a launcher such as Epic).
pub type Started = Option<u32>;

fn shell_execute(file: &str, params: &str, dir: Option<&Path>) -> Result<Started, StartError> {
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let (f, p) = (wide(file), wide(params));
    let d = dir.map(crate::sys::wide_path);
    let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.fMask = SEE_MASK_NOCLOSEPROCESS;
    sei.lpFile = f.as_ptr();
    sei.lpParameters = if params.is_empty() { std::ptr::null() } else { p.as_ptr() };
    sei.lpDirectory = d.as_ref().map(|d| d.as_ptr()).unwrap_or(std::ptr::null());
    sei.nShow = SW_SHOWNORMAL;
    process::allow_any_foreground();
    let ok = unsafe { ShellExecuteExW(&mut sei) };
    if ok == 0 {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        return Err(match err {
            1223 => StartError::Cancelled,
            2 | 3 => StartError::Missing,
            e => StartError::Other(std::io::Error::from_raw_os_error(e as i32).to_string()),
        });
    }
    if sei.hProcess.is_null() {
        return Ok(None);
    }
    let pid = unsafe { windows_sys::Win32::System::Threading::GetProcessId(sei.hProcess) };
    unsafe { windows_sys::Win32::Foundation::CloseHandle(sei.hProcess) };
    Ok(Some(pid))
}

/// "/I{GUID}" opens the MSI maintenance UI — switch it to removal.
fn msi_install_to_remove(cmd: &str) -> String {
    let re = regex::Regex::new(r"(?i)/I\s*\{").unwrap();
    re.replace_all(cmd, "/X{").into_owned()
}

/// Launches the program's own uninstaller. UAC is handled by the shell.
pub fn start(app: &App, quiet: bool) -> Result<Started, StartError> {
    if app.launcher_manifest.is_some() {
        // Epic has no uninstall command line: open the launcher's library, the user removes the game there
        shell_execute(app.uninstall_string.as_deref().unwrap_or_default(), "", None)?;
        return Ok(None);
    }
    let mut c = match (quiet, &app.quiet_uninstall_string) {
        (true, Some(q)) if !q.trim().is_empty() => q.clone(),
        _ => app.uninstall_string.clone().unwrap_or_default(),
    };
    if app.is_msi && (c.trim().is_empty() || c.to_lowercase().contains("msiexec")) {
        c = format!("MsiExec.exe /X{}{}", app.product_code.as_deref().unwrap_or_default(), if quiet { " /qb /norestart" } else { "" });
    } else if c.to_lowercase().contains("msiexec") {
        c = msi_install_to_remove(&c);
    }
    let (file, args) = cmdline::split(&c);
    if file.is_empty() {
        return Err(StartError::Missing);
    }
    let fp = Path::new(&file);
    if fp.is_absolute() {
        if !fp.exists() {
            return Err(StartError::Missing);
        }
    } else if !file.contains('\\') && !file.contains(':') && !file.contains("://") && cmdline::extract_executable(&file).is_none() {
        return Err(StartError::Missing);
    }
    let dir = fp.parent().filter(|d| d.is_dir());
    shell_execute(&file, &args, dir)
}

pub fn start_modify(app: &App) -> Result<Started, StartError> {
    let (file, args) = cmdline::split(app.modify_path.as_deref().unwrap_or_default());
    shell_execute(&file, &args, None)
}

/// True when the uninstaller referenced by the entry no longer exists on disk.
pub fn uninstaller_missing(app: &App) -> bool {
    if app.is_store || app.is_msi || app.is_launcher_uninstall() {
        return false;
    }
    match app.uninstall_string.as_deref() {
        Some(u) if !u.trim().is_empty() => cmdline::extract_executable(u).is_none(),
        _ => false,
    }
}

fn reg_open(hive: Hive, view: View, sub: &str) -> std::io::Result<RegKey> {
    let base = RegKey::predef(if hive == Hive::LocalMachine { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER });
    base.open_subkey_with_flags(sub, KEY_READ | if view == View::Wow32 { KEY_WOW64_32KEY } else { KEY_WOW64_64KEY })
}

pub fn still_installed(app: &App) -> bool {
    if app.is_store {
        return store_package_exists(app.package_full_name.as_deref().unwrap_or_default());
    }
    if let Some(m) = app.steam_manifest.as_ref().filter(|_| app.is_steam_game()) {
        return m.exists(); // Steam deletes the manifest once the game is gone
    }
    if let Some(m) = &app.launcher_manifest {
        return m.exists();
    }
    match (app.hive, app.view, &app.registry_subkey) {
        (Some(h), Some(v), Some(sub)) => match reg_open(h, v, sub) {
            Ok(_) => true,
            Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        },
        _ => false,
    }
}

fn store_package_exists(full_name: &str) -> bool {
    com_init();
    let Ok(pm) = windows::Management::Deployment::PackageManager::new() else { return true };
    pm.FindPackageByUserSecurityIdPackageFullName(&windows::core::HSTRING::new(), &windows::core::HSTRING::from(full_name)).is_ok()
}

/// Removes a Store package. Decided by the real package state (polled), not only by the completion
/// callback — some packages finish removing without the callback ever reaching the app.
/// `progress` gets 0..1. Returns Err(None) when cancelled.
pub fn remove_store_app(
    app: &App,
    progress: impl Fn(f32),
    cancelled: impl Fn() -> bool,
) -> Result<(), Option<String>> {
    use windows_future::AsyncStatus;
    com_init();
    let full = app.package_full_name.clone().unwrap_or_default();
    let pm = windows::Management::Deployment::PackageManager::new().map_err(|e| Some(e.message().to_string()))?;
    let op = pm.RemovePackageAsync(&windows::core::HSTRING::from(&full)).map_err(|e| Some(e.message().to_string()))?;
    let pct = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let p2 = pct.clone();
    let _ = op.SetProgress(&windows_future::AsyncOperationProgressHandler::new(
        move |_, p: windows::core::Ref<windows::Management::Deployment::DeploymentProgress>| {
            p2.store(p.percentage, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        },
    ));
    let start = std::time::Instant::now();
    loop {
        progress(pct.load(std::sync::atomic::Ordering::Relaxed) as f32 / 100.0);
        match op.Status() {
            Ok(AsyncStatus::Completed) => {
                if let Ok(r) = op.GetResults()
                    && r.ExtendedErrorCode().is_ok_and(|c| c.is_err()) && store_package_exists(&full) {
                        return Err(Some(r.ErrorText().map(|t| t.to_string()).unwrap_or_else(|_| "Ошибка удаления пакета".into())));
                    }
                return Ok(());
            }
            Ok(AsyncStatus::Error) => {
                if !store_package_exists(&full) {
                    return Ok(());
                }
                let msg = op.ErrorCode().map(|c| windows::core::Error::from_hresult(c).message().to_string()).unwrap_or_default();
                return Err(Some(if msg.is_empty() { "Ошибка удаления пакета".into() } else { msg }));
            }
            Ok(AsyncStatus::Canceled) => return Err(None),
            _ => {}
        }
        if start.elapsed() > Duration::from_secs(2) && !store_package_exists(&full) {
            progress(1.0);
            return Ok(());
        }
        if cancelled() {
            let _ = op.Cancel();
            return Err(None);
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

// ------------------------------------------------------------------ registry entries & backups

fn safe_file_name(label: &str) -> String {
    let s: String = label.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    s.chars().take(60).collect()
}

/// Exports a key ("HKEY_CURRENT_USER\…") to the Backups folder. The .reg file, or None.
pub fn backup_key(full_key: &str, label: &str) -> Option<PathBuf> {
    let dir = data::backups_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let file = dir.join(format!("{}_{}.reg", safe_file_name(label), chrono::Local::now().format("%Y%m%d_%H%M%S_%3f")));
    let r = cmd::run(cmd::system_tool("reg.exe"), &["export", full_key, &file.to_string_lossy(), "/y"], Encoding::Oem, Duration::from_secs(20)).ok()?;
    (r.ok() && file.exists()).then_some(file)
}

/// Deletes the uninstall entry. Returns the .reg backup (None when it could not be written).
pub fn remove_uninstall_entry(app: &App) -> Result<Option<PathBuf>, String> {
    let (Some(hive), Some(view), Some(sub), Some(full)) = (app.hive, app.view, app.registry_subkey.as_ref(), app.registry_full_path())
    else {
        return Err("У этой программы нет записи в реестре.".into());
    };
    let backup = backup_key(&full, &app.name);
    let base = RegKey::predef(if hive == Hive::LocalMachine { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER });
    let flags = KEY_ALL_ACCESS | if view == View::Wow32 { KEY_WOW64_32KEY } else { KEY_WOW64_64KEY };
    let (parent, leaf) = sub.rsplit_once('\\').ok_or("Неверный путь записи")?;
    let p = base.open_subkey_with_flags(parent, flags).map_err(|e| access_message(&e))?;
    p.delete_subkey_all(leaf).map_err(|e| access_message(&e))?;
    Ok(backup)
}

pub fn access_message(e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        "Эта запись относится ко всем пользователям компьютера. Перезапустите Простор от имени администратора и повторите.".into()
    } else {
        e.to_string()
    }
}

/// reg import of the backups (undo).
pub fn import_reg(files: &[String]) -> Result<(), String> {
    let mut ok = 0;
    for f in files.iter().filter(|f| Path::new(f).exists()) {
        if cmd::run(cmd::system_tool("reg.exe"), &["import", f], Encoding::Oem, Duration::from_secs(30)).is_ok_and(|r| r.ok()) {
            ok += 1;
        }
    }
    if ok == 0 {
        return Err(if shell::is_admin() {
            "Не удалось импортировать резервную копию реестра.".into()
        } else {
            "Не удалось импортировать резервную копию. Для записей «для всех пользователей» нужны права администратора.".into()
        });
    }
    if ok == files.len() { Ok(()) } else { Err(format!("Восстановлено {ok} из {}.", files.len())) }
}

// ------------------------------------------------------------------ restore point

pub enum RestorePoint {
    Created,
    /// Windows makes at most one point a day and quietly skipped this one.
    Recent(String),
    Failed(String),
}

fn latest_restore_point() -> Option<(u32, String)> {
    let r = cmd::powershell(
        "$p = Get-ComputerRestorePoint -ErrorAction SilentlyContinue | Sort-Object SequenceNumber | Select-Object -Last 1; \
         if ($p) { \"$($p.SequenceNumber)|$([Management.ManagementDateTimeConverter]::ToDateTime($p.CreationTime).ToString('d MMMM, HH:mm', [Globalization.CultureInfo]'ru-RU'))\" }",
        Duration::from_secs(60),
    )
    .ok()?;
    let line = r.output.lines().rev().find(|l| l.contains('|'))?.trim().to_string();
    let (seq, time) = line.split_once('|')?;
    Some((seq.parse().ok()?, time.to_string()))
}

pub fn create_restore_point(description: &str) -> RestorePoint {
    use windows_sys::Win32::System::Restore::{
        APPLICATION_UNINSTALL, BEGIN_SYSTEM_CHANGE, END_SYSTEM_CHANGE, RESTOREPOINTINFOW, SRSetRestorePointW, STATEMGRSTATUS,
    };
    if !shell::is_admin() {
        return RestorePoint::Failed("Нужен режим администратора.".into());
    }
    let before = latest_restore_point();
    let mut info: RESTOREPOINTINFOW = unsafe { std::mem::zeroed() };
    info.dwEventType = BEGIN_SYSTEM_CHANGE;
    info.dwRestorePtType = APPLICATION_UNINSTALL;
    for (i, u) in description.encode_utf16().take(250).enumerate() {
        info.szDescription[i] = u;
    }
    let mut status: STATEMGRSTATUS = unsafe { std::mem::zeroed() };
    if unsafe { SRSetRestorePointW(&info, &mut status) } == 0 {
        return RestorePoint::Failed(match status.nStatus {
            1058 => "Защита системы выключена — включите её в «Свойствах системы → Защита системы».".into(),
            112 => "На диске недостаточно места для точки восстановления.".into(),
            n => format!("Windows не создала точку восстановления (код {n})."),
        });
    }
    let mut end: RESTOREPOINTINFOW = unsafe { std::mem::zeroed() };
    end.dwEventType = END_SYSTEM_CHANGE;
    end.dwRestorePtType = APPLICATION_UNINSTALL;
    end.llSequenceNumber = status.llSequenceNumber;
    let mut s2: STATEMGRSTATUS = unsafe { std::mem::zeroed() };
    unsafe { SRSetRestorePointW(&end, &mut s2) };
    let after = latest_restore_point();
    match (before, after) {
        (Some((b, time)), a) if a.as_ref().is_none_or(|(s, _)| *s <= b) => RestorePoint::Recent(format!("Точка восстановления уже есть — от {time}")),
        _ => RestorePoint::Created,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msi_switch() {
        assert_eq!(msi_install_to_remove("MsiExec.exe /I{ABC}"), "MsiExec.exe /X{ABC}");
        assert_eq!(msi_install_to_remove("msiexec /i {ABC} /qn"), "msiexec /X{ABC} /qn");
    }

    #[test]
    fn missing_uninstaller_is_detected() {
        let a = App { uninstall_string: Some(r#""C:\Nope\uninstall.exe" /S"#.into()), ..Default::default() };
        assert!(uninstaller_missing(&a));
        let b = App { uninstall_string: Some(r"C:\Windows\System32\cmd.exe /c".into()), ..Default::default() };
        assert!(!uninstaller_missing(&b));
        assert!(matches!(start(&a, false), Err(StartError::Missing)));
    }

    #[test]
    fn backup_and_delete_a_test_entry() {
        let leaf = format!("ProstorTest{}", std::process::id());
        let sub = format!(r"{}\{leaf}", super::super::inventory::UNINSTALL_KEY);
        let (k, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(&sub).unwrap();
        k.set_value("DisplayName", &"Prostor Test Entry").unwrap();
        drop(k);
        let app = App {
            name: "Prostor Test Entry".into(),
            hive: Some(Hive::CurrentUser),
            view: Some(View::Wow64),
            registry_subkey: Some(sub.clone()),
            ..Default::default()
        };
        assert!(still_installed(&app));
        let backup = remove_uninstall_entry(&app).unwrap().expect("backup written");
        assert!(!still_installed(&app));
        import_reg(&[backup.to_string_lossy().to_string()]).unwrap();
        assert!(still_installed(&app));
        remove_uninstall_entry(&app).unwrap();
        let _ = std::fs::remove_file(backup);
    }
}
