//! Small self-closing notice in the corner of the screen, shown after a scheduled cleanup
//! (port of Views/NotificationWindow.cs).

use crate::ui::{self, Lucide, Tone};
use gpui_kit::component::{ActiveTheme as _, Icon, StyledExt as _, Theme, h_flex, v_flex};
use gpui_kit::*;
use std::time::Duration;

const SHOW_FOR: Duration = Duration::from_secs(6);

struct Notice {
    title: String,
    text: String,
}

impl Render for Notice {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let accent = theme.primary;
        v_flex()
            .id("notice")
            .size_full()
            .bg(theme.popover)
            // no Root in this window to set the text colour and font
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .cursor_pointer()
            .on_click(|_, window, _| window.remove_window())
            .child(
                h_flex()
                    .flex_1()
                    .gap_3()
                    .px_4()
                    .child(ui::icon_tile(Lucide::Sparkles, Tone::Success, 40., cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(div().text_xs().text_color(theme.muted_foreground).child(self.title.clone()))
                            .child(div().text_base().font_semibold().truncate().child(self.text.clone())),
                    )
                    .child(Icon::new(Lucide::ChevronRight).size_4().text_color(theme.muted_foreground)),
            )
            .child(
                // how long the notice stays: a thin line that runs out
                div().h(px(3.)).w_full().bg(theme.foreground.opacity(0.06)).child(
                    div().h_full().bg(accent).with_animation("countdown", Animation::new(SHOW_FOR), |el, t| el.w(relative(1.0 - t))),
                ),
            )
    }
}

/// Native rounded corners and shadow for the borderless window (Windows 11).
fn round_corners(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DwmSetWindowAttribute};
    const DWMWCP_ROUND: i32 = 2;
    if let Ok(h) = HasWindowHandle::window_handle(window)
        && let RawWindowHandle::Win32(w) = h.as_raw()
    {
        unsafe {
            DwmSetWindowAttribute(w.hwnd.get() as _, DWMWA_WINDOW_CORNER_PREFERENCE as u32, &DWMWCP_ROUND as *const i32 as _, 4);
        }
    }
}

/// Shows the notice for a few seconds, then exits.
pub fn show(title: &str, text: &str) {
    let (title, text) = (title.to_string(), text.to_string());
    gpui_kit::application().with_assets(crate::ui::icons::AppAssets).run(move |cx: &mut App| {
        gpui_kit::init(cx);
        Theme::sync_system_appearance(None, cx);
        ui::theme::apply_accent(cx);
        let size = size(px(360.), px(76.));
        // PROSTOR_DISPLAY=2 puts it on the second monitor, as for the main window
        let display = std::env::var("PROSTOR_DISPLAY").ok().and_then(|v| v.parse::<usize>().ok()).and_then(|n| {
            let primary = cx.primary_display().map(|d| d.id());
            let mut list = cx.displays();
            list.sort_by_key(|d| Some(d.id()) != primary);
            list.get(n.checked_sub(1)?).cloned()
        });
        let display = display.or_else(|| cx.primary_display());
        // GPUI checks the bounds against this display and falls back to a default size when
        // they do not fit, so the corner position and the display must agree
        let display_id = display.as_ref().map(|d| d.id());
        let origin = display
            .map(|d| {
                let b = d.visible_bounds();
                point(b.origin.x + b.size.width - size.width - px(16.), b.origin.y + b.size.height - size.height - px(16.))
            })
            .unwrap_or_else(|| point(px(100.), px(100.)));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size })),
            display_id,
            titlebar: None,
            kind: WindowKind::PopUp,
            focus: false,
            is_movable: false,
            is_resizable: false,
            ..Default::default()
        };
        let (t, x) = (title.clone(), text.clone());
        let opened = cx.open_window(options, move |window, cx| {
            round_corners(window);
            cx.new(|_| Notice { title: t, text: x })
        });
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        if opened.is_err() {
            cx.quit();
            return;
        }
        cx.spawn(async move |cx| {
            cx.background_executor().timer(SHOW_FOR).await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
