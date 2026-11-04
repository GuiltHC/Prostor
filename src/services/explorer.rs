//! "Анализировать в Простор" in the context menu of folders and drives, current user only
//! (port of Core/ExplorerIntegration.cs — the same registry keys, so the menu carries over).

use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

const VERB: &str = "Prostor.Analyze";
const TITLE: &str = "Анализировать в «Простор»";
const ROOTS: [&str; 3] = [r"Software\Classes\Directory\shell", r"Software\Classes\Directory\Background\shell", r"Software\Classes\Drive\shell"];

fn exe() -> String {
    std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
}

fn command() -> Option<String> {
    let k = RegKey::predef(HKEY_CURRENT_USER).open_subkey(format!(r"{}\{VERB}\command", ROOTS[0])).ok()?;
    k.get_value::<String, _>("").ok()
}

pub fn is_enabled() -> bool {
    command().is_some_and(|c| c.to_lowercase().contains("--scan"))
}

/// The menu points to a different copy of the program (moved or re-downloaded).
pub fn points_elsewhere() -> bool {
    command().is_some_and(|c| !c.to_lowercase().contains(&exe().to_lowercase()))
}

pub fn enable() -> Result<(), String> {
    let exe = exe();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for root in ROOTS {
        let (k, _) = hkcu.create_subkey(format!(r"{root}\{VERB}")).map_err(|e| e.to_string())?;
        k.set_value("", &TITLE).map_err(|e| e.to_string())?;
        k.set_value("Icon", &format!("\"{exe}\",0")).map_err(|e| e.to_string())?;
        let (c, _) = k.create_subkey("command").map_err(|e| e.to_string())?;
        c.set_value("", &format!("\"{exe}\" --scan \"%V\"")).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn disable() {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for root in ROOTS {
        let _ = hkcu.delete_subkey_all(format!(r"{root}\{VERB}"));
    }
}
