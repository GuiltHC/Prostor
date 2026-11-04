//! Only the icons the app draws are embedded: the component set plus the Lucide icons used in
//! the pages (the full catalog would add about a megabyte to the exe).

use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

gpui_kit::assets::icon_assets!(
    PageIcons,
    [
        AppWindow, Archive, ArchiveRestore, ArrowUp, Ban, BookOpen, Bug, ChartPie, ChevronRight, CircleCheck, Clock, Cloud, Code, Copy, Cpu,
        Database, Download, ExternalLink, File, FileClock, FileDown, FileText, Film, Folder, FolderInput, FolderOpen, Gamepad2, Globe,
        HardDrive, Hourglass, Image, Info, Keyboard, LayoutDashboard, ListFilter, ListX, Lock, MessageCircle, Monitor, MonitorCog, Moon,
        Music, Package, Palette, Play, Plus, RefreshCw, Rocket, Search, Send, Settings, Settings2, ShieldAlert, ShieldCheck, Shredder, Sparkles, Store,
        Sun, Timer, Trash, TrendingDown, TrendingUp, TriangleAlert, Undo2, Usb, User, Users, Zap,
    ]
);

/// The page icons first, then the component bundle (checkboxes, dialogs, inputs…).
#[derive(Clone, Copy, Default)]
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Ok(Some(data)) = PageIcons.load(path) {
            return Ok(Some(data));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = PageIcons.list(path)?;
        all.extend(gpui_kit::assets::Assets.list(path)?);
        all.sort();
        all.dedup();
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon the code names must resolve, or it would silently draw nothing.
    #[test]
    fn used_icons_are_embedded() {
        let src = concat!(
            include_str!("../app.rs"),
            include_str!("../ui/mod.rs"),
            include_str!("../ui/ask.rs"),
            include_str!("../pages/analyzer.rs"),
            include_str!("../pages/cleanup.rs"),
            include_str!("../pages/dashboard.rs"),
            include_str!("../pages/journal.rs"),
            include_str!("../pages/programs.rs"),
            include_str!("../pages/settings.rs"),
            include_str!("../pages/startup.rs"),
        );
        let re = regex::Regex::new(r"Lucide::([A-Z]\w+)").unwrap();
        for cap in re.captures_iter(src) {
            let name = &cap[1];
            let icon = gpui_kit::assets::IconName::ALL.iter().copied().find(|i| format!("{i:?}") == name).unwrap_or_else(|| panic!("unknown icon {name}"));
            let path = icon.path();
            assert!(matches!(AppAssets.load(&path), Ok(Some(_))), "icon {name} ({path}) is not embedded");
        }
    }
}
