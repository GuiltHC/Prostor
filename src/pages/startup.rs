//! "Автозагрузка" page (port of ViewModels/StartupViewModel.cs + Views/StartupView.xaml).

use crate::data::journal;
use crate::fmt;
use crate::services::startup::{self, BootRecord, Item, Kind};
use crate::sys::shell;
use crate::ui::{self, Lucide, Tone};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Programs,
    Tasks,
    Services,
}

enum Boot {
    Loading,
    NeedsAdmin,
    Empty,
    Ready(Vec<BootRecord>),
}

pub struct StartupView {
    items: Vec<Item>,
    loading: bool,
    filter: Filter,
    search: Entity<InputState>,
    query: String,
    boot: Boot,
    busy: Option<String>,
    _tasks: Vec<Task<()>>,
    _subs: Vec<Subscription>,
}

fn seconds(s: f64) -> String {
    if s >= 60.0 { format!("{} мин {} с", (s / 60.0) as u64, (s % 60.0) as u64) } else { format!("{:.1} с", s).replace('.', ",") }
}

impl StartupView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск по названию или издателю"));
        let sub = cx.subscribe(&search, |this: &mut Self, state, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.query = state.read(cx).value().to_string();
                cx.notify();
            }
        });
        let mut v = Self {
            items: Vec::new(),
            loading: false,
            filter: Filter::All,
            search,
            query: String::new(),
            boot: Boot::Loading,
            busy: None,
            _tasks: Vec::new(),
            _subs: vec![sub],
        };
        v.load(cx);
        v
    }

    /// Ctrl+F.
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |s, cx| s.focus(window, cx));
    }

    pub fn enabled_count(&self) -> usize {
        self.items.iter().filter(|i| i.enabled).count()
    }

    pub fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.boot = Boot::Loading;
        cx.notify();
        let items = cx.background_executor().spawn(async { startup::load() });
        let boots = cx.background_executor().spawn(async { startup::load_boot_history(20) });
        let t1 = cx.spawn(async move |this, cx| {
            let items = items.await;
            let paths: Vec<_> = items.iter().filter_map(|i| i.file_path.clone()).collect();
            this.update(cx, |this, cx| {
                this.items = items;
                this.loading = false;
                let icons = ui::preload_icons(paths, cx);
                this._tasks.push(icons);
                cx.notify();
            })
            .ok();
        });
        let t2 = cx.spawn(async move |this, cx| {
            let b = boots.await;
            this.update(cx, |this, cx| {
                this.boot = match b {
                    None => Boot::NeedsAdmin,
                    Some(v) if v.is_empty() => Boot::Empty,
                    Some(v) => Boot::Ready(v),
                };
                cx.notify();
            })
            .ok();
        });
        self._tasks = vec![t1, t2];
    }

    fn visible(&self) -> Vec<usize> {
        let q = self.query.trim().to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, i)| {
                if !q.is_empty()
                    && !i.title().to_lowercase().contains(&q)
                    && !i.name.to_lowercase().contains(&q)
                    && !i.publisher.as_ref().is_some_and(|p| p.to_lowercase().contains(&q))
                {
                    return false;
                }
                match self.filter {
                    Filter::All => true,
                    Filter::Programs => matches!(i.kind, Kind::Run | Kind::Folder),
                    Filter::Tasks => i.kind == Kind::Task,
                    Filter::Services => i.kind == Kind::Service,
                }
            })
            .map(|(ix, _)| ix)
            .collect()
    }

    fn toggle(&mut self, ix: usize, enabled: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut item) = self.items.get(ix).cloned() else { return };
        let title = item.title();
        let source = item.source;
        self.busy = Some(item.key());
        cx.notify();
        let work = cx.background_executor().spawn(async move {
            let r = startup::set_enabled(&mut item, enabled);
            (item, r)
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let (item, r) = work.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match r {
                    Ok(()) => {
                        journal::add(
                            journal::Entry::new(
                                "startup",
                                if enabled { format!("«{title}» включена в автозагрузку") } else { format!("«{title}» убрана из автозагрузки") },
                            )
                            .detail(source),
                        );
                        if let Some(slot) = this.items.get_mut(ix) {
                            *slot = item;
                        }
                        let text = if enabled { format!("«{title}» будет запускаться с Windows") } else { format!("«{title}» убрана из автозагрузки") };
                        window.push_notification(if enabled { Notification::info(text) } else { Notification::success(text) }, cx);
                    }
                    Err(e) => {
                        crate::data::log_error("startup toggle", &e);
                        window.push_notification(Notification::warning("Не хватает прав — запустите Простор от имени администратора"), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.push(task);
    }

    fn toggle_delay(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut item) = self.items.get(ix).cloned() else { return };
        if !item.can_delay() {
            return;
        }
        let (title, source, key) = (item.title(), item.source, item.key());
        let work = cx.background_executor().spawn(async move {
            let r = match item.delay_task.clone() {
                Some(task) => startup::undo_delayed(&item.key(), &task).map(|_| {
                    item.delay_task = None;
                    None
                }),
                None => startup::make_delayed(&mut item, "PT1M").map(Some),
            };
            (item, r)
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let (item, r) = work.await;
            this.update_in(cx, |this, window, cx| {
                match r {
                    Ok(Some(task)) => {
                        journal::add(
                            journal::Entry::new("startup", format!("«{title}» запускается через минуту после входа"))
                                .detail(source)
                                .undo(journal::UndoKind::DelayedStart, vec![key.clone(), task]),
                        );
                        window.push_notification(
                            Notification::success(format!("«{title}» будет запускаться через минуту после входа — рабочий стол появится быстрее")),
                            cx,
                        );
                    }
                    Ok(None) => {
                        journal::add(journal::Entry::new("startup", format!("«{title}» снова запускается сразу")).detail(source));
                        window.push_notification(Notification::info(format!("«{title}» будет запускаться сразу при входе")), cx);
                    }
                    Err(e) => ui::error(window, cx, "Не удалось изменить запуск", e),
                }
                if let Some(slot) = this.items.get_mut(ix) {
                    *slot = item;
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.push(task);
    }

    // ---------------- rendering

    fn render_boot(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let up = std::time::Duration::from_millis(unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() });
        let since = chrono::Local::now() - chrono::Duration::from_std(up).unwrap_or_default();
        let uptime = if up.as_secs() >= 86_400 {
            format!("{} д {} ч", up.as_secs() / 86_400, up.as_secs() / 3600 % 24)
        } else if up.as_secs() >= 3600 {
            format!("{} ч {} мин", up.as_secs() / 3600, up.as_secs() / 60 % 60)
        } else {
            format!("{} мин", up.as_secs() / 60)
        };
        let stat = |label: &str, value: String, hint: String| {
            v_flex()
                .gap_0p5()
                .min_w(px(120.))
                .child(div().text_xs().text_color(theme.muted_foreground).child(label.to_string()))
                .child(div().text_lg().font_semibold().child(value))
                .when(!hint.is_empty(), |s| s.child(div().text_xs().text_color(theme.muted_foreground).child(hint)))
        };
        let (big, stats, note, chart): (String, Vec<AnyElement>, Option<String>, Option<AnyElement>) = match &self.boot {
            Boot::Loading => ("…".into(), Vec::new(), Some("Читаем журнал загрузки Windows…".into()), None),
            Boot::NeedsAdmin => (
                "—".into(),
                Vec::new(),
                Some("Windows открывает журнал загрузки только администраторам — перезапустите Простор от имени администратора.".into()),
                None,
            ),
            Boot::Empty => ("—".into(), Vec::new(), Some("Windows ещё не записала ни одной загрузки. Данные появятся после перезагрузки компьютера.".into()), None),
            Boot::Ready(b) => {
                let last = b.last().unwrap();
                let avg = b.iter().map(|x| x.main_path_seconds).sum::<f64>() / b.len() as f64;
                let note = ((chrono::Local::now() - last.time).num_days() > 30).then(|| {
                    format!(
                        "Последняя запись — {}: при включённом «быстром запуске» Windows записывает только полные перезагрузки.",
                        last.time.format("%d.%m.%Y")
                    )
                });
                let stats = vec![
                    stat("В среднем", seconds(avg), format!("за {}", fmt::count(b.len() as u64, "загрузку", "загрузки", "загрузок"))).into_any_element(),
                    stat("С автозагрузкой", seconds(last.total_seconds), fmt::count(last.startup_apps as u64, "программа", "программы", "программ")).into_any_element(),
                ];
                (seconds(last.main_path_seconds), stats, note, Some(boot_chart(b, cx).into_any_element()))
            }
        };
        let mut stats = stats;
        stats.push(stat("Работает без перезагрузки", uptime, format!("с {}", since.format("%d.%m %H:%M"))).into_any_element());
        ui::card(cx).p_5().gap_4().child(
            h_flex()
                .gap_6()
                .items_center()
                .flex_wrap()
                .child(
                    h_flex()
                        .gap_4()
                        .items_center()
                        .child(ui::icon_tile(Lucide::Timer, Tone::Accent, 48., cx))
                        .child(
                            v_flex()
                                .child(div().text_xs().text_color(theme.muted_foreground).child("Загрузка Windows"))
                                .child(div().text_size(px(30.)).line_height(px(36.)).font_semibold().child(big))
                                .child(div().text_xs().text_color(theme.muted_foreground).child("до рабочего стола")),
                        ),
                )
                .child(div().w(px(1.)).h(px(56.)).bg(ui::card_border(&theme)))
                .child(h_flex().gap_6().flex_wrap().children(stats))
                .child(div().flex_1())
                .when_some(chart, |s, c| s.child(c)),
        )
        .when_some(note, |s, n| s.child(div().text_xs().text_color(theme.muted_foreground).child(n)))
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let item = &self.items[ix];
        let can = item.can_modify();
        let busy = self.busy.as_deref() == Some(&item.key());
        let state_tone = if !item.enabled { Tone::Neutral } else if item.is_delayed() { Tone::Info } else { Tone::Success };
        let path = item.file_path.clone();
        let search_name = item.name.clone();
        h_flex()
            .id(("startup-row", ix))
            .gap_3()
            .px_3()
            .py_2p5()
            .rounded_lg()
            .hover(|s| s.bg(theme.foreground.opacity(0.04)))
            .child(ui::app_icon(item.file_path.as_deref(), &item.title(), 32., cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().font_medium().truncate().child(item.title()))
                            .child(ui::badge(item.state_text(), state_tone, cx)),
                    )
                    .child(div().text_xs().text_color(theme.muted_foreground).truncate().child({
                        let sub = item.subtitle();
                        if sub.is_empty() { item.source.to_string() } else { format!("{sub}  ·  {}", item.source) }
                    }))
                    .when_some(item.lock_hint(), |s, h| {
                        s.child(
                            h_flex()
                                .gap_1()
                                .text_xs()
                                .text_color(theme.warning)
                                .child(Icon::new(Lucide::Lock).size_3())
                                .child(h),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_shrink_0()
                    .when(item.can_delay() && item.enabled, |s| {
                        s.child(
                            Button::new(("delay", ix))
                                .ghost()
                                .small()
                                .icon(Icon::new(if item.is_delayed() { Lucide::Zap } else { Lucide::Hourglass }))
                                .tooltip(if item.is_delayed() { "Запускать сразу при входе" } else { "Запускать через минуту после входа" })
                                .disabled(!can)
                                .on_click(cx.listener(move |this, _, window, cx| this.toggle_delay(ix, window, cx))),
                        )
                    })
                    .child(
                        Button::new(("open", ix))
                            .ghost()
                            .small()
                            .icon(Icon::new(Lucide::FolderOpen))
                            .tooltip("Открыть расположение")
                            .disabled(path.is_none())
                            .on_click(move |_, _, _| {
                                if let Some(p) = &path {
                                    shell::show_in_explorer(p);
                                }
                            }),
                    )
                    .child(
                        Button::new(("web", ix))
                            .ghost()
                            .small()
                            .icon(Icon::new(Lucide::Search))
                            .tooltip("Что это? Поиск в интернете")
                            .on_click(move |_, _, _| shell::search_web(&format!("{search_name} в автозагрузке что это"))),
                    )
                    .child(div().w(px(8.)))
                    .child(if busy {
                        Spinner::new().small().into_any_element()
                    } else {
                        Switch::new(("switch", ix))
                            .checked(item.enabled)
                            .disabled(!can)
                            .on_click(cx.listener(move |this, checked: &bool, window, cx| this.toggle(ix, *checked, window, cx)))
                            .into_any_element()
                    }),
            )
    }
}

fn boot_chart(boots: &[BootRecord], cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let max = boots.iter().map(|b| b.main_path_seconds).fold(1.0, f64::max);
    let mut row = h_flex().gap_1().items_end().h(px(64.)).flex_shrink_0();
    for (i, b) in boots.iter().enumerate() {
        let h = (b.main_path_seconds / max * 60.0).max(3.0) as f32;
        let last = i == boots.len() - 1;
        row = row.child(
            div()
                .id(("boot", i))
                .w(px(9.))
                .h(px(h))
                .rounded_t(px(3.))
                .bg(if last { theme.primary } else { theme.primary.opacity(0.35) })
                .tooltip({
                    let text = format!(
                        "{} — {} до рабочего стола, {} вместе с автозагрузкой ({} прогр.)",
                        b.time.format("%d.%m.%Y %H:%M"),
                        seconds(b.main_path_seconds),
                        seconds(b.total_seconds),
                        b.startup_apps
                    );
                    move |window, cx| gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx)
                }),
        );
    }
    row
}

impl Render for StartupView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let count = |f: fn(&Item) -> bool| self.items.iter().filter(|i| f(i)).count();
        let (all, progs, tasks, services) =
            (self.items.len(), count(|i| matches!(i.kind, Kind::Run | Kind::Folder)), count(|i| i.kind == Kind::Task), count(|i| i.kind == Kind::Service));
        let options = vec![
            (format!("Все · {all}"), self.filter == Filter::All),
            (format!("Программы · {progs}"), self.filter == Filter::Programs),
            (format!("Задачи · {tasks}"), self.filter == Filter::Tasks),
            (format!("Службы · {services}"), self.filter == Filter::Services),
        ];
        let filters = ui::chips(
            "startup-filter",
            &options,
            |this: &mut Self, i, cx| {
                this.filter = [Filter::All, Filter::Programs, Filter::Tasks, Filter::Services][i];
                cx.notify();
            },
            cx,
        );

        let summary = if self.items.is_empty() {
            if self.loading { "Собираем список…".to_string() } else { "Записей автозагрузки не найдено".into() }
        } else {
            format!("Включено {} из {}: программы, задачи планировщика и сторонние службы", self.enabled_count(), self.items.len())
        };

        let mut body = v_flex()
            .gap_3()
            .child(self.render_boot(cx))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(filters)
                    .child(div().flex_1())
                    .child(div().w(px(300.)).child(Input::new(&self.search).cleanable(true).prefix(Icon::new(Lucide::Search).size_4()))),
            )
            .child(div().text_sm().text_color(theme.muted_foreground).child(summary));

        let visible = self.visible();
        if self.loading && self.items.is_empty() {
            body = body.child(ui::skeleton_rows(7, 32., cx));
        } else if visible.is_empty() && !self.items.is_empty() {
            body = body.child(ui::banner(Tone::Neutral, Lucide::Search, "Ничего не нашлось", None, cx));
        }
        for group in ["Программы", "Задачи планировщика", "Службы"] {
            let rows: Vec<usize> = visible.iter().copied().filter(|&ix| self.items[ix].group() == group).collect();
            if rows.is_empty() {
                continue;
            }
            body = body.child(ui::section_title(format!("{group} · {}", rows.len()), None, cx));
            let mut card = ui::card(cx).p_1p5();
            for ix in rows {
                card = card.child(self.render_row(ix, cx));
            }
            body = body.child(card);
        }

        ui::page(
            "startup-page",
            "Автозагрузка",
            &if self.loading && self.items.is_empty() {
                "Собираем программы, задачи и службы, которые запускаются вместе с Windows…".to_string()
            } else {
                format!("{} · Отключённые программы не удаляются — их можно включить обратно в любой момент.", fmt::count(self.enabled_count() as u64, "запись включена", "записи включены", "записей включено"))
            },
            Button::new("refresh")
                .outline()
                .icon(Icon::new(Lucide::RefreshCw))
                .label("Обновить")
                .disabled(self.loading)
                .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
            body,
            cx,
        )
    }
}
