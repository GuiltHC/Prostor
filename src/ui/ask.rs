//! Awaitable dialogs: open a question from an async task and get the pressed button back.
//! Used by multi-step flows (uninstall → wait → "not removed?" → force → review leftovers).

use super::{Lucide, Tone, icon_tile};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Primary,
    Danger,
    Plain,
}

/// A question with buttons. The answer is the index of the pressed button.
pub struct Ask {
    pub icon: (Lucide, Tone),
    pub title: String,
    pub message: String,
    pub buttons: Vec<(String, Kind)>,
    /// Content under the message (options, a list to review).
    pub extra: Option<AnyView>,
    pub footnote: Option<String>,
    pub width: f32,
}

impl Ask {
    pub fn new(icon: (Lucide, Tone), title: impl Into<String>, message: impl Into<String>) -> Self {
        Self { icon, title: title.into(), message: message.into(), buttons: Vec::new(), extra: None, footnote: None, width: 520. }
    }
    pub fn button(mut self, label: impl Into<String>, kind: Kind) -> Self {
        self.buttons.push((label.into(), kind));
        self
    }
    pub fn extra(mut self, v: impl Into<AnyView>) -> Self {
        self.extra = Some(v.into());
        self
    }
    pub fn footnote(mut self, f: impl Into<String>) -> Self {
        let f = f.into();
        self.footnote = (!f.is_empty()).then_some(f);
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
}

/// Opens the question and resolves with the pressed button (None when the window went away).
pub async fn ask(cx: &mut AsyncWindowContext, a: Ask) -> Option<usize> {
    let (tx, rx) = futures::channel::oneshot::channel::<usize>();
    let tx = Rc::new(RefCell::new(Some(tx)));
    let a = Rc::new(a);
    cx.update(|window, cx| {
        let a = a.clone();
        let tx = tx.clone();
        window.open_dialog(cx, move |dialog: Dialog, _, cx| {
            let theme = cx.theme().clone();
            let mut buttons = h_flex().w_full().justify_end().gap_2();
            // the last button is the main one: show it on the right
            for (i, (label, kind)) in a.buttons.iter().enumerate().rev().collect::<Vec<_>>().into_iter().rev() {
                let tx = tx.clone();
                let b = Button::new(("ask", i)).label(label.clone());
                let b = match kind {
                    Kind::Primary => b.primary(),
                    Kind::Danger => b.danger(),
                    Kind::Plain => b,
                };
                buttons = buttons.child(b.on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    if let Some(tx) = tx.borrow_mut().take() {
                        let _ = tx.send(i);
                    }
                }));
            }
            dialog
                .w(px(a.width))
                .bg(theme.background)
                .close_button(false)
                .overlay_closable(false)
                .keyboard(false)
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            h_flex()
                                .gap_4()
                                .items_start()
                                .child(icon_tile(a.icon.0, a.icon.1, 40., cx))
                                .child(
                                    v_flex()
                                        .gap_2()
                                        .flex_1()
                                        .min_w_0()
                                        .child(div().text_lg().font_semibold().child(a.title.clone()))
                                        .when(!a.message.is_empty(), |s| {
                                            s.child(v_flex().gap_1().text_sm().text_color(theme.muted_foreground).children(
                                                a.message.split('\n').map(|l| div().min_h(px(8.)).child(l.to_string())),
                                            ))
                                        }),
                                ),
                        )
                        .when_some(a.extra.clone(), |s, v| s.child(v))
                        .when_some(a.footnote.clone(), |s, f| s.child(div().text_xs().text_color(theme.muted_foreground).child(f))),
                )
                .footer(buttons)
        });
    })
    .ok()?;
    rx.await.ok()
}

/// Several views one under another (options + a list).
pub struct Stack(pub Vec<AnyView>);

impl Render for Stack {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        v_flex().gap_3().children(self.0.iter().cloned())
    }
}

// ------------------------------------------------------------------ options (checkboxes)

pub struct OptionItem {
    pub label: String,
    pub checked: bool,
    pub enabled: bool,
}

/// A few checkboxes shown inside a question.
pub struct Options {
    pub items: Vec<OptionItem>,
}

impl Options {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }
    pub fn add(mut self, label: impl Into<String>, checked: bool, enabled: bool) -> Self {
        self.items.push(OptionItem { label: label.into(), checked, enabled });
        self
    }
    pub fn checked(&self, i: usize) -> bool {
        self.items.get(i).is_some_and(|o| o.checked && o.enabled)
    }
}

impl Render for Options {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut col = v_flex().gap_2().pl(px(56.));
        for (i, o) in self.items.iter().enumerate() {
            col = col.child(
                Checkbox::new(("opt", i))
                    .label(o.label.clone())
                    .checked(o.checked)
                    .disabled(!o.enabled)
                    .on_click(cx.listener(move |this, c: &bool, _, cx| {
                        this.items[i].checked = *c;
                        cx.notify();
                    })),
            );
        }
        col
    }
}

// ------------------------------------------------------------------ picks (a list to review)

pub struct Pick {
    pub title: String,
    pub subtitle: String,
    pub detail: String,
    pub checked: bool,
}

/// A scrolling list with a checkbox per row (leftovers, programs to update or remove).
pub struct Picks {
    pub items: Vec<Pick>,
}

impl Picks {
    pub fn checked(&self) -> Vec<usize> {
        self.items.iter().enumerate().filter(|(_, p)| p.checked).map(|(i, _)| i).collect()
    }
}

impl Render for Picks {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let all = self.items.iter().all(|p| p.checked);
        let mut list = v_flex().gap_0p5();
        for (i, p) in self.items.iter().enumerate() {
            list = list.child(
                h_flex()
                    .id(("pick", i))
                    .gap_3()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.list_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.items[i].checked = !this.items[i].checked;
                        cx.notify();
                    }))
                    .child(Checkbox::new(("pick-chk", i)).checked(p.checked).on_click(cx.listener(move |this, c: &bool, _, cx| {
                        this.items[i].checked = *c;
                        cx.notify();
                    })))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().truncate().child(p.title.clone()))
                            .when(!p.subtitle.is_empty(), |s| {
                                s.child(div().text_xs().text_color(theme.muted_foreground).truncate().child(p.subtitle.clone()))
                            }),
                    )
                    .child(div().text_xs().text_color(theme.muted_foreground).flex_shrink_0().child(p.detail.clone())),
            );
        }
        v_flex()
            .gap_1()
            .child(
                h_flex().justify_between().child(div().text_xs().text_color(theme.muted_foreground).child(format!(
                    "Отмечено {} из {}",
                    self.items.iter().filter(|p| p.checked).count(),
                    self.items.len()
                ))).child(
                    Button::new("pick-all")
                        .ghost()
                        .label(if all { "Снять все" } else { "Отметить все" })
                        .disabled(self.items.is_empty())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            for p in &mut this.items {
                                p.checked = !all;
                            }
                            cx.notify();
                        })),
                ),
            )
            .child(
                div()
                    .id("picks-scroll")
                    .max_h(px(300.))
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .p_1()
                    .overflow_y_scrollbar()
                    .child(list),
            )
    }
}
