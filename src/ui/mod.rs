//! Shared building blocks for the pages: headers, cards, banners, badges and dialogs.

pub mod ask;
pub mod charts;
pub mod icons;
pub mod theme;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, Theme, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub use gpui_kit::assets::IconName as Lucide;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
    Info,
}

pub fn tone_color(t: Tone, theme: &Theme) -> Hsla {
    match t {
        Tone::Neutral => theme.muted_foreground,
        Tone::Accent => theme.primary,
        Tone::Success => theme.success,
        Tone::Warning => theme.warning,
        Tone::Danger => theme.danger,
        Tone::Info => theme.info,
    }
}

/// Soft background for a tone (banners, icon tiles).
pub fn tone_soft(t: Tone, theme: &Theme) -> Hsla {
    match t {
        Tone::Neutral => theme.muted,
        _ => tone_color(t, theme).opacity(if theme.is_dark() { 0.16 } else { 0.11 }),
    }
}

/// A page: title, subtitle, actions on the right, then scrolling content.
pub fn page(id: &'static str, title: &str, subtitle: &str, actions: impl IntoElement, body: impl IntoElement, cx: &App) -> Div {
    let theme = cx.theme();
    div().size_full().child(
        v_flex()
            .id(id)
            .size_full()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .px_8()
                    .pt_7()
                    .pb_10()
                    .gap_5()
                    .w_full()
                    .max_w(px(1240.))
                    .mx_auto()
                    .child(
                        h_flex()
                            .gap_4()
                            .items_end()
                            .justify_between()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_size(px(26.)).line_height(px(32.)).font_semibold().child(title.to_string()))
                                    .when(!subtitle.is_empty(), |s| {
                                        s.child(div().text_sm().text_color(theme.muted_foreground).max_w(px(760.)).child(subtitle.to_string()))
                                    }),
                            )
                            .child(h_flex().gap_2().flex_shrink_0().child(actions)),
                    )
                    .child(body),
            ),
    )
}

/// Rounded panel: a faint lift off the content layer with a hairline border.
pub fn card(cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .rounded_xl()
        .border_1()
        .border_color(card_border(theme))
        .bg(card_bg(theme))
}

pub fn card_bg(theme: &Theme) -> Hsla {
    if theme.is_dark() { theme.foreground.opacity(0.035) } else { hsla(0., 0., 1., 1.) }
}

pub fn card_border(theme: &Theme) -> Hsla {
    theme.foreground.opacity(if theme.is_dark() { 0.075 } else { 0.085 })
}

/// A pulsing placeholder block while data loads.
pub fn skeleton(w: impl Into<Length>, h: f32) -> gpui_kit::component::skeleton::Skeleton {
    gpui_kit::component::skeleton::Skeleton::new().w(w.into()).h(px(h)).rounded_md()
}

/// Placeholder list rows (icon, two lines, a value on the right) in a card.
pub fn skeleton_rows(n: usize, icon: f32, cx: &App) -> Div {
    let mut list = card(cx).p_1p5();
    for i in 0..n {
        // vary the widths so the rows do not look stamped out
        let (a, b) = [(0.42, 0.26), (0.55, 0.34), (0.36, 0.22), (0.48, 0.3)][i % 4];
        list = list.child(
            h_flex()
                .gap_3()
                .px_3()
                .py_2p5()
                .child(skeleton(px(icon), icon).rounded_lg())
                .child(v_flex().flex_1().gap_2().child(skeleton(relative(a), 12.)).child(skeleton(relative(b), 9.)))
                .child(skeleton(px(64.), 12.)),
        );
    }
    list
}

/// Background of a list row under the mouse.
pub fn row_hover(theme: &Theme) -> Hsla {
    theme.foreground.opacity(if theme.is_dark() { 0.045 } else { 0.035 })
}

/// A friendly placeholder for an empty list or a step that has not been taken yet.
pub fn empty_state(icon: Lucide, tone: Tone, title: impl Into<SharedString>, text: impl Into<SharedString>, action: Option<AnyElement>, cx: &App) -> Div {
    let theme = cx.theme();
    let color = tone_color(tone, theme);
    v_flex()
        .flex_1()
        .w_full()
        .items_center()
        .justify_center()
        .gap_2()
        .py_10()
        .px_6()
        .child(
            div()
                .size(px(60.))
                .mb_2()
                .rounded(px(18.))
                .bg(linear_gradient(135., linear_color_stop(color.opacity(0.26), 0.), linear_color_stop(color.opacity(0.06), 1.)))
                .border_1()
                .border_color(color.opacity(0.25))
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(icon).size_7().text_color(color)),
        )
        .child(div().text_lg().font_semibold().text_center().child(title.into()))
        .child(div().text_sm().text_color(theme.muted_foreground).max_w(px(540.)).text_center().child(text.into()))
        .when_some(action, |s, a| s.child(div().pt_3().child(a)))
}

/// Card background on hover, for cards that open something.
pub fn card_hover(theme: &Theme) -> Hsla {
    if theme.is_dark() { theme.foreground.opacity(0.06) } else { theme.foreground.opacity(0.025) }
}

/// Coloured square with an icon.
pub fn icon_tile(icon: impl Into<Icon>, tone: Tone, size: f32, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex_shrink_0()
        .size(px(size))
        .rounded(px(size * 0.28))
        .bg(tone_soft(tone, theme))
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::new(icon).size(px(size * 0.5)).text_color(tone_color(tone, theme)))
}

/// Coloured strip with an icon, text and an optional button.
pub fn banner(tone: Tone, icon: impl Into<Icon>, text: impl Into<SharedString>, action: Option<AnyElement>, cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .gap_3()
        .px_4()
        .py_3()
        .rounded_lg()
        .bg(tone_soft(tone, theme))
        .child(Icon::new(icon).size_4().flex_shrink_0().text_color(tone_color(tone, theme)))
        .child(div().flex_1().min_w_0().text_sm().child(text.into()))
        .when_some(action, |s, a| s.child(div().flex_shrink_0().child(a)))
}

/// Small rounded label.
pub fn badge(text: impl Into<SharedString>, tone: Tone, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex_shrink_0()
        .px_2()
        .py(px(1.))
        .rounded_full()
        .text_xs()
        .bg(tone_soft(tone, theme))
        .text_color(tone_color(tone, theme))
        .child(text.into())
}

pub fn section_title(text: impl Into<SharedString>, hint: Option<&str>, cx: &App) -> Div {
    v_flex()
        .gap_0p5()
        .pt_3()
        .child(div().text_base().font_semibold().child(text.into()))
        .when_some(hint, |s, h| s.child(div().text_xs().text_color(cx.theme().muted_foreground).child(h.to_string())))
}

/// Thin progress line.
pub fn bar(fraction: f32, color: Hsla, cx: &App) -> Div {
    div()
        .h(px(6.))
        .w_full()
        .rounded_full()
        .bg(cx.theme().muted)
        .overflow_hidden()
        .child(div().h_full().rounded_full().w(relative(fraction.clamp(0.0, 1.0))).bg(color))
}

// ------------------------------------------------------------------ program icons

/// The program's own icon when it is cached, otherwise a letter on a tinted square.
pub fn app_icon(path: Option<&std::path::Path>, title: &str, size: f32, cx: &App) -> AnyElement {
    if let Some(Some(image)) = path.and_then(crate::sys::icons::cached) {
        return img(image).size(px(size)).flex_shrink_0().into_any_element();
    }
    let theme = cx.theme();
    let letter = title.trim().chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    // a stable hue per name, so the same program always gets the same colour
    let hue = (title.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 360) as f32 / 360.0;
    div()
        .flex_shrink_0()
        .size(px(size))
        .rounded(px(size * 0.25))
        .bg(hsla(hue, 0.45, if theme.is_dark() { 0.30 } else { 0.86 }, 1.0))
        .text_color(hsla(hue, 0.5, if theme.is_dark() { 0.85 } else { 0.30 }, 1.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(size * 0.45))
        .font_semibold()
        .child(letter)
        .into_any_element()
}

/// Extracts icons in the background and redraws the view when they are ready.
pub fn preload_icons<V: 'static>(paths: Vec<std::path::PathBuf>, cx: &mut Context<V>) -> Task<()> {
    let work = cx.background_executor().spawn(async move {
        for p in paths.iter().filter(|p| crate::sys::icons::cached(p).is_none()) {
            crate::sys::icons::load(p);
        }
    });
    cx.spawn(async move |this, cx| {
        work.await;
        this.update(cx, |_, cx| cx.notify()).ok();
    })
}

/// Segmented filter: one pill per option, the selected one filled.
pub fn segmented<V: 'static>(
    id: &'static str,
    options: &[(String, bool)],
    on_select: impl Fn(&mut V, usize, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> Div {
    let theme = cx.theme().clone();
    let dark = theme.is_dark();
    let on_select = std::rc::Rc::new(on_select);
    let mut row = h_flex().gap_0p5().p(px(3.)).rounded_lg().bg(theme.foreground.opacity(if dark { 0.06 } else { 0.05 }));
    for (i, (label, selected)) in options.iter().enumerate() {
        let f = on_select.clone();
        row = row.child(
            div()
                .id((id, i))
                .px_3()
                .py(px(5.))
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .when(*selected, |s| {
                    s.bg(if dark { theme.foreground.opacity(0.11) } else { hsla(0., 0., 1., 1.) }).shadow_xs().font_medium().text_color(theme.foreground)
                })
                .when(!*selected, |s| s.text_color(theme.muted_foreground).hover(|s| s.text_color(theme.foreground).bg(theme.foreground.opacity(0.04))))
                .child(label.clone())
                .on_click(cx.listener(move |this, _, _, cx| f(this, i, cx))),
        );
    }
    row
}

/// Filter chips that wrap: "Игры · 3" shows the number in a quieter weight after the name.
pub fn chips<V: 'static>(
    id: &'static str,
    options: &[(String, bool)],
    on_select: impl Fn(&mut V, usize, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> Div {
    let theme = cx.theme().clone();
    let on_select = std::rc::Rc::new(on_select);
    let mut row = h_flex().gap_1p5().flex_wrap();
    for (i, (label, selected)) in options.iter().enumerate() {
        let f = on_select.clone();
        let (name, count) = match label.split_once(" · ") {
            Some((n, c)) => (n.to_string(), Some(c.to_string())),
            None => (label.clone(), None),
        };
        let selected = *selected;
        row = row.child(
            h_flex()
                .id((id, i))
                .gap_1p5()
                .h(px(30.))
                .px_3()
                .rounded_full()
                .border_1()
                .text_sm()
                .cursor_pointer()
                .when(selected, |s| s.bg(theme.primary.opacity(0.14)).border_color(theme.primary.opacity(0.45)).text_color(theme.foreground).font_medium())
                .when(!selected, |s| {
                    s.border_color(card_border(&theme))
                        .text_color(theme.foreground.opacity(0.8))
                        .hover(|s| s.bg(theme.foreground.opacity(0.04)).text_color(theme.foreground))
                })
                .child(name)
                .when_some(count, |s, c| s.child(div().text_xs().text_color(if selected { theme.primary } else { theme.muted_foreground }).child(c)))
                .on_click(cx.listener(move |this, _, _, cx| f(this, i, cx))),
        );
    }
    row
}

// ------------------------------------------------------------------ dialogs

fn dialog_body(icon: Option<(Lucide, Tone)>, title: &str, message: &str, cx: &App) -> Div {
    h_flex()
        .gap_4()
        .items_start()
        .when_some(icon, |s, (i, t)| s.child(icon_tile(i, t, 40., cx)))
        .child(
            v_flex()
                .gap_2()
                .flex_1()
                .min_w_0()
                .child(div().text_lg().font_semibold().child(title.to_string()))
                .when(!message.is_empty(), |s| {
                    s.child(v_flex().gap_1().text_sm().text_color(cx.theme().muted_foreground).children(
                        message.split('\n').map(|l| div().min_h(px(8.)).child(l.to_string())),
                    ))
                }),
        )
}

/// Asks before doing something. `on_ok` runs after the dialog closes.
pub fn confirm(
    window: &mut Window,
    cx: &mut App,
    icon: (Lucide, Tone),
    title: impl Into<String>,
    message: impl Into<String>,
    ok_text: impl Into<SharedString>,
    on_ok: impl Fn(&mut Window, &mut App) + 'static,
) {
    let (title, message, ok_text) = (title.into(), message.into(), ok_text.into());
    let on_ok = std::rc::Rc::new(on_ok);
    let danger = icon.1 == Tone::Danger;
    window.open_dialog(cx, move |dialog: Dialog, _window, cx| {
        let on_ok = on_ok.clone();
        dialog.w(px(500.)).bg(cx.theme().background).child(dialog_body(Some(icon), &title, &message, cx)).footer(
            h_flex()
                .w_full()
                .justify_end()
                .gap_2()
                .child(Button::new("cancel").label("Отмена").on_click(|_, window, cx| window.close_dialog(cx)))
                .child(
                    Button::new("ok")
                        .map(|b| if danger { b.danger() } else { b.primary() })
                        .label(ok_text.clone())
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            on_ok(window, cx);
                        }),
                ),
        )
    });
}

/// Shows a result or an error.
pub fn message(window: &mut Window, cx: &mut App, icon: (Lucide, Tone), title: impl Into<String>, message: impl Into<String>) {
    let (title, message) = (title.into(), message.into());
    window.open_dialog(cx, move |dialog: Dialog, _window, cx| {
        dialog.w(px(460.)).bg(cx.theme().background).child(dialog_body(Some(icon), &title, &message, cx)).footer(
            h_flex()
                .w_full()
                .justify_end()
                .child(Button::new("ok").primary().label("Хорошо").on_click(|_, window, cx| window.close_dialog(cx))),
        )
    });
}

pub fn error(window: &mut Window, cx: &mut App, title: impl Into<String>, text: impl Into<String>) {
    message(window, cx, (Lucide::TriangleAlert, Tone::Danger), title, text);
}

/// State of a long operation shown in a modal.
pub struct Busy {
    pub title: String,
    pub message: String,
    pub detail: String,
    /// None — indeterminate.
    pub progress: Option<f32>,
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub cancel_label: String,
}

impl Busy {
    pub fn new(title: impl Into<String>) -> Self {
        Self { title: title.into(), message: String::new(), detail: String::new(), progress: None, cancel: None, cancel_label: "Остановить".into() }
    }
}

impl Render for Busy {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_3()
                    .child(gpui_kit::component::spinner::Spinner::new().color(theme.primary))
                    .child(div().flex_1().min_w_0().text_lg().font_semibold().truncate().child(self.title.clone()))
                    .when_some(self.progress, |s, p| {
                        s.child(div().text_sm().font_medium().text_color(theme.muted_foreground).child(format!("{}%", (p * 100.0).round() as u32)))
                    }),
            )
            .when(!self.message.is_empty(), |s| s.child(div().text_sm().truncate().child(self.message.clone())))
            .child(match self.progress {
                Some(p) => Progress::new("busy-progress").value(p * 100.0).into_any_element(),
                None => Progress::new("busy-progress").loading(true).into_any_element(),
            })
            .when(!self.detail.is_empty(), |s| s.child(div().text_xs().text_color(theme.muted_foreground).child(self.detail.clone())))
            .when_some(self.cancel.clone(), |s, flag| {
                s.child(
                    h_flex().justify_end().child(
                        Button::new("busy-cancel")
                            .small()
                            .outline()
                            .label(self.cancel_label.clone())
                            .on_click(move |_, _, _| flag.store(true, std::sync::atomic::Ordering::Relaxed)),
                    ),
                )
            })
    }
}

/// Opens a modal that shows `busy` until [`close_busy`].
pub fn open_busy(window: &mut Window, cx: &mut App, busy: Entity<Busy>) {
    window.open_dialog(cx, move |dialog: Dialog, _, cx| {
        dialog.w(px(480.)).bg(cx.theme().background).close_button(false).overlay_closable(false).keyboard(false).child(busy.clone())
    });
}

pub fn close_busy(window: &mut Window, cx: &mut App) {
    window.close_dialog(cx);
}
