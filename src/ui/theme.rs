//! The app's accent on top of the gpui-component theme: Prostor violet, or the Windows accent colour
//! when "UseSystemAccent" is on (the same setting as in the C# version).

use crate::data::settings;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;

/// Windows accent colour (DWM AccentColor is 0xAABBGGRR).
pub fn system_accent() -> Option<Rgba> {
    let k = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\DWM").ok()?;
    let v: u32 = k.get_value("AccentColor").ok()?;
    let (r, g, b) = (v & 0xFF, (v >> 8) & 0xFF, (v >> 16) & 0xFF);
    Some(Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 })
}

fn use_system_accent() -> bool {
    settings::get().extra.get("UseSystemAccent").and_then(|v| v.as_bool()).unwrap_or(false)
}

pub fn set_use_system_accent(on: bool) {
    settings::update(|s| {
        s.extra.insert("UseSystemAccent".into(), serde_json::Value::Bool(on));
    });
}

/// Applies the accent to the current theme. Call after every theme change.
pub fn apply_accent(cx: &mut App) {
    let dark = Theme::global(cx).is_dark();
    let base: Hsla = match use_system_accent().then(system_accent).flatten() {
        Some(c) => c.into(),
        None => rgb(0x7B6CFF).into(),
    };
    // keep enough contrast against the background in both themes
    let mut accent = base;
    accent.l = if dark { accent.l.clamp(0.55, 0.72) } else { accent.l.clamp(0.42, 0.58) };
    let hover = Hsla { l: (accent.l + if dark { 0.06 } else { -0.05 }).clamp(0.0, 1.0), ..accent };
    let active = Hsla { l: (accent.l + if dark { -0.06 } else { -0.10 }).clamp(0.0, 1.0), ..accent };
    let fg = if accent.l > 0.68 { hsla(0., 0., 0.08, 1.) } else { hsla(0., 0., 1., 1.) };
    Theme::update(cx, |t| {
        t.primary = accent;
        t.primary_hover = hover;
        t.primary_active = active;
        t.primary_foreground = fg;
        t.button_primary = accent;
        t.button_primary_hover = hover;
        t.button_primary_active = active;
        t.button_primary_foreground = fg;
        t.ring = accent;
        t.progress_bar = accent;
        t.slider_bar = accent;
        // the track is grey when off (the checked track takes the accent); the thumb is light in both themes
        t.switch = if dark { hsla(240. / 360., 0.05, 0.30, 1.0) } else { hsla(240. / 360., 0.08, 0.80, 1.0) };
        t.switch_thumb = hsla(0., 0., 0.98, 1.0);
        // dialogs dim the page behind them, so the question stands out
        t.overlay = hsla(240. / 360., 0.2, 0.02, if dark { 0.55 } else { 0.28 });
        t.sidebar_primary = accent;
        t.sidebar_primary_foreground = fg;
        t.link = accent;
        t.link_hover = hover;
        t.selection = accent.opacity(0.3);
        t.list_active = accent.opacity(if dark { 0.18 } else { 0.12 });
        t.list_active_border = accent.opacity(0.5);
        t.table_active = accent.opacity(if dark { 0.18 } else { 0.12 });
        t.table_active_border = accent.opacity(0.5);
        t.tab_active_foreground = accent;
        // calmer surfaces than the stock palette: the content layer is a touch lifted from the
        // window, borders are soft, so cards read as layers rather than outlines
        if dark {
            t.background = hsla(240. / 360., 0.06, 0.085, 1.0);
            t.secondary = hsla(240. / 360., 0.05, 0.14, 1.0);
            t.muted = hsla(240. / 360., 0.05, 0.155, 1.0);
            t.border = hsla(240. / 360., 0.05, 0.19, 1.0);
            t.input = hsla(240. / 360., 0.05, 0.22, 1.0);
            t.popover = hsla(240. / 360., 0.06, 0.11, 1.0);
        } else {
            t.background = hsla(240. / 360., 0.2, 0.988, 1.0);
            t.secondary = hsla(240. / 360., 0.16, 0.955, 1.0);
            t.muted = hsla(240. / 360., 0.16, 0.945, 1.0);
            t.border = hsla(240. / 360., 0.12, 0.89, 1.0);
            t.input = hsla(240. / 360., 0.12, 0.86, 1.0);
            // the stock amber is unreadable as text on white
            t.warning = hsla(32. / 360., 0.95, 0.42, 1.0);
        }
    });
    if mica_enabled() {
        // Root paints the window with this token; under Mica the window itself is the backdrop
        // and the content layer paints `background` on its own
        let t = Theme::global_mut(cx);
        t.tokens.background = hsla(0., 0., 0., 0.).into();
        cx.refresh_windows();
    }
}

/// Windows build number (22621+ has the Mica backdrop).
fn windows_build() -> u32 {
    winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .ok()
        .and_then(|k| k.get_value::<String, _>("CurrentBuildNumber").ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

pub fn mica_supported() -> bool {
    static BUILD: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *BUILD.get_or_init(windows_build) >= 22621
}

/// Mica is on by default where Windows supports it; "UseMica": false turns it off.
pub fn mica_enabled() -> bool {
    mica_supported() && settings::get().extra.get("UseMica").and_then(|v| v.as_bool()).unwrap_or(true)
}

pub fn set_use_mica(on: bool) {
    settings::update(|s| {
        s.extra.insert("UseMica".into(), serde_json::Value::Bool(on));
    });
}

/// Backdrop material and the dark/light frame for the window, in step with the theme.
pub fn apply_window(window: &mut Window, cx: &App) {
    window.set_background_appearance(if mica_enabled() { WindowBackgroundAppearance::MicaBackdrop } else { WindowBackgroundAppearance::Opaque });
    set_dark_frame(window, Theme::global(cx).is_dark());
}

/// The launcher's "start minimised" hint (some shells pass it) must not hide the main window at start.
pub fn restore_if_minimized(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, ShowWindow};
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    if let RawWindowHandle::Win32(h) = handle.as_raw() {
        let hwnd = h.hwnd.get() as _;
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
        }
    }
}

/// DWMWA_USE_IMMERSIVE_DARK_MODE: Mica and the window frame follow the app theme, not the system one.
fn set_dark_frame(window: &Window, dark: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    if let RawWindowHandle::Win32(h) = handle.as_raw() {
        let value: i32 = dark as i32;
        unsafe {
            DwmSetWindowAttribute(h.hwnd.get() as _, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, &value as *const i32 as _, 4);
        }
    }
}

/// "system", "light" or "dark", then the accent.
pub fn set_mode(mode: &str, mut window: Option<&mut Window>, cx: &mut App) {
    match mode {
        "light" => Theme::change(ThemeMode::Light, window.as_deref_mut(), cx),
        "dark" => Theme::change(ThemeMode::Dark, window.as_deref_mut(), cx),
        _ => Theme::sync_system_appearance(window.as_deref_mut(), cx),
    }
    apply_accent(cx);
    if let Some(w) = window {
        apply_window(w, cx);
    }
}
