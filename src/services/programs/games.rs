//! Games: Steam libraries, Epic, GOG, EA, Ubisoft, Battle.net and Xbox — correct folders, sizes and
//! the way to uninstall them (port of Services/SteamLibrary.cs and Services/GameLaunchers.cs).

use super::inventory::App;
use super::vdf::{self, Value};
use crate::sys::shell;
use std::path::{Path, PathBuf};
use winreg::RegKey;
use winreg::enums::*;

pub const EPIC: &str = "Epic Games";
pub const GOG: &str = "GOG";
pub const EA: &str = "EA";
pub const UBISOFT: &str = "Ubisoft Connect";
pub const BATTLE_NET: &str = "Battle.net";
pub const XBOX: &str = "Xbox";

// ------------------------------------------------------------------ Steam

#[derive(Clone, Debug)]
pub struct SteamGame {
    pub app_id: String,
    pub name: String,
    pub install_dir: Option<String>,
    pub library: PathBuf,
    pub manifest: PathBuf,
    pub size_on_disk: u64,
    pub last_updated: Option<i64>,
}

impl SteamGame {
    pub fn game_path(&self) -> PathBuf {
        self.library.join(r"steamapps\common").join(self.install_dir.as_deref().unwrap_or_default())
    }
    /// Workshop content and the shader cache that belong to the game.
    pub fn extra_paths(&self) -> Vec<PathBuf> {
        [self.library.join(r"steamapps\workshop\content").join(&self.app_id), self.library.join(r"steamapps\shadercache").join(&self.app_id)]
            .into_iter()
            .filter(|p| p.is_dir())
            .collect()
    }
}

pub fn steam_path() -> Option<PathBuf> {
    let cu = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Valve\Steam").ok();
    if let Some(p) = cu.and_then(|k| k.get_value::<String, _>("SteamPath").ok()) {
        let p = PathBuf::from(p.replace('/', "\\"));
        if p.is_dir() {
            return Some(p);
        }
    }
    let lm = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(r"SOFTWARE\Valve\Steam", KEY_READ | KEY_WOW64_32KEY).ok()?;
    let p = PathBuf::from(lm.get_value::<String, _>("InstallPath").ok()?);
    p.is_dir().then_some(p)
}

/// Steamworks Common Redistributables is not a game.
const NOT_GAMES: [&str; 1] = ["228980"];

pub fn steam_games() -> Vec<SteamGame> {
    let mut result = Vec::new();
    let Some(steam) = steam_path() else { return result };
    let mut libraries = vec![steam.clone()];
    if let Ok(text) = std::fs::read_to_string(steam.join(r"steamapps\libraryfolders.vdf")) {
        let o = vdf::parse(&text);
        if let Some(folders) = vdf::get(&o, "libraryfolders").and_then(Value::as_obj) {
            for entry in folders.values().filter_map(Value::as_obj) {
                if let Some(p) = vdf::get(entry, "path").and_then(Value::as_str) {
                    let p = PathBuf::from(p);
                    if p.is_dir() {
                        libraries.push(p);
                    }
                }
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    for lib in libraries {
        if !seen.insert(lib.to_string_lossy().to_lowercase()) {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(lib.join("steamapps")) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            let o = vdf::parse(&text);
            let Some(state) = vdf::get(&o, "AppState").and_then(Value::as_obj) else { continue };
            let get = |k: &str| vdf::get(state, k).and_then(Value::as_str).map(str::to_string);
            let Some(id) = get("appid").filter(|i| !i.is_empty() && !NOT_GAMES.contains(&i.as_str())) else { continue };
            result.push(SteamGame {
                name: get("name").unwrap_or_else(|| format!("Steam App {id}")),
                install_dir: get("installdir"),
                library: lib.clone(),
                manifest: e.path(),
                size_on_disk: get("SizeOnDisk").and_then(|s| s.parse().ok()).unwrap_or(0),
                last_updated: get("LastUpdated").and_then(|s| s.parse().ok()).filter(|&t: &i64| t > 0),
                app_id: id,
            });
        }
    }
    result
}

fn link(app: &mut App, g: &SteamGame) {
    app.steam_app_id = Some(g.app_id.clone());
    app.steam_manifest = Some(g.manifest.clone());
    if g.game_path().is_dir() {
        app.install_location = Some(g.game_path());
    }
    app.extra_locations = g.extra_paths();
    if app.size.unwrap_or(0) == 0 && g.size_on_disk > 0 {
        app.size = Some(g.size_on_disk);
    }
}

/// Links "Steam App N" uninstall entries to their game folders (Steam does not register them) and
/// adds installed games that have no uninstall entry at all.
pub fn attach_steam(apps: &mut Vec<App>) {
    let games = steam_games();
    if games.is_empty() {
        return;
    }
    let mut matched = std::collections::HashSet::new();
    for app in apps.iter_mut() {
        let Some(key) = app.registry_subkey.as_ref() else { continue };
        let leaf = key.rsplit('\\').next().unwrap_or_default();
        let Some(id) = leaf.strip_prefix("Steam App ").map(|s| s.trim().to_string()) else { continue };
        if let Some(g) = games.iter().find(|g| g.app_id == id) {
            link(app, g);
            matched.insert(id);
        }
    }
    let Some(exe) = steam_path().map(|p| p.join("steam.exe")).filter(|p| p.is_file()) else { return };
    for g in games.iter().filter(|g| !matched.contains(&g.app_id) && g.game_path().is_dir()) {
        let mut app = App {
            id: format!("steam|{}", g.app_id),
            name: g.name.clone(),
            publisher: Some("Steam".into()),
            install_date: g.last_updated.and_then(|t| chrono::DateTime::from_timestamp(t, 0)).map(|d| d.with_timezone(&chrono::Local)),
            uninstall_string: Some(format!("\"{}\" steam://uninstall/{}", exe.display(), g.app_id)),
            ..Default::default()
        };
        link(&mut app, g);
        apps.push(app);
    }
}

// ------------------------------------------------------------------ Epic

#[derive(Clone, Debug, PartialEq)]
pub struct EpicGame {
    pub app_name: String,
    pub display_name: String,
    pub install_location: Option<String>,
    pub install_size: u64,
    pub manifest: PathBuf,
}

pub fn epic_manifests() -> PathBuf {
    shell::program_data().join(r"Epic\EpicGamesLauncher\Data\Manifests")
}

pub fn parse_epic_manifest(json: &str, file: &Path) -> Option<EpicGame> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let app = s("AppName").filter(|a| !a.is_empty())?;
    if v.get("bIsIncompleteInstall").and_then(|b| b.as_bool()) == Some(true) {
        return None;
    }
    if let Some(main) = s("MainGameAppName").filter(|m| !m.is_empty())
        && !main.eq_ignore_ascii_case(&app) {
            return None; // DLC
        }
    Some(EpicGame {
        display_name: s("DisplayName").unwrap_or_else(|| app.clone()),
        install_location: s("InstallLocation"),
        install_size: v.get("InstallSize").and_then(|x| x.as_u64()).unwrap_or(0),
        manifest: file.to_path_buf(),
        app_name: app,
    })
}

fn same_folder(a: Option<&PathBuf>, b: &str) -> bool {
    a.is_some_and(|a| a.to_string_lossy().trim_end_matches('\\').to_lowercase() == b.trim_end_matches(['\\', '/']).to_lowercase())
}

fn attach_epic(apps: &mut Vec<App>) {
    let Ok(rd) = std::fs::read_dir(epic_manifests()) else { return };
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().to_lowercase().ends_with(".item") {
            continue;
        }
        let Some(g) = std::fs::read_to_string(e.path()).ok().and_then(|t| parse_epic_manifest(&t, &e.path())) else { continue };
        let Some(loc) = g.install_location.as_ref().filter(|l| Path::new(l).is_dir()) else { continue };
        let loc = loc.trim_end_matches(['\\', '/']).to_string();
        // some games register an uninstall entry too: enrich it instead of duplicating
        if let Some(existing) = apps.iter_mut().find(|a| same_folder(a.install_location.as_ref(), &loc)) {
            existing.game_source = Some(EPIC.into());
            continue;
        }
        let mut app = App {
            id: format!("epic|{}", g.app_name),
            name: g.display_name.clone(),
            publisher: Some(EPIC.into()),
            install_location: Some(PathBuf::from(&loc)),
            uninstall_string: Some("com.epicgames.launcher://store/library".into()),
            install_date: std::fs::metadata(&g.manifest).and_then(|m| m.modified()).ok().map(chrono::DateTime::<chrono::Local>::from),
            game_source: Some(EPIC.into()),
            launcher_manifest: Some(g.manifest.clone()),
            size: (g.install_size > 0).then_some(g.install_size),
            ..Default::default()
        };
        app.set_launcher("Epic Games Launcher");
        apps.push(app);
    }
}

// ------------------------------------------------------------------ GOG / EA / Ubisoft / Battle.net

fn lm32(path: &str) -> Option<RegKey> {
    RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(path, KEY_READ | KEY_WOW64_32KEY).ok()
}

fn tag_by_registry(apps: &mut [App], root: &str, value: &str, source: &str) {
    let Some(root) = lm32(root) else { return };
    for id in root.enum_keys().flatten() {
        let Ok(k) = root.open_subkey(&id) else { continue };
        let Ok(path) = k.get_value::<String, _>(value) else { continue };
        let path = path.replace('/', "\\");
        if path.trim().is_empty() {
            continue;
        }
        for a in apps.iter_mut().filter(|a| same_folder(a.install_location.as_ref(), &path)) {
            a.game_source = Some(source.into());
        }
    }
}

fn uninstall_contains(a: &App, needle: &str) -> bool {
    a.uninstall_string.as_ref().is_some_and(|u| u.to_lowercase().contains(&needle.to_lowercase()))
}

pub fn attach_launchers(apps: &mut Vec<App>) {
    attach_epic(apps);
    tag_by_registry(apps, r"SOFTWARE\GOG.com\Games", "path", GOG);
    for a in apps.iter_mut().filter(|a| uninstall_contains(a, "uplay://uninstall")) {
        a.game_source = Some(UBISOFT.into());
        a.set_launcher(UBISOFT);
    }
    tag_by_registry(apps, r"SOFTWARE\Ubisoft\Launcher\Installs", "InstallDir", UBISOFT);
    for a in apps.iter_mut().filter(|a| uninstall_contains(a, r"EAInstaller\") || uninstall_contains(a, r"__Installer\")) {
        a.game_source.get_or_insert_with(|| EA.into());
    }
    for a in apps.iter_mut().filter(|a| uninstall_contains(a, "Blizzard Uninstaller") && !a.name.eq_ignore_ascii_case("Battle.net")) {
        a.game_source = Some(BATTLE_NET.into());
    }
}

/// Xbox / Game Pass titles are Store packages installed into an XboxGames folder.
pub fn tag_xbox(apps: &mut [App]) {
    for a in apps.iter_mut().filter(|a| a.is_store) {
        if a.install_location.as_ref().is_some_and(|l| l.to_string_lossy().to_lowercase().contains(r"\xboxgames\")) {
            a.game_source = Some(XBOX.into());
        }
    }
}

/// Steam app id → last played (unix seconds), from every local Steam user.
pub fn steam_last_played() -> std::collections::HashMap<String, i64> {
    let mut out = std::collections::HashMap::new();
    let Some(steam) = steam_path() else { return out };
    let Ok(users) = std::fs::read_dir(steam.join("userdata")) else { return out };
    for u in users.flatten() {
        let Ok(text) = std::fs::read_to_string(u.path().join(r"config\localconfig.vdf")) else { continue };
        let o = vdf::parse(&text);
        let Some(apps) = vdf::dig(&o, &["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"]).and_then(Value::as_obj) else {
            continue;
        };
        for (id, v) in apps {
            let Some(t) = v.as_obj().and_then(|a| vdf::get(a, "LastPlayed")).and_then(Value::as_str).and_then(|s| s.parse::<i64>().ok()) else {
                continue;
            };
            if t > 0 && out.get(id).is_none_or(|old| *old < t) {
                out.insert(id.clone(), t);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epic_manifest() {
        let j = r#"{"AppName":"Fortnite","DisplayName":"Fortnite","InstallLocation":"D:\\Epic\\Fortnite","InstallSize":1000,"MainGameAppName":"Fortnite"}"#;
        let g = parse_epic_manifest(j, Path::new("x.item")).unwrap();
        assert_eq!(g.display_name, "Fortnite");
        assert_eq!(g.install_size, 1000);
        let dlc = r#"{"AppName":"DLC1","MainGameAppName":"Fortnite"}"#;
        assert!(parse_epic_manifest(dlc, Path::new("y.item")).is_none());
        let partial = r#"{"AppName":"X","bIsIncompleteInstall":true}"#;
        assert!(parse_epic_manifest(partial, Path::new("z.item")).is_none());
    }

    #[test]
    fn reads_local_steam() {
        // just must not fail when Steam is absent or present
        let games = steam_games();
        for g in &games {
            assert!(!g.app_id.is_empty());
        }
        let _ = steam_last_played();
    }
}
