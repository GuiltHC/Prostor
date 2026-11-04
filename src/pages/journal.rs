//! "Журнал" page: everything the app changed, with undo (port of ViewModels/JournalViewModel.cs).

use crate::data::{self, journal};
use crate::fmt;
use crate::pages::Navigate;
use crate::services::undo;
use crate::sys::shell;
use crate::ui::{self, Busy, Lucide, Tone};
use chrono::{Datelike, Local};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::sync::atomic::Ordering;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Programs,
    Cleanup,
    Files,
    System,
}

pub struct JournalView {
    entries: Vec<journal::Entry>,
    version: u64,
    filter: Filter,
    busy: Option<String>,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<Navigate> for JournalView {}

fn kind_icon(kind: &str) -> (Lucide, Tone) {
    match kind {
        "uninstall" | "force" => (Lucide::Trash, Tone::Danger),
        "entry" => (Lucide::ListX, Tone::Accent),
        "leftovers" => (Lucide::Shredder, Tone::Accent),
        "cleanup" | "autoclean" => (Lucide::Sparkles, Tone::Success),
        "delete" | "duplicates" => (Lucide::Trash, Tone::Danger),
        "compress" => (Lucide::Archive, Tone::Success),
        "move" => (Lucide::FolderInput, Tone::Accent),
        "startup" => (Lucide::Rocket, Tone::Warning),
        "update" => (Lucide::Download, Tone::Accent),
        _ => (Lucide::Info, Tone::Accent),
    }
}

const MONTHS: [&str; 12] = ["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"];
const WEEKDAYS: [&str; 7] = ["понедельник", "вторник", "среда", "четверг", "пятница", "суббота", "воскресенье"];

fn day_text(d: chrono::NaiveDate) -> String {
    let today = Local::now().date_naive();
    if d == today {
        return "Сегодня".into();
    }
    if Some(d) == today.pred_opt() {
        return "Вчера".into();
    }
    let m = MONTHS[d.month0() as usize];
    if d.year() == today.year() {
        format!("{} {m}, {}", d.day(), WEEKDAYS[d.weekday().num_days_from_monday() as usize])
    } else {
        format!("{} {m} {}", d.day(), d.year())
    }
}

impl JournalView {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let mut v = Self { entries: Vec::new(), version: u64::MAX, filter: Filter::All, busy: None, _tasks: Vec::new() };
        v.reload();
        v
    }

    fn reload(&mut self) {
        self.entries = journal::load();
        self.version = journal::VERSION.load(Ordering::Relaxed);
    }

    fn matches(&self, e: &journal::Entry) -> bool {
        match self.filter {
            Filter::All => true,
            Filter::Programs => matches!(e.kind.as_str(), "uninstall" | "force" | "entry" | "leftovers" | "update"),
            Filter::Cleanup => matches!(e.kind.as_str(), "cleanup" | "autoclean"),
            Filter::Files => matches!(e.kind.as_str(), "delete" | "duplicates" | "compress" | "move"),
            Filter::System => matches!(e.kind.as_str(), "startup" | "system"),
        }
    }

    fn freed_total(&self) -> u64 {
        self.entries
            .iter()
            .filter(|e| matches!(e.kind.as_str(), "uninstall" | "cleanup" | "autoclean" | "delete" | "duplicates" | "leftovers" | "force" | "compress" | "move"))
            .map(|e| e.bytes.max(0) as u64)
            .sum()
    }

    fn start_undo(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.entries.iter().find(|e| e.id == id).cloned() else { return };
        let Some(what) = e.undo.describe() else { return };
        let this = cx.entity().downgrade();
        let message = format!("«{}»\n\n{}", e.title, undo::explain(e.undo));
        ui::confirm(window, cx, (Lucide::Undo2, Tone::Accent), format!("{what}?"), message, "Отменить действие", move |window, cx| {
            let e = e.clone();
            this.update(cx, |v, cx| v.run_undo(e, window, cx)).ok();
        });
    }

    fn run_undo(&mut self, e: journal::Entry, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = Some(e.id.clone());
        let busy = cx.new(|_| Busy::new("Возвращаем…"));
        ui::open_busy(window, cx, busy.clone());
        let status = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let s2 = status.clone();
        let entry = e.clone();
        let work = cx.background_executor().spawn(async move { undo::undo(&entry, |t| *s2.lock().unwrap() = t.to_string()) });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = status;
            this.update_in(cx, |v, window, cx| {
                v.busy = None;
                ui::close_busy(window, cx);
                match result {
                    Ok(()) => {
                        journal::mark_undone(&e.id);
                        v.reload();
                        window.push_notification(Notification::success("Готово — действие отменено"), cx);
                        match e.undo {
                            journal::UndoKind::RegImport => cx.emit(Navigate::to("reload:programs")),
                            journal::UndoKind::DelayedStart => cx.emit(Navigate::to("reload:startup")),
                            _ => {}
                        }
                    }
                    Err(msg) => ui::error(window, cx, "Не получилось отменить", msg),
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.push(task);
        cx.notify();
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        ui::confirm(
            window,
            cx,
            (Lucide::TriangleAlert, Tone::Warning),
            "Очистить журнал?",
            "Записи исчезнут, и отменить эти действия отсюда будет нельзя. Резервные копии реестра останутся в папке Backups.",
            "Очистить",
            move |_, cx| {
                journal::clear();
                this.update(cx, |v, cx| {
                    v.reload();
                    cx.notify();
                })
                .ok();
            },
        );
    }
}

impl Render for JournalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if journal::VERSION.load(Ordering::Relaxed) != self.version {
            self.reload();
        }
        let theme = cx.theme().clone();
        let summary = if self.entries.is_empty() {
            "Здесь появятся удалённые программы, очистки и другие изменения — и кнопки, чтобы их отменить".to_string()
        } else {
            format!("{} · освобождено всего {}", fmt::count(self.entries.len() as u64, "запись", "записи", "записей"), fmt::size(self.freed_total()))
        };

        let opts: Vec<(String, bool)> = [(Filter::All, "Всё"), (Filter::Programs, "Программы"), (Filter::Cleanup, "Очистка"), (Filter::Files, "Файлы"), (Filter::System, "Система")]
            .iter()
            .map(|(f, l)| (l.to_string(), *f == self.filter))
            .collect();
        let filters = ui::chips(
            "journal-filter",
            &opts,
            |this: &mut Self, i, cx| {
                this.filter = [Filter::All, Filter::Programs, Filter::Cleanup, Filter::Files, Filter::System][i];
                cx.notify();
            },
            cx,
        );

        let mut body = v_flex().gap_3().child(filters);
        let visible: Vec<&journal::Entry> = self.entries.iter().filter(|e| self.matches(e)).collect();
        if self.entries.is_empty() {
            body = body.child(ui::empty_state(
                Lucide::BookOpen,
                Tone::Accent,
                "Пока ничего не происходило",
                "Удаления программ, очистки и другие изменения появятся здесь — многие из них можно будет отменить.",
                None,
                cx,
            ));
        } else if visible.is_empty() {
            body = body.child(ui::empty_state(Lucide::ListFilter, Tone::Neutral, "В этой категории записей нет", "Выберите другой фильтр выше.", None, cx));
        }
        let mut day: Option<chrono::NaiveDate> = None;
        let mut card: Option<Div> = None;
        let mut first_in_day = true;
        for (ix, e) in visible.into_iter().enumerate() {
            let d = e.time.date_naive();
            if day != Some(d) {
                if let Some(c) = card.take() {
                    body = body.child(c);
                }
                body = body.child(div().pt_3().px_1().text_xs().font_semibold().text_color(theme.muted_foreground).child(day_text(d)));
                card = Some(ui::card(cx).py_1());
                day = Some(d);
                first_in_day = true;
            }
            let (icon, tone) = kind_icon(&e.kind);
            let id = e.id.clone();
            let busy = self.busy.as_deref() == Some(&e.id);
            let row = h_flex()
                .gap_3()
                .mx_4()
                .py_3()
                .when(!first_in_day, |s| s.border_t_1().border_color(ui::card_border(&theme)))
                .child(ui::icon_tile(icon, if e.undone { Tone::Neutral } else { tone }, 34., cx))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().font_medium().truncate().when(e.undone, |s| s.text_color(theme.muted_foreground)).child(e.title.clone()))
                                .when(e.undone, |s| s.child(ui::badge("отменено", Tone::Neutral, cx))),
                        )
                        .when_some(e.detail.clone(), |s, d| s.child(div().text_xs().text_color(theme.muted_foreground).truncate().child(d))),
                )
                .when(e.bytes > 0, |s| s.child(div().text_sm().font_medium().flex_shrink_0().child(fmt::size(e.bytes as u64))))
                .child(div().w(px(44.)).flex_shrink_0().text_right().text_xs().text_color(theme.muted_foreground).child(e.time.format("%H:%M").to_string()))
                .when(e.can_undo(), |s| {
                    s.child(if busy {
                        Spinner::new().small().into_any_element()
                    } else {
                        Button::new(("undo", ix))
                            .outline()
                            .small()
                            .icon(Icon::new(Lucide::Undo2))
                            .label(e.undo.describe().unwrap_or("Отменить"))
                            .disabled(self.busy.is_some())
                            .on_click(cx.listener(move |this, _, window, cx| this.start_undo(id.clone(), window, cx)))
                            .into_any_element()
                    })
                });
            card = card.map(|c| c.child(row));
            first_in_day = false;
        }
        if let Some(c) = card {
            body = body.child(c);
        }

        let actions = h_flex()
            .gap_2()
            .child(Button::new("backups").ghost().icon(Icon::new(Lucide::FolderOpen)).label("Резервные копии").on_click(|_, _, _| {
                let dir = data::backups_dir();
                let _ = std::fs::create_dir_all(&dir);
                shell::open_folder(&dir);
            }))
            .child(
                Button::new("clear-journal")
                    .outline()
                    .icon(Icon::new(Lucide::Trash))
                    .label("Очистить журнал")
                    .disabled(self.entries.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| this.clear(window, cx))),
            );
        ui::page("journal-page", "Журнал", &summary, actions, body, cx)
    }
}
