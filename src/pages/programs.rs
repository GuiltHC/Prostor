//! "Программы" page (port of ViewModels/ProgramsViewModel.cs + Views/ProgramsView.xaml).

use crate::data::{journal, settings};
use crate::fmt;
use crate::services::history;
use crate::services::programs::advisor::Hint;
use crate::services::programs::inventory::{self, App};
use crate::services::programs::leftovers::{self, Leftover};
use crate::services::programs::uninstall::{self, RestorePoint, StartError};
use crate::services::programs::{process, usage, winget};
use crate::sys::shell;
use crate::ui::ask::{self, Ask, Kind as Btn, Options, Pick, Picks, Stack};
use crate::ui::{self, Busy, Lucide, Tone};
use chrono::{DateTime, Local};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Progress, current line and result of a winget update running in the background.
type UpdateState = (Option<f32>, String, Option<Result<bool, String>>);

const LARGE: u64 = 1 << 30;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Desktop,
    Store,
    Games,
    Large,
    Recent,
    Unused,
    Updates,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Size,
    Date,
    LastUsed,
    Name,
}

impl Sort {
    fn name(self) -> &'static str {
        match self {
            Sort::Size => "Size",
            Sort::Date => "Date",
            Sort::LastUsed => "LastUsed",
            Sort::Name => "Name",
        }
    }
    fn from(s: &str) -> Sort {
        match s {
            "Date" => Sort::Date,
            "LastUsed" => Sort::LastUsed,
            "Name" => Sort::Name,
            _ => Sort::Size,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    Removed,
    Kept,
    EntryRemoved,
}

pub struct ProgramsView {
    apps: Vec<App>,
    loading: bool,
    generation: u64,
    filter: Filter,
    sort: Sort,
    search: Entity<InputState>,
    query: String,
    selected: Option<String>,
    checked: HashSet<String>,
    usage_ready: bool,
    has_prefetch: bool,
    checking_updates: bool,
    busy: bool,
    /// Programs removed by the forced removal during this run: their leftovers are already gone.
    forced: HashSet<String>,
    _tasks: Vec<Task<()>>,
    _subs: Vec<Subscription>,
}

fn hint_tone(h: Hint) -> Tone {
    match h {
        Hint::Dependency | Hint::Driver => Tone::Warning,
        Hint::System => Tone::Danger,
        Hint::Preinstalled => Tone::Success,
        Hint::None => Tone::Neutral,
    }
}

fn ago(d: Option<DateTime<Local>>) -> String {
    let Some(d) = d else { return "—".into() };
    let days = (Local::now().date_naive() - d.date_naive()).num_days();
    if days < 1 {
        "сегодня".into()
    } else if days < 2 {
        "вчера".into()
    } else if days < 30 {
        format!("{days} {} назад", fmt::plural(days as u64, "день", "дня", "дней"))
    } else if days < 365 {
        let m = ((days as f64 / 30.4) as u64).max(1);
        format!("{m} {} назад", fmt::plural(m, "месяц", "месяца", "месяцев"))
    } else {
        let y = ((days as f64 / 365.25) as u64).max(1);
        format!("{y} {} назад", fmt::plural(y, "год", "года", "лет"))
    }
}

fn date(d: Option<DateTime<Local>>) -> String {
    d.map(|d| fmt::filetime_date((d.timestamp() as u64 + 11_644_473_600) * 10_000_000)).unwrap_or_else(|| "—".into())
}

fn launcher_hint(app: &App) -> String {
    let l = app.launcher_name().unwrap_or("лаунчер");
    if app.launcher_manifest.is_some() {
        format!("Откроется библиотека {l} — удалите игру там: «…» у игры → «Управление» → «Удалить».")
    } else {
        format!("Игру удалит {l} — подтвердите удаление в его окне.")
    }
}

impl ProgramsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск по названию или издателю"));
        let sub = cx.subscribe(&search, |this: &mut Self, state, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.query = state.read(cx).value().to_string();
                cx.notify();
            }
        });
        let sort = Sort::from(settings::get().programs_sort.as_deref().unwrap_or("Size"));
        let mut v = Self {
            apps: Vec::new(),
            loading: false,
            generation: 0,
            filter: Filter::All,
            sort,
            search,
            query: String::new(),
            selected: None,
            checked: HashSet::new(),
            usage_ready: false,
            has_prefetch: false,
            checking_updates: false,
            busy: false,
            forced: HashSet::new(),
            _tasks: Vec::new(),
            _subs: vec![sub],
        };
        v.load(cx);
        v
    }

    pub fn total_size(&self) -> u64 {
        self.apps.iter().filter_map(|a| a.size).sum()
    }
    pub fn count(&self) -> usize {
        self.apps.len()
    }
    pub fn unused(&self) -> (usize, u64) {
        let u: Vec<&App> = self.apps.iter().filter(|a| a.unused).collect();
        (u.len(), u.iter().filter_map(|a| a.size).sum())
    }
    pub fn updates(&self) -> usize {
        self.apps.iter().filter(|a| a.has_update()).count()
    }
    pub fn largest(&self, n: usize) -> Vec<(String, u64, Option<std::path::PathBuf>)> {
        let mut v: Vec<&App> = self.apps.iter().filter(|a| a.size.is_some()).collect();
        v.sort_by_key(|a| std::cmp::Reverse(a.size));
        v.into_iter().take(n).map(|a| (a.name.clone(), a.size.unwrap_or(0), a.icon_source())).collect()
    }

    /// Opens the page focused on one program (from the dashboard).
    pub fn select_by_name(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(a) = self.apps.iter().find(|a| a.name == name) {
            self.filter = Filter::All;
            self.selected = Some(a.id.clone());
            cx.notify();
        }
    }

    pub fn show_unused(&mut self, cx: &mut Context<Self>) {
        self.filter = Filter::Unused;
        cx.notify();
    }

    pub fn show_updates(&mut self, cx: &mut Context<Self>) {
        self.filter = Filter::Updates;
        cx.notify();
    }

    // ---------------- loading

    pub fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.generation += 1;
        let generation = self.generation;
        cx.notify();
        let work = cx.background_executor().spawn(async { inventory::load_all() });
        let task = cx.spawn(async move |this, cx| {
            let mut apps = work.await;
            apps.sort_by_key(|a| std::cmp::Reverse(a.size.unwrap_or(0)));
            let snapshot = apps.clone();
            let ok = this
                .update(cx, |this, cx| {
                    if this.generation != generation {
                        return false;
                    }
                    let ids: HashSet<&str> = apps.iter().map(|a| a.id.as_str()).collect();
                    this.checked.retain(|c| ids.contains(c.as_str()));
                    if this.selected.as_ref().is_some_and(|s| !ids.contains(s.as_str())) {
                        this.selected = None;
                    }
                    this.apps = apps;
                    this.loading = false;
                    let icons = ui::preload_icons(this.apps.iter().filter_map(|a| a.icon_source()).collect(), cx);
                    this._tasks.push(icons);
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !ok {
                return;
            }

            // sizes of programs whose installer did not report one, Store apps and games
            let measure = snapshot.clone();
            let sizes = cx.background_executor().spawn(async move {
                use rayon::prelude::*;
                let locations: Vec<String> = measure
                    .iter()
                    .filter_map(|a| a.install_location.as_ref())
                    .chain(measure.iter().flat_map(|a| a.extra_locations.iter()))
                    .map(|p| p.to_string_lossy().trim_end_matches('\\').to_lowercase())
                    .collect();
                let pool = rayon::ThreadPoolBuilder::new().num_threads(3).build().ok();
                let run = || {
                    measure
                        .par_iter()
                        .filter(|a| a.is_store || a.is_game() || (a.size.is_none() && a.install_location.is_some()))
                        .map(|a| (a.id.clone(), inventory::measure_size(a, &locations)))
                        .filter(|(_, s)| *s > 0)
                        .collect::<Vec<_>>()
                };
                match pool {
                    Some(p) => p.install(run),
                    None => run(),
                }
            });
            let usage_snapshot = snapshot.clone();
            let usage_work = cx.background_executor().spawn(async move {
                let data = usage::load();
                let res: Vec<(String, Option<DateTime<Local>>)> =
                    usage_snapshot.iter().map(|a| (a.id.clone(), usage::last_used(a, &data))).collect();
                (data.has_prefetch(), res)
            });
            let updates_work = cx.background_executor().spawn(async { winget::available().then(winget::upgrades) });
            this.update(cx, |this, cx| {
                this.checking_updates = winget::available();
                cx.notify();
            })
            .ok();

            let (prefetch, used) = usage_work.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                for (id, t) in used {
                    if let Some(a) = this.apps.iter_mut().find(|a| a.id == id) {
                        a.last_used = t;
                    }
                }
                this.has_prefetch = prefetch;
                this.usage_ready = true;
                this.refresh_unused();
                cx.notify();
            })
            .ok();

            let sizes = sizes.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                for (id, s) in sizes {
                    if let Some(a) = this.apps.iter_mut().find(|a| a.id == id) {
                        a.size = Some(s);
                        a.size_measured = true;
                    }
                }
                cx.notify();
            })
            .ok();

            let ups = updates_work.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.checking_updates = false;
                if let Some(u) = ups {
                    winget::apply(&mut this.apps, &u);
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks = vec![task];
    }

    /// Re-evaluates "unused" after usage data arrived or the threshold changed.
    pub fn refresh_unused(&mut self) {
        let months = settings::get().unused_months.max(1) as i64;
        let threshold = Local::now() - chrono::Duration::days(months * 30);
        for a in &mut self.apps {
            a.unused = a.last_used.is_some_and(|t| t < threshold) && a.install_date.is_none_or(|d| d <= threshold) && !a.hint.risky();
        }
    }

    fn months_text() -> String {
        match settings::get().unused_months {
            12 => "года".into(),
            6 => "полугода".into(),
            m => format!("{m} {}", fmt::plural(m.max(0) as u64, "месяца", "месяцев", "месяцев")),
        }
    }

    fn visible(&self) -> Vec<usize> {
        let q = self.query.trim().to_lowercase();
        let recent = Local::now() - chrono::Duration::days(30);
        let mut v: Vec<usize> = self
            .apps
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                (q.is_empty() || a.name.to_lowercase().contains(&q) || a.publisher.as_ref().is_some_and(|p| p.to_lowercase().contains(&q)))
                    && match self.filter {
                        Filter::All => true,
                        Filter::Desktop => !a.is_store && !a.is_game(),
                        Filter::Store => a.is_store && !a.is_game(),
                        Filter::Games => a.is_game(),
                        Filter::Large => a.size.unwrap_or(0) >= LARGE,
                        Filter::Recent => a.install_date.is_some_and(|d| d >= recent),
                        Filter::Unused => a.unused,
                        Filter::Updates => a.has_update(),
                    }
            })
            .map(|(i, _)| i)
            .collect();
        let apps = &self.apps;
        match self.sort {
            Sort::Size => v.sort_by(|&a, &b| apps[b].size.cmp(&apps[a].size).then_with(|| apps[a].name.to_lowercase().cmp(&apps[b].name.to_lowercase()))),
            Sort::Date => v.sort_by(|&a, &b| apps[b].install_date.cmp(&apps[a].install_date)),
            // programs without launch data go last
            Sort::LastUsed => v.sort_by_key(|&i| (apps[i].last_used.is_none(), apps[i].last_used)),
            Sort::Name => v.sort_by_key(|&i| apps[i].name.to_lowercase()),
        }
        v
    }

    fn app(&self, id: &str) -> Option<&App> {
        self.apps.iter().find(|a| a.id == id)
    }

    fn remove_from_list(&mut self, ids: &[String], cx: &mut Context<Self>) {
        self.apps.retain(|a| !ids.contains(&a.id));
        for id in ids {
            self.checked.remove(id);
        }
        if self.selected.as_ref().is_some_and(|s| ids.contains(s)) {
            self.selected = None;
        }
        cx.notify();
    }

    // ---------------- actions

    /// Ctrl+F.
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Delete: the ticked programs, or the one open in the details panel.
    pub fn uninstall_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<String> = if self.checked.is_empty() { self.selected.clone().into_iter().collect() } else { self.checked.iter().cloned().collect() };
        let ids: Vec<String> = ids.into_iter().filter(|id| self.app(id).is_some_and(|a| a.can_uninstall())).collect();
        if !ids.is_empty() {
            self.start_uninstall(ids, window, cx);
        }
    }

    fn start_uninstall(&mut self, ids: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let apps: Vec<App> = ids.iter().filter_map(|id| self.app(id).cloned()).collect();
        if apps.is_empty() {
            return;
        }
        let all = self.apps.clone();
        let t = cx.spawn_in(window, async move |this, cx| uninstall_flow(this, apps, all, cx).await);
        self._tasks.push(t);
    }

    fn start_force(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(app) = self.app(&id).cloned() else { return };
        let all = self.apps.clone();
        let t = cx.spawn_in(window, async move |this, cx| {
            this.update(cx, |v, cx| {
                v.busy = true;
                cx.notify();
            })
            .ok();
            force_uninstall(&this, &app, &all, None, false, cx).await;
            this.update(cx, |v, cx| {
                v.busy = false;
                v.forced.clear();
                cx.notify();
            })
            .ok();
        });
        self._tasks.push(t);
    }

    fn start_remove_entry(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(app) = self.app(&id).cloned() else { return };
        let t = cx.spawn_in(window, async move |this, cx| {
            if offer_remove_entry(&app, false, cx).await == Outcome::EntryRemoved {
                this.update(cx, |v, cx| v.remove_from_list(std::slice::from_ref(&app.id), cx)).ok();
            }
        });
        self._tasks.push(t);
    }

    fn start_update(&mut self, ids: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let apps: Vec<App> = ids.iter().filter_map(|id| self.app(id).cloned()).filter(|a| a.winget_id.is_some()).collect();
        if apps.is_empty() {
            return;
        }
        let t = cx.spawn_in(window, async move |this, cx| update_flow(this, apps, cx).await);
        self._tasks.push(t);
    }

    fn start_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let apps = self.apps.clone();
        let t = cx.spawn_in(window, async move |_, cx| export_flow(apps, cx).await);
        self._tasks.push(t);
    }

    // ---------------- rendering

    fn render_filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let recent = Local::now() - chrono::Duration::days(30);
        let c = |f: &dyn Fn(&App) -> bool| self.apps.iter().filter(|a| f(a)).count();
        let options: Vec<(Filter, String)> = vec![
            (Filter::All, format!("Все · {}", self.apps.len())),
            (Filter::Desktop, format!("Классические · {}", c(&|a| !a.is_store && !a.is_game()))),
            (Filter::Store, format!("Store · {}", c(&|a| a.is_store && !a.is_game()))),
            (Filter::Games, format!("Игры · {}", c(&|a| a.is_game()))),
            (Filter::Large, format!("Крупные · {}", c(&|a| a.size.unwrap_or(0) >= LARGE))),
            (Filter::Recent, format!("Новые · {}", c(&|a| a.install_date.is_some_and(|d| d >= recent)))),
            (Filter::Unused, format!("Не используются · {}", c(&|a| a.unused))),
            (Filter::Updates, format!("Обновления · {}", c(&|a| a.has_update()))),
        ];
        let opts: Vec<(String, bool)> = options.iter().map(|(f, l)| (l.clone(), *f == self.filter)).collect();
        let keys: Vec<Filter> = options.iter().map(|(f, _)| *f).collect();
        ui::chips(
            "programs-filter",
            &opts,
            move |this: &mut Self, i, cx| {
                this.filter = keys[i];
                cx.notify();
            },
            cx,
        )
        .flex_wrap()
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let a = &self.apps[ix];
        let id = a.id.clone();
        let id2 = a.id.clone();
        let selected = self.selected.as_deref() == Some(&a.id);
        let checked = self.checked.contains(&a.id);
        let max = self.apps.iter().filter_map(|a| a.size).max().unwrap_or(1).max(1);
        let show_last_used = self.sort == Sort::LastUsed || self.filter == Filter::Unused;
        // with the details panel open the row is narrow: the name matters more than the date
        let compact = self.selected.is_some();
        h_flex()
            .id(("app-row", ix))
            .gap_3()
            .px_3()
            .py_2()
            .rounded_lg()
            .cursor_pointer()
            .when(selected, |s| s.bg(theme.list_active))
            .when(!selected, |s| s.hover(|s| s.bg(ui::row_hover(&theme))))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = Some(id.clone());
                cx.notify();
            }))
            .child(Checkbox::new(("app-chk", ix)).checked(checked).on_click(cx.listener(move |this, c: &bool, _, cx| {
                if *c {
                    this.checked.insert(id2.clone());
                } else {
                    this.checked.remove(&id2);
                }
                cx.notify();
            })))
            .child(ui::app_icon(a.icon_source().as_deref(), &a.name, 32., cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().font_medium().truncate().child(a.name.clone()))
                            .when_some(a.hint.label(), |s, l| s.child(ui::badge(l, hint_tone(a.hint), cx)))
                            .when(a.has_update(), |s| s.child(ui::badge(format!("→ {}", a.update_version.clone().unwrap_or_default()), Tone::Info, cx))),
                    )
                    .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(a.subtitle())),
            )
            .child(
                v_flex()
                    .w(px(if compact { 80. } else { 130. }))
                    .flex_shrink_0()
                    .gap_1()
                    .items_end()
                    .child(div().text_sm().font_medium().child(a.size.map(fmt::size).unwrap_or_else(|| "—".into())))
                    .child(ui::bar(a.size.unwrap_or(0) as f32 / max as f32, theme.primary.opacity(0.7), cx).h(px(4.))),
            )
            .when(!compact, |s| {
                s.child(
                    div()
                        .w(px(110.))
                        .flex_shrink_0()
                        .text_right()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(if show_last_used { ago(a.last_used) } else { date(a.install_date) }),
                )
            })
    }

    fn render_details(&self, a: &App, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let id = a.id.clone();
        // label on the left, value on the right, hairlines between: an "info" sheet
        let facts_card = |facts: Vec<(&'static str, String)>| {
            let mut list = v_flex().px_3().rounded_lg().bg(theme.foreground.opacity(if theme.is_dark() { 0.03 } else { 0.025 }));
            for (i, (label, value)) in facts.into_iter().enumerate() {
                list = list.child(
                    h_flex()
                        .gap_3()
                        .py_2()
                        .items_start()
                        .when(i > 0, |s| s.border_t_1().border_color(ui::card_border(&theme)))
                        .child(div().w(px(104.)).flex_shrink_0().text_xs().text_color(theme.muted_foreground).pt(px(1.)).child(label))
                        .child(div().flex_1().min_w_0().text_sm().text_right().child(value)),
                );
            }
            list
        };
        let stat = |label: &str, value: String, hint: String| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .p_3()
                .rounded_lg()
                .bg(theme.foreground.opacity(if theme.is_dark() { 0.03 } else { 0.025 }))
                .child(div().text_xs().text_color(theme.muted_foreground).child(label.to_string()))
                .child(div().text_lg().font_semibold().truncate().child(value))
                .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(hint))
        };
        let size_note = match (a.size, a.size_measured) {
            (None, _) => "неизвестен",
            (Some(_), true) => "по папке программы",
            (Some(_), false) => "по данным установщика",
        };
        let location = a.install_location.clone();
        let reg = a.registry_full_path();
        let name = a.name.clone();
        let pfn = a.package_family_name.clone();
        let help = a.help_link.clone();
        let missing = uninstall::uninstaller_missing(a);
        let can_force = !a.is_store && !a.is_launcher_uninstall() && (a.has_registry_key() || a.install_location.is_some());

        let (i1, i2, i3, i4, i5, i6) = (id.clone(), id.clone(), id.clone(), id.clone(), id.clone(), name.clone());
        ui::card(cx).w_full().p_5().gap_4().child(
            v_flex()
                .gap_4()
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(ui::app_icon(a.icon_source().as_deref(), &a.name, 48., cx))
                        .child(
                            v_flex()
                                .min_w_0()
                                .child(div().text_lg().font_semibold().child(a.name.clone()))
                                .child(div().text_xs().text_color(theme.muted_foreground).child(a.kind_text())),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(stat("Размер", a.size.map(fmt::size).unwrap_or_else(|| "—".into()), size_note.to_string()))
                        .child(stat(
                            "Последний запуск",
                            a.last_used.map(|d| ago(Some(d))).unwrap_or_else(|| "—".into()),
                            a.last_used.map(|d| date(Some(d))).unwrap_or_else(|| if self.usage_ready { "нет данных".into() } else { "собираем данные…".into() }),
                        )),
                )
                .when_some(a.hint.explanation(), |s, e| {
                    s.child(ui::banner(hint_tone(a.hint), Lucide::Info, format!("{}. {e}", a.hint.label().unwrap_or_default()), None, cx))
                })
                .when(a.unused, |s| {
                    s.child(ui::banner(
                        Tone::Info,
                        Lucide::Clock,
                        format!("Не запускалась {} — возможно, она больше не нужна.", ago(a.last_used).replace(" назад", "")),
                        None,
                        cx,
                    ))
                })
                .when(missing, |s| {
                    s.child(ui::banner(Tone::Warning, Lucide::TriangleAlert, "Файл деинсталлятора не найден — программу, вероятно, уже удалили вручную.", None, cx))
                })
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Button::new("d-uninstall")
                                .danger()
                                .icon(Icon::new(Lucide::Trash))
                                .label(if a.is_launcher_uninstall() { format!("Удалить через {}", a.launcher_name().unwrap_or_default()) } else { "Удалить".into() })
                                .disabled(!a.can_uninstall() || self.busy)
                                .on_click(cx.listener(move |this, _, window, cx| this.start_uninstall(vec![i1.clone()], window, cx))),
                        )
                        .when(a.has_update(), |s| {
                            s.child(
                                Button::new("d-update")
                                    .primary()
                                    .icon(Icon::new(Lucide::Download))
                                    .label(format!("Обновить до {}", a.update_version.clone().unwrap_or_default()))
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, window, cx| this.start_update(vec![i2.clone()], window, cx))),
                            )
                        })
                        .when(a.can_modify(), |s| {
                            let app = a.clone();
                            s.child(Button::new("d-modify").outline().label("Изменить").on_click(move |_, window, cx| {
                                if let Err(e) = uninstall::start_modify(&app) {
                                    ui::error(window, cx, "Не удалось запустить установщик", format!("{e:?}"));
                                }
                            }))
                        }),
                )
                .child(facts_card({
                    let mut f: Vec<(&'static str, String)> = vec![(
                        "Установлена",
                        a.install_date.map(|d| format!("{} ({})", date(Some(d)), ago(Some(d)))).unwrap_or_else(|| "Неизвестно".into()),
                    )];
                    if let Some(p) = a.publisher.clone() {
                        f.push(("Издатель", p));
                    }
                    if let Some(v) = a.version.clone() {
                        f.push(("Версия", v));
                    }
                    f.push(("Источник", a.source_text()));
                    if let Some(l) = a.install_location.clone() {
                        f.push(("Папка", l.display().to_string()));
                    }
                    f
                }))
                .child(
                    h_flex()
                        .gap_1()
                        .flex_wrap()
                        .child(
                            Button::new("d-folder")
                                .ghost()
                                .small()
                                .icon(Icon::new(Lucide::FolderOpen))
                                .label("Папка")
                                .disabled(location.is_none())
                                .on_click(move |_, _, _| {
                                    if let Some(l) = &location {
                                        shell::open_folder(l);
                                    }
                                }),
                        )
                        .when_some(reg, |s, r| {
                            s.child(Button::new("d-reg").ghost().small().icon(Icon::new(Lucide::Database)).label("Реестр").on_click(move |_, _, _| shell::open_regedit(&r)))
                        })
                        .child(Button::new("d-web").ghost().small().icon(Icon::new(Lucide::Search)).label("В интернете").on_click(move |_, _, _| shell::search_web(&i6)))
                        .child(Button::new("d-copy").ghost().small().icon(Icon::new(Lucide::Copy)).label("Название").on_click({
                            let n = name.clone();
                            move |_, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(n.clone()));
                                window.push_notification(Notification::info("Название скопировано"), cx);
                            }
                        }))
                        .when_some(pfn.filter(|_| a.is_store), |s, p| {
                            s.child(Button::new("d-store").ghost().small().icon(Icon::new(Lucide::Store)).label("Store").on_click(move |_, _, _| {
                                shell::open(&format!("ms-windows-store://pdp/?PFN={p}"));
                            }))
                        })
                        .when_some(help, |s, h| {
                            s.child(Button::new("d-site").ghost().small().icon(Icon::new(Lucide::ExternalLink)).label("Сайт").on_click(move |_, _, _| {
                                shell::open(&h);
                            }))
                        }),
                )
                .when(a.has_registry_key() || can_force, |s| {
                    s.child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .when(can_force, |s| {
                                s.child(
                                    Button::new("d-force")
                                        .ghost()
                                        .small()
                                        .icon(Icon::new(Lucide::Shredder))
                                        .label("Удалить принудительно")
                                        .disabled(self.busy)
                                        .on_click(cx.listener(move |this, _, window, cx| this.start_force(i3.clone(), window, cx))),
                                )
                            })
                            .when(a.has_registry_key(), |s| {
                                s.child(
                                    Button::new("d-entry")
                                        .ghost()
                                        .small()
                                        .icon(Icon::new(Lucide::ListX))
                                        .label("Убрать из списка")
                                        .on_click(cx.listener(move |this, _, window, cx| this.start_remove_entry(i4.clone(), window, cx))),
                                )
                            }),
                    )
                })
                .child(div().text_xs().text_color(theme.muted_foreground.opacity(0.7)).truncate().child(format!("ID: {i5}"))),
        )
    }
}

impl Render for ProgramsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // a narrow window keeps more room for the list
        let panel_w = if window.viewport_size().width < px(1150.) { 316. } else { 360. };
        let theme = cx.theme().clone();
        let visible = self.visible();
        let summary = if self.apps.is_empty() {
            "Загружаем список установленных программ…".to_string()
        } else {
            format!("{} · занимают ≈ {}", fmt::count(self.apps.len() as u64, "программа", "программы", "программ"), fmt::size(self.total_size()))
        };

        let sort_opts: Vec<(String, bool)> = [(Sort::Size, "По размеру"), (Sort::Date, "По дате"), (Sort::LastUsed, "По запуску"), (Sort::Name, "По имени")]
            .iter()
            .map(|(s, l)| (l.to_string(), *s == self.sort))
            .collect();
        let sorter = ui::segmented(
            "programs-sort",
            &sort_opts,
            |this: &mut Self, i, cx| {
                this.sort = [Sort::Size, Sort::Date, Sort::LastUsed, Sort::Name][i];
                settings::update(|s| s.programs_sort = Some(this.sort.name().into()));
                cx.notify();
            },
            cx,
        );

        let loading = self.loading && self.apps.is_empty();
        let mut list = v_flex().gap_0p5();
        if !loading && visible.is_empty() {
            list = list.child(ui::empty_state(Lucide::Search, Tone::Neutral, "Ничего не нашлось", "Попробуйте другой фильтр или запрос.", None, cx));
        }
        for ix in visible.iter().copied() {
            list = list.child(self.render_row(ix, cx));
        }

        let mut top = v_flex()
            .gap_3()
            .child(self.render_filters(cx))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().w(px(320.)).child(Input::new(&self.search).cleanable(true).prefix(Icon::new(Lucide::Search).size_4())))
                    .child(div().flex_1())
                    .when(self.checking_updates, |s| {
                        s.child(h_flex().gap_2().text_xs().text_color(theme.muted_foreground).child(Spinner::new().small()).child("Проверяем обновления…"))
                    })
                    .child(sorter),
            );
        if self.filter == Filter::Unused {
            let (n, size) = self.unused();
            let text = if n == 0 {
                if self.usage_ready { "Все программы запускались недавно — или Windows не записала их запуски.".to_string() } else { "Собираем данные о запусках…".into() }
            } else {
                format!(
                    "{} не запускались дольше {} · {}. Данные {}.",
                    fmt::count(n as u64, "программа", "программы", "программ"),
                    Self::months_text(),
                    fmt::size(size),
                    if self.has_prefetch { "по журналу запусков Windows и меню «Пуск»" } else { "по запускам из меню «Пуск», панели задач и Проводника; в режиме администратора — точнее" }
                )
            };
            top = top.child(ui::banner(Tone::Info, Lucide::Clock, text, None, cx));
        }
        if self.filter == Filter::Updates && self.updates() > 0 {
            let ids: Vec<String> = self.apps.iter().filter(|a| a.has_update()).map(|a| a.id.clone()).collect();
            top = top.child(ui::banner(
                Tone::Accent,
                Lucide::Download,
                format!("Доступно {} через winget", fmt::count(self.updates() as u64, "обновление", "обновления", "обновлений")),
                Some(
                    Button::new("update-all")
                        .primary()
                        .small()
                        .label("Обновить всё")
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, window, cx| this.start_update(ids.clone(), window, cx)))
                        .into_any_element(),
                ),
                cx,
            ));
        }

        let selected = self.selected.as_ref().and_then(|s| self.app(s)).cloned();
        let checked_ids: Vec<String> = self.apps.iter().filter(|a| self.checked.contains(&a.id)).map(|a| a.id.clone()).collect();
        let checked_size: u64 = self.apps.iter().filter(|a| self.checked.contains(&a.id)).filter_map(|a| a.size).sum();

        let body = h_flex()
            .flex_1()
            .min_h_0()
            .gap_4()
            .items_start()
            // the scroll wrapper does not take part in the flex layout: size plain boxes, scroll inside them
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(v_flex().id("programs-list").size_full().overflow_y_scrollbar().child(if loading {
                        ui::skeleton_rows(9, 34., cx)
                    } else {
                        ui::card(cx).p_1p5().child(list)
                    })),
            )
            .when_some(selected, |s, a| {
                s.child(
                    div()
                        .w(px(panel_w))
                        .flex_shrink_0()
                        .h_full()
                        .child(div().id("programs-details").size_full().overflow_y_scrollbar().child(self.render_details(&a, cx))),
                )
            });

        let actions = h_flex()
            .gap_2()
            .child(
                Button::new("export")
                    .ghost()
                    .icon(Icon::new(Lucide::FileDown))
                    .label("Экспорт")
                    .disabled(self.apps.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| this.start_export(window, cx))),
            )
            .child(
                Button::new("refresh")
                    .outline()
                    .icon(Icon::new(Lucide::RefreshCw))
                    .label("Обновить")
                    .disabled(self.loading || self.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
            );

        let header = h_flex()
            .gap_4()
            .items_start()
            .justify_between()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_2xl().font_semibold().child("Программы"))
                    .child(div().text_sm().text_color(theme.muted_foreground).child(summary)),
            )
            .child(actions);

        v_flex()
            .size_full()
            .px_8()
            .pt_6()
            .pb_4()
            .gap_4()
            .child(header)
            .child(top)
            .child(body)
            .when(!checked_ids.is_empty(), |s| {
                let ids = checked_ids.clone();
                let upd: Vec<String> = self.apps.iter().filter(|a| self.checked.contains(&a.id) && a.has_update()).map(|a| a.id.clone()).collect();
                s.child(
                    h_flex()
                        .gap_3()
                        .px_4()
                        .py_3()
                        .rounded_xl()
                        .bg(theme.secondary)
                        .border_1()
                        .border_color(theme.border)
                        .child(div().flex_1().font_medium().child(format!(
                            "Выбрано: {}{}",
                            fmt::count(ids.len() as u64, "программа", "программы", "программ"),
                            if checked_size > 0 { format!(" · {}", fmt::size(checked_size)) } else { String::new() }
                        )))
                        .child(Button::new("clear-checks").ghost().label("Снять выбор").on_click(cx.listener(|this, _, _, cx| {
                            this.checked.clear();
                            cx.notify();
                        })))
                        .when(!upd.is_empty(), |s| {
                            s.child(
                                Button::new("update-checked")
                                    .outline()
                                    .label(format!("Обновить ({})", upd.len()))
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, window, cx| this.start_update(upd.clone(), window, cx))),
                            )
                        })
                        .child(
                            Button::new("uninstall-checked")
                                .danger()
                                .icon(Icon::new(Lucide::Trash))
                                .label("Удалить выбранные")
                                .disabled(self.busy)
                                .on_click(cx.listener(move |this, _, window, cx| this.start_uninstall(ids.clone(), window, cx))),
                        ),
                )
            })
    }
}

// ------------------------------------------------------------------ flows (async, with dialogs)

async fn bg<T: Send + 'static>(cx: &mut AsyncWindowContext, f: impl FnOnce() -> T + Send + 'static) -> T {
    cx.background_executor().spawn(async move { f() }).await
}

fn toast(cx: &mut AsyncWindowContext, n: Notification) {
    cx.update(|window, cx| window.push_notification(n, cx)).ok();
}

fn open_busy(cx: &mut AsyncWindowContext, busy: &Entity<Busy>) {
    let b = busy.clone();
    cx.update(|window, cx| ui::open_busy(window, cx, b)).ok();
}

fn close_busy(cx: &mut AsyncWindowContext) {
    cx.update(ui::close_busy).ok();
}

fn set_busy(cx: &mut AsyncWindowContext, busy: &Entity<Busy>, f: impl FnOnce(&mut Busy)) {
    busy.update(cx, |b, cx| {
        f(b);
        cx.notify();
    });
}

async fn uninstall_flow(this: WeakEntity<ProgramsView>, apps: Vec<App>, all: Vec<App>, cx: &mut AsyncWindowContext) {
    let single = apps.len() == 1;
    let desktop = apps.iter().any(|a| !a.is_store && !a.is_launcher_uninstall());
    let any_quiet = apps.iter().any(|a| !a.is_store && a.has_quiet());
    let size: u64 = apps.iter().filter_map(|a| a.size).sum();
    let admin = shell::is_admin();
    let (search_leftovers, restore_default) = {
        let s = settings::get();
        (s.search_leftovers, s.restore_point_before_uninstall)
    };

    let mut message = if single {
        let a = &apps[0];
        if a.is_store {
            "Приложение Microsoft Store будет удалено вместе с его данными. Если оно запущено, Windows его закроет.".to_string()
        } else if a.is_launcher_uninstall() {
            launcher_hint(a)
        } else {
            "Запустится штатный деинсталлятор программы — следуйте его инструкциям.".into()
        }
    } else {
        "Программы будут удаляться по очереди. Для классических программ откроются их деинсталляторы.".into()
    };
    if size > 0 {
        message.push_str(&format!("\nОсвободится примерно {}.", fmt::size(size)));
    }
    if apps.iter().any(|a| !a.is_steam_game() && a.name == "Steam") && all.iter().any(|a| a.is_steam_game()) {
        message.push_str("\n\nВнимание: деинсталлятор Steam удалит и все игры из своей папки.");
    }
    let risky: Vec<&App> = apps.iter().filter(|a| a.hint.risky()).collect();
    if !risky.is_empty() {
        if single {
            message.push_str(&format!("\n\nОсторожно: {}", risky[0].hint.explanation().unwrap_or_default()));
        } else {
            let names: Vec<String> = risky.iter().take(3).map(|a| format!("«{}»", a.name)).collect();
            message.push_str(&format!(
                "\n\nОсторожно: {}{} — драйверы или компоненты, которые нужны другим программам.",
                names.join(", "),
                if risky.len() > 3 { " и др." } else { "" }
            ));
        }
    }

    // options: indices depend on what applies
    let mut opt_ix = (None, None, None);
    let mut options = Options::new();
    let mut n = 0;
    if desktop {
        options = options.add("Найти остатки после удаления", search_leftovers, true);
        opt_ix.0 = Some(n);
        n += 1;
    }
    if any_quiet {
        options = options.add("Тихий режим, если программа его поддерживает", false, true);
        opt_ix.1 = Some(n);
        n += 1;
    }
    if desktop {
        options = options.add(
            if admin { "Создать точку восстановления Windows" } else { "Точка восстановления (нужен режим администратора)" },
            admin && restore_default,
            admin,
        );
        opt_ix.2 = Some(n);
    }
    let options = cx.new(|_| options);
    let picks = (!single).then(|| {
        cx.new(|_| Picks {
            items: apps
                .iter()
                .map(|a| Pick {
                    title: a.name.clone(),
                    subtitle: a.hint.label().map(str::to_string).or_else(|| a.publisher.clone()).unwrap_or_default(),
                    detail: a.size.map(fmt::size).unwrap_or_default(),
                    checked: true,
                })
                .collect(),
        })
    });
    let mut views: Vec<AnyView> = vec![options.clone().into()];
    if let Some(p) = &picks {
        views.insert(0, p.clone().into());
    }
    let stack = cx.new(|_| Stack(views));
    let title = if single { format!("Удалить «{}»?", apps[0].name) } else { format!("Удалить {}?", fmt::count(apps.len() as u64, "программу", "программы", "программ")) };
    let answer = ask::ask(cx, Ask::new((Lucide::Trash, Tone::Danger), title, message).button("Отмена", Btn::Plain).button("Удалить", Btn::Danger).extra(stack)).await;
    if answer != Some(1) {
        return;
    }
    let (leftovers_opt, quiet, restore) = options.read_with(cx, |o, _| {
        (opt_ix.0.is_some_and(|i| o.checked(i)), opt_ix.1.is_some_and(|i| o.checked(i)), opt_ix.2.is_some_and(|i| o.checked(i)))
    });
    if let Some(i) = opt_ix.0 {
        let v = options.read_with(cx, |o, _| o.items[i].checked);
        settings::update(|s| s.search_leftovers = v);
    }
    if opt_ix.2.is_some() && admin {
        settings::update(|s| s.restore_point_before_uninstall = restore);
    }
    let chosen: Vec<App> = match &picks {
        Some(p) => {
            let idx = p.read_with(cx, |p, _| p.checked());
            idx.into_iter().map(|i| apps[i].clone()).collect()
        }
        None => apps.clone(),
    };
    if chosen.is_empty() {
        return;
    }

    this.update(cx, |v, cx| {
        v.busy = true;
        cx.notify();
    })
    .ok();

    if restore && admin && chosen.iter().any(|a| !a.is_store && !a.is_launcher_uninstall()) {
        let desc = if chosen.len() == 1 { format!("Простор: удаление {}", chosen[0].name) } else { format!("Простор: удаление {} программ", chosen.len()) };
        let busy = cx.new(|_| {
            let mut b = Busy::new("Создаём точку восстановления…");
            b.message = "Это займёт до минуты. Если удаление что-то сломает, систему можно будет вернуть к этому состоянию.".into();
            b
        });
        open_busy(cx, &busy);
        let r = bg(cx, move || uninstall::create_restore_point(&desc)).await;
        close_busy(cx);
        match r {
            RestorePoint::Created => toast(cx, Notification::info("Точка восстановления создана")),
            RestorePoint::Recent(m) => toast(cx, Notification::info(m)),
            RestorePoint::Failed(m) => {
                toast(cx, Notification::warning(format!("Точка восстановления не создана: {m}")));
                cx.background_executor().timer(Duration::from_millis(1200)).await;
            }
        }
    }

    let total = chosen.len();
    let mut removed: Vec<App> = Vec::new();
    let mut entries: Vec<String> = Vec::new();
    for (i, app) in chosen.iter().enumerate() {
        let outcome = uninstall_one(&this, app, &all, quiet, i, total, cx).await;
        match outcome {
            Outcome::Removed => {
                let detail: Vec<String> = [app.publisher.clone(), app.version.clone()].into_iter().flatten().filter(|s| !s.trim().is_empty()).collect();
                journal::add(journal::Entry::new("uninstall", format!("Удалена «{}»", app.name)).detail(detail.join(" · ")).bytes(app.size.unwrap_or(0)));
                removed.push(app.clone());
            }
            Outcome::EntryRemoved => entries.push(app.id.clone()),
            Outcome::Kept => {}
        }
    }
    let gone: Vec<String> = removed.iter().map(|a| a.id.clone()).chain(entries).collect();
    this.update(cx, |v, cx| v.remove_from_list(&gone, cx)).ok();
    if !removed.is_empty() {
        toast(
            cx,
            Notification::success(if removed.len() == 1 {
                format!("«{}» удалена", removed[0].name)
            } else {
                format!("Удалено: {}", fmt::count(removed.len() as u64, "программа", "программы", "программ"))
            }),
        );
    }
    let forced: HashSet<String> = this.update(cx, |v, _| v.forced.clone()).unwrap_or_default();
    let for_leftovers: Vec<App> = removed.into_iter().filter(|a| !a.is_store && !a.is_launcher_uninstall() && !forced.contains(&a.id)).collect();
    if leftovers_opt && !for_leftovers.is_empty() {
        let remaining: Vec<App> = this.update(cx, |v, _| v.apps.clone()).unwrap_or_default();
        offer_leftovers(for_leftovers, remaining, cx).await;
    }
    this.update(cx, |v, cx| {
        v.busy = false;
        v.forced.clear();
        cx.notify();
    })
    .ok();
    history::record_async();
}

fn new_progress(app: &App, index: usize, total: usize) -> Busy {
    let mut b = Busy::new(format!("Удаляем «{}»", app.name));
    if total > 1 {
        b.detail = format!("Программа {} из {total}", index + 1);
    }
    b
}

async fn uninstall_one(this: &WeakEntity<ProgramsView>, app: &App, all: &[App], quiet: bool, index: usize, total: usize, cx: &mut AsyncWindowContext) -> Outcome {
    if app.is_store {
        return uninstall_store(app, index, total, cx).await;
    }
    loop {
        let cancel = Arc::new(AtomicBool::new(false));
        let busy = cx.new(|_| {
            let mut b = new_progress(app, index, total);
            b.message = if app.is_launcher_uninstall() { format!("Открываем {}…", app.launcher_name().unwrap_or_default()) } else { "Запускаем деинсталлятор…".into() };
            b.cancel = Some(cancel.clone());
            b.cancel_label = if total > 1 { "Пропустить".into() } else { "Прекратить ожидание".into() };
            b
        });
        open_busy(cx, &busy);
        let a = app.clone();
        let started = bg(cx, move || uninstall::start(&a, quiet)).await;
        let pid = match started {
            Ok(pid) => pid,
            Err(StartError::Cancelled) => {
                close_busy(cx);
                toast(cx, Notification::warning("Удаление отменено"));
                return Outcome::Kept;
            }
            Err(StartError::Missing) => {
                close_busy(cx);
                return offer_broken(this, app, all, cx).await;
            }
            Err(StartError::Other(e)) => {
                close_busy(cx);
                let t = format!("Не удалось запустить деинсталлятор «{}»", app.name);
                cx.update(|window, cx| ui::error(window, cx, t, e)).ok();
                return Outcome::Kept;
            }
        };
        let gone = wait_for_uninstall(app, pid, &busy, &cancel, total, cx).await;
        close_busy(cx);
        if gone {
            return Outcome::Removed;
        }
        // the uninstaller finished (or the user stopped waiting) but the program is still registered
        let orphan = uninstall::uninstaller_missing(app) || app.install_location.as_ref().is_some_and(|l| !l.exists());
        let can_force = !app.is_launcher_uninstall() && (app.has_registry_key() || app.install_location.is_some());
        let mut q = Ask::new(
            (Lucide::TriangleAlert, Tone::Warning),
            format!("«{}» не удалена", app.name),
            if orphan {
                "Программа осталась в списке, хотя её файлов уже нет. Скорее всего, это «осиротевшая» запись — её можно убрать из списка.".to_string()
            } else {
                format!(
                    "Деинсталлятор завершил работу, но программа осталась в системе. Возможно, удаление было отменено или завершилось с ошибкой.{}",
                    if can_force { " Если деинсталлятор не работает, можно удалить программу принудительно." } else { "" }
                )
            },
        )
        .button(if total > 1 { "Пропустить" } else { "Закрыть" }, Btn::Plain);
        let third = if orphan && app.has_registry_key() {
            Some("Убрать из списка")
        } else if can_force {
            Some("Удалить принудительно")
        } else {
            None
        };
        if let Some(t) = third {
            q = q.button(t, Btn::Plain);
        }
        q = q.button("Повторить", Btn::Primary);
        let retry_ix = if third.is_some() { 2 } else { 1 };
        match ask::ask(cx, q).await {
            Some(i) if i == retry_ix => continue,
            Some(1) if third.is_some() => {
                return if orphan { offer_remove_entry(app, false, cx).await } else { force_uninstall(this, app, all, Some(index), false, cx).await };
            }
            _ => return Outcome::Kept,
        }
    }
}

/// Waits for the uninstaller and everything it spawns. Never forever: once it is gone the result is
/// decided after a grace period (longer when the work went to a launcher such as Steam).
async fn wait_for_uninstall(app: &App, pid: Option<u32>, busy: &Entity<Busy>, stop: &Arc<AtomicBool>, total: usize, cx: &mut AsyncWindowContext) -> bool {
    let tracker = pid.map(|p| Arc::new(Mutex::new(process::Tracker::new(p))));
    let start = Instant::now();
    let mut last_alive = Duration::ZERO;
    let launcher = app.is_launcher_uninstall();
    while !stop.load(Ordering::Relaxed) {
        let (a, t) = (app.clone(), tracker.clone());
        let (installed, alive, name) = bg(cx, move || {
            let installed = uninstall::still_installed(&a);
            let (alive, name) = match &t {
                Some(t) => {
                    let mut t = t.lock().unwrap_or_else(|e| e.into_inner());
                    let alive = t.poll();
                    if alive {
                        t.raise_windows();
                    }
                    (alive, t.active_name.clone())
                }
                None => (false, None),
            };
            (installed, alive, name)
        })
        .await;
        if !installed {
            set_busy(cx, busy, |b| {
                b.progress = Some(1.0);
                b.message = "Готово — программа удалена.".into();
            });
            cx.background_executor().timer(Duration::from_millis(350)).await;
            return true;
        }
        let elapsed = start.elapsed();
        if alive {
            last_alive = elapsed;
        }
        let idle = elapsed - last_alive;
        let handed_off = launcher || last_alive < Duration::from_secs(4); // exited at once → work continues elsewhere
        let limit = if launcher {
            Duration::from_secs(300)
        } else if handed_off {
            Duration::from_secs(20)
        } else {
            Duration::from_secs(8)
        };
        let msg = if launcher {
            format!("{} Это окно закроется само.", launcher_hint(app))
        } else if alive {
            "Идёт удаление — следуйте инструкциям в окне деинсталлятора. Это окно закроется само.".into()
        } else if handed_off {
            "Деинсталлятор сразу закрылся. Если удаление продолжается в другом окне — дождитесь его, если вы его отменили — нажмите «Не ждать».".into()
        } else {
            "Деинсталлятор закрылся — проверяем результат…".into()
        };
        let mut text = format!("Прошло {}", fmt::clock(elapsed.as_secs()));
        if alive
            && let Some(n) = &name {
                text.push_str(&format!("  ·  процесс {n}"));
            }
        if !alive && idle > Duration::from_secs(1) {
            text.push_str(&format!("  ·  ожидание закончится через {}", fmt::clock(limit.saturating_sub(idle).as_secs())));
        }
        set_busy(cx, busy, |b| {
            b.message = msg;
            b.detail = text;
            if !alive && total == 1 {
                b.cancel_label = "Не ждать".into();
            }
        });
        if !alive && idle > limit {
            break;
        }
        cx.background_executor().timer(Duration::from_millis(700)).await;
    }
    set_busy(cx, busy, |b| b.message = "Проверяем результат…".into());
    let a = app.clone();
    !bg(cx, move || uninstall::still_installed(&a)).await
}

async fn uninstall_store(app: &App, index: usize, total: usize, cx: &mut AsyncWindowContext) -> Outcome {
    let cancel = Arc::new(AtomicBool::new(false));
    let pct = Arc::new(AtomicU32::new(0));
    let busy = cx.new(|_| {
        let mut b = new_progress(app, index, total);
        b.message = "Удаляем приложение Microsoft Store…".into();
        b.progress = Some(0.0);
        b.cancel = Some(cancel.clone());
        b.cancel_label = "Отменить".into();
        b
    });
    open_busy(cx, &busy);
    let (a, c, p) = (app.clone(), cancel.clone(), pct.clone());
    let work = cx.background_executor().spawn(async move {
        uninstall::remove_store_app(&a, |x| p.store((x * 1000.0) as u32, Ordering::Relaxed), || c.load(Ordering::Relaxed))
    });
    let result = Arc::new(Mutex::new(None));
    let r2 = result.clone();
    cx.background_executor()
        .spawn(async move {
            let r = work.await;
            *r2.lock().unwrap() = Some(r);
        })
        .detach();
    let r = loop {
        if let Some(r) = result.lock().unwrap().take() {
            break r;
        }
        let v = pct.load(Ordering::Relaxed) as f32 / 1000.0;
        set_busy(cx, &busy, |b| {
            b.progress = Some(v);
            b.detail = if v >= 1.0 { "Завершаем…".into() } else { format!("Выполнено {}", fmt::percent(v as f64)) };
        });
        cx.background_executor().timer(Duration::from_millis(250)).await;
    };
    match r {
        Ok(()) => {
            set_busy(cx, &busy, |b| {
                b.progress = Some(1.0);
                b.message = "Готово — приложение удалено.".into();
            });
            cx.background_executor().timer(Duration::from_millis(350)).await;
            close_busy(cx);
            Outcome::Removed
        }
        Err(None) => {
            close_busy(cx);
            let a = app.clone();
            if !bg(cx, move || uninstall::still_installed(&a)).await {
                return Outcome::Removed;
            }
            toast(cx, Notification::warning("Удаление отменено"));
            Outcome::Kept
        }
        Err(Some(e)) => {
            close_busy(cx);
            let t = format!("Не удалось удалить «{}»", app.name);
            cx.update(|window, cx| ui::error(window, cx, t, e)).ok();
            Outcome::Kept
        }
    }
}

/// The uninstaller file is missing: offer to force-remove the rest or drop the entry.
async fn offer_broken(this: &WeakEntity<ProgramsView>, app: &App, all: &[App], cx: &mut AsyncWindowContext) -> Outcome {
    let has_files = app.install_location.as_ref().is_some_and(|l| l.is_dir());
    if !has_files {
        return offer_remove_entry(app, true, cx).await;
    }
    let mut q = Ask::new(
        (Lucide::TriangleAlert, Tone::Warning),
        "Деинсталлятор не найден",
        format!(
            "Файл деинсталлятора «{}» отсутствует, но папка программы на месте. Простор может удалить программу сам: папку, записи в реестре, ярлыки и задачи — с резервными копиями.",
            app.name
        ),
    )
    .button("Отмена", Btn::Plain);
    if app.has_registry_key() {
        q = q.button("Убрать из списка", Btn::Plain);
    }
    q = q.button("Удалить принудительно", Btn::Danger);
    let force_ix = if app.has_registry_key() { 2 } else { 1 };
    match ask::ask(cx, q).await {
        Some(i) if i == force_ix => force_uninstall(this, app, all, Some(0), true, cx).await,
        Some(1) if app.has_registry_key() => offer_remove_entry(app, false, cx).await,
        _ => Outcome::Kept,
    }
}

async fn offer_remove_entry(app: &App, missing_uninstaller: bool, cx: &mut AsyncWindowContext) -> Outcome {
    if !app.has_registry_key() {
        let t = format!("Не удалось удалить «{}»", app.name);
        cx.update(|window, cx| ui::error(window, cx, t, "Деинсталлятор программы не найден.")).ok();
        return Outcome::Kept;
    }
    let mut message = String::new();
    if missing_uninstaller {
        message.push_str(&format!("Файл деинсталлятора «{}» отсутствует — похоже, программу уже удалили вручную, а в списке осталась её запись.\n\n", app.name));
    }
    message.push_str("Будет удалена только запись в реестре, файлы на диске не трогаются. Резервная копия записи сохранится — её можно вернуть из «Журнала».");
    let q = Ask::new(
        (Lucide::ListX, Tone::Warning),
        if missing_uninstaller { "Деинсталлятор не найден".to_string() } else { format!("Убрать «{}» из списка?", app.name) },
        message,
    )
    .button("Отмена", Btn::Plain)
    .button("Убрать из списка", Btn::Primary);
    if ask::ask(cx, q).await != Some(1) {
        return Outcome::Kept;
    }
    let a = app.clone();
    match bg(cx, move || uninstall::remove_uninstall_entry(&a)).await {
        Ok(backup) => {
            let mut e = journal::Entry::new("entry", format!("Убрана запись «{}»", app.name)).detail("Только запись в списке программ, файлы не тронуты");
            if let Some(b) = backup {
                e = e.undo(journal::UndoKind::RegImport, vec![b.to_string_lossy().to_string()]);
            }
            journal::add(e);
            toast(cx, Notification::success(format!("Запись «{}» убрана из списка", app.name)));
            Outcome::EntryRemoved
        }
        Err(e) => {
            cx.update(|window, cx| ui::error(window, cx, "Не удалось убрать запись", e)).ok();
            Outcome::Kept
        }
    }
}

/// `index`: set when called from a running uninstall (None — started from the details panel).
async fn force_uninstall(this: &WeakEntity<ProgramsView>, app: &App, all: &[App], index: Option<usize>, confirmed: bool, cx: &mut AsyncWindowContext) -> Outcome {
    let standalone = index.is_none();
    if !confirmed {
        let q = Ask::new(
            (Lucide::Shredder, Tone::Danger),
            format!("Удалить «{}» принудительно?", app.name),
            "Деинсталлятор программы запускаться не будет — Простор сам удалит её папку, записи в реестре, ярлыки, задачи и службы. Используйте, если обычное удаление не работает.\n\nПапки и ярлыки попадут в корзину, записи реестра сохранятся в резервных копиях. Запущенные процессы программы будут закрыты.",
        )
        .button("Отмена", Btn::Plain)
        .button("Найти, что удалить", Btn::Danger);
        if ask::ask(cx, q).await != Some(1) {
            return Outcome::Kept;
        }
    }
    let busy = cx.new(|_| Busy::new(format!("Ищем всё, что относится к «{}»…", app.name)));
    open_busy(cx, &busy);
    let (a, rest) = (app.clone(), all.to_vec());
    let found = bg(cx, move || leftovers::find(&a, &rest, true)).await;
    close_busy(cx);
    let ok = review_and_remove(
        vec![(app.clone(), found)],
        &format!("Удаление «{}»", app.name),
        "Отмеченное будет удалено. Папки и ярлыки — в корзину, записи реестра и задачи — с резервными копиями.",
        "Удалить",
        true,
        cx,
    )
    .await;
    if !ok {
        return Outcome::Kept;
    }
    if app.has_registry_key() {
        let a = app.clone();
        match bg(cx, move || uninstall::remove_uninstall_entry(&a)).await {
            Ok(Some(b)) => journal::add(
                journal::Entry::new("entry", format!("Убрана запись «{}»", app.name))
                    .detail("При принудительном удалении")
                    .undo(journal::UndoKind::RegImport, vec![b.to_string_lossy().to_string()]),
            ),
            Ok(None) => {}
            Err(e) => {
                cx.update(|window, cx| ui::error(window, cx, "Запись программы не удалена", e)).ok();
                return Outcome::Kept;
            }
        }
    }
    this.update(cx, |v, _| {
        v.forced.insert(app.id.clone());
    })
    .ok();
    if standalone {
        journal::add(
            journal::Entry::new("force", format!("Принудительно удалена «{}»", app.name))
                .detail(app.install_location.as_ref().map(|l| l.display().to_string()).unwrap_or_default())
                .bytes(app.size.unwrap_or(0)),
        );
        this.update(cx, |v, cx| v.remove_from_list(std::slice::from_ref(&app.id), cx)).ok();
        toast(cx, Notification::success(format!("«{}» удалена", app.name)));
        history::record_async();
    }
    Outcome::Removed
}

async fn offer_leftovers(removed: Vec<App>, remaining: Vec<App>, cx: &mut AsyncWindowContext) {
    let busy = cx.new(|_| {
        let mut b = Busy::new("Ищем остатки…");
        b.message = "Проверяем папки, реестр, ярлыки, автозагрузку, задачи и службы.".into();
        b
    });
    open_busy(cx, &busy);
    let found = bg(cx, move || removed.iter().map(|a| (a.clone(), leftovers::find(a, &remaining, true))).collect::<Vec<_>>()).await;
    close_busy(cx);
    if found.iter().all(|f| f.1.is_empty()) {
        toast(cx, Notification::success("Остатков не найдено — всё чисто"));
        return;
    }
    review_and_remove(
        found,
        "Найдены остатки",
        "После удаления остались файлы и записи. Отмеченное будет удалено: папки и ярлыки — в корзину, записи реестра и задачи — с резервными копиями.",
        "Удалить остатки",
        false,
        cx,
    )
    .await;
}

/// Shows what was found and removes the checked items. False when cancelled.
async fn review_and_remove(found: Vec<(App, Vec<Leftover>)>, title: &str, message: &str, primary: &str, force: bool, cx: &mut AsyncWindowContext) -> bool {
    let admin = shell::is_admin();
    let all: Vec<Leftover> = found.iter().flat_map(|f| f.1.iter().cloned()).collect();
    let allowed: Vec<Leftover> = all.iter().filter(|i| admin || !i.needs_admin).cloned().collect();
    let hidden = all.len() - allowed.len();
    if allowed.is_empty() {
        if hidden > 0 {
            let t = format!("Найдено {}, которые можно удалить только в режиме администратора.", fmt::count(hidden as u64, "запись", "записи", "записей"));
            cx.update(|window, cx| ui::error(window, cx, "Нужны права администратора", t)).ok();
        } else if !force {
            toast(cx, Notification::success("Остатков не найдено — всё чисто"));
        }
        return force && hidden == 0;
    }
    let total: u64 = allowed.iter().filter(|i| i.kind == leftovers::Kind::Folder).map(|i| i.size).sum();
    let picks = cx.new(|_| Picks {
        items: allowed
            .iter()
            .map(|i| Pick {
                title: i.title.clone(),
                subtitle: format!("{} · {}", i.kind.text(), i.target),
                detail: if i.kind == leftovers::Kind::Folder { if i.size > 0 { fmt::size(i.size) } else { "пустая".into() } } else { i.kind.text().into() },
                checked: true,
            })
            .collect(),
    });
    let mut msg = message.to_string();
    if total > 0 {
        msg.push_str(&format!("\nПапки занимают {}.", fmt::size(total)));
    }
    let q = Ask::new(if force { (Lucide::Shredder, Tone::Danger) } else { (Lucide::Sparkles, Tone::Warning) }, title, msg)
        .button(if force { "Отмена" } else { "Оставить" }, Btn::Plain)
        .button(primary, if force { Btn::Danger } else { Btn::Primary })
        .extra(picks.clone())
        .footnote(if hidden > 0 {
            format!(
                "Ещё {} (службы, правила брандмауэра, записи для всех пользователей) можно удалить в режиме администратора.",
                fmt::count(hidden as u64, "запись", "записи", "записей")
            )
        } else {
            String::new()
        })
        .width(640.);
    if ask::ask(cx, q).await != Some(1) {
        return false;
    }
    let chosen: Vec<Leftover> = picks.read_with(cx, |p, _| p.checked()).into_iter().map(|i| allowed[i].clone()).collect();
    if chosen.is_empty() {
        return force;
    }
    if force
        && let Some(loc) = found[0].0.install_location.clone() {
            bg(cx, move || {
                for (pid, _) in process::running_from(&loc) {
                    process::kill(pid);
                }
            })
            .await;
        }
    let busy = cx.new(|_| Busy::new("Удаляем…"));
    open_busy(cx, &busy);
    let label = if found.len() == 1 { found[0].0.name.clone() } else { "leftovers".into() };
    let progress = Arc::new(Mutex::new(String::new()));
    let p2 = progress.clone();
    let result = bg(cx, move || leftovers::remove(&chosen, &label, |t| *p2.lock().unwrap() = t.to_string())).await;
    close_busy(cx);

    let names = if found.len() == 1 { format!("«{}»", found[0].0.name) } else { fmt::count(found.len() as u64, "программы", "программ", "программ") };
    if !result.recycled.is_empty() {
        journal::add(
            journal::Entry::new("leftovers", format!("Остатки {names} перемещены в корзину"))
                .detail(fmt::count(result.recycled.len() as u64, "объект", "объекта", "объектов"))
                .bytes(result.bytes)
                .undo(journal::UndoKind::RecycleRestore, result.recycled.clone()),
        );
    }
    if !result.reg_backups.is_empty() {
        journal::add(
            journal::Entry::new("leftovers", format!("Удалены записи реестра {names}"))
                .detail(format!("{} · есть резервные копии", fmt::count(result.reg_backups.len() as u64, "ключ", "ключа", "ключей")))
                .undo(journal::UndoKind::RegImport, result.reg_backups.clone()),
        );
    }
    if !result.task_backups.is_empty() {
        let list: Vec<&str> = result.task_backups.iter().map(|t| t.split('|').next().unwrap_or_default()).collect();
        journal::add(
            journal::Entry::new("leftovers", format!("Удалены задачи планировщика {names}"))
                .detail(list.join(", "))
                .undo(journal::UndoKind::TaskRestore, result.task_backups.clone()),
        );
    }
    toast(
        cx,
        if result.failed == 0 {
            Notification::success(format!("Удалено: {}", fmt::count(result.removed as u64, "объект", "объекта", "объектов")))
        } else {
            Notification::warning(format!(
                "Удалено {} из {} — часть занята или требует прав администратора",
                result.removed,
                result.removed + result.failed
            ))
        },
    );
    true
}

async fn update_flow(this: WeakEntity<ProgramsView>, apps: Vec<App>, cx: &mut AsyncWindowContext) {
    if !winget::available() {
        cx.update(|window, cx| ui::error(window, cx, "winget не найден", "Обновление работает через «Установщик приложений» (winget) из Microsoft Store.")).ok();
        return;
    }
    let chosen: Vec<App> = if apps.len() > 1 {
        let picks = cx.new(|_| Picks {
            items: apps
                .iter()
                .map(|a| Pick {
                    title: a.name.clone(),
                    subtitle: format!("{} → {}", a.version.clone().unwrap_or_default(), a.update_version.clone().unwrap_or_default()),
                    detail: a.size.map(fmt::size).unwrap_or_default(),
                    checked: true,
                })
                .collect(),
        });
        let q = Ask::new(
            (Lucide::Download, Tone::Accent),
            format!("Обновить {}?", fmt::count(apps.len() as u64, "программу", "программы", "программ")),
            "Обновления скачаются и установятся по очереди. Некоторым программам понадобится подтверждение администратора, а запущенные программы могут закрыться.",
        )
        .button("Отмена", Btn::Plain)
        .button("Обновить", Btn::Primary)
        .extra(picks.clone());
        if ask::ask(cx, q).await != Some(1) {
            return;
        }
        picks.read_with(cx, |p, _| p.checked()).into_iter().map(|i| apps[i].clone()).collect()
    } else {
        apps
    };
    if chosen.is_empty() {
        return;
    }
    this.update(cx, |v, cx| {
        v.busy = true;
        cx.notify();
    })
    .ok();
    let total = chosen.len();
    let (mut ok, mut failed) = (Vec::new(), Vec::new());
    for (i, app) in chosen.iter().enumerate() {
        let busy = cx.new(|_| {
            let mut b = Busy::new(format!("Обновляем «{}»", app.name));
            b.message = format!("{} → {}", app.version.clone().unwrap_or_default(), app.update_version.clone().unwrap_or_default());
            if total > 1 {
                b.detail = format!("Программа {} из {total}", i + 1);
            }
            b
        });
        open_busy(cx, &busy);
        let state: Arc<Mutex<UpdateState>> = Arc::new(Mutex::new((None, String::new(), None)));
        let (s2, id) = (state.clone(), app.winget_id.clone().unwrap_or_default());
        cx.background_executor()
            .spawn(async move {
                let s3 = s2.clone();
                let r = winget::upgrade(&id, move |p, line| {
                    let mut g = s3.lock().unwrap();
                    if p.is_some() {
                        g.0 = p;
                    }
                    if !line.is_empty() {
                        g.1 = line.to_string();
                    }
                });
                s2.lock().unwrap().2 = Some(match r {
                    Ok(r) => Ok(winget::is_success(&r)),
                    Err(e) => Err(e.to_string()),
                });
            })
            .detach();
        let result = loop {
            let (p, line, done) = {
                let mut g = state.lock().unwrap();
                (g.0, g.1.clone(), g.2.take())
            };
            if let Some(r) = done {
                break r;
            }
            set_busy(cx, &busy, |b| {
                b.progress = p;
                if !line.is_empty() {
                    b.message = line;
                }
            });
            cx.background_executor().timer(Duration::from_millis(250)).await;
        };
        close_busy(cx);
        match result {
            Ok(true) => {
                journal::add(
                    journal::Entry::new("update", format!("Обновлена «{}»", app.name))
                        .detail(format!("{} → {}", app.version.clone().unwrap_or_default(), app.update_version.clone().unwrap_or_default())),
                );
                ok.push(app.id.clone());
            }
            _ => failed.push(app.name.clone()),
        }
    }
    this.update(cx, |v, cx| {
        v.busy = false;
        for a in v.apps.iter_mut().filter(|a| ok.contains(&a.id)) {
            a.version = a.update_version.take();
            a.winget_id = None;
        }
        cx.notify();
    })
    .ok();
    if failed.is_empty() {
        toast(cx, Notification::success(format!("Обновлено: {}", fmt::count(ok.len() as u64, "программа", "программы", "программ"))));
    } else {
        let t = format!("Не удалось обновить: {}. Попробуйте позже или обновите программу из её меню.", failed.join(", "));
        cx.update(|window, cx| ui::error(window, cx, "Часть обновлений не установилась", t)).ok();
    }
}

async fn export_flow(apps: Vec<App>, cx: &mut AsyncWindowContext) {
    let q = Ask::new(
        (Lucide::FileDown, Tone::Accent),
        "Экспорт списка программ",
        "Таблица CSV откроется в Excel. Файл для winget позволяет установить те же программы на другом компьютере командой «winget import».",
    )
    .button("Отмена", Btn::Plain)
    .button("Файл для winget", Btn::Plain)
    .button("Таблица CSV", Btn::Primary);
    let choice = ask::ask(cx, q).await;
    let (csv, name) = match choice {
        Some(2) => (true, "Программы.csv"),
        Some(1) => (false, "programs-winget.json"),
        _ => return,
    };
    if !csv && !winget::available() {
        cx.update(|window, cx| ui::error(window, cx, "winget не найден", "Экспорт для winget работает через «Установщик приложений» из Microsoft Store.")).ok();
        return;
    }
    let docs = shell::home().join("Documents");
    let Ok(rx) = cx.update(|_, cx| cx.prompt_for_new_path(&docs, Some(name))) else { return };
    let Ok(Ok(Some(path))) = rx.await else { return };
    let result = if csv {
        let text = winget::build_csv(&apps);
        std::fs::write(&path, text).map_err(|e| e.to_string())
    } else {
        let busy = cx.new(|_| Busy::new("Экспортируем через winget…"));
        open_busy(cx, &busy);
        let p = path.clone();
        let r = bg(cx, move || winget::export(&p)).await;
        close_busy(cx);
        match r {
            Ok(r) if path.exists() => {
                let _ = r;
                Ok(())
            }
            Ok(r) => Err(r.output),
            Err(e) => Err(e.to_string()),
        }
    };
    match result {
        Ok(()) => {
            toast(cx, Notification::success("Список сохранён"));
            shell::show_in_explorer(&path);
        }
        Err(e) => {
            cx.update(|window, cx| ui::error(window, cx, "Не удалось сохранить список", e)).ok();
        }
    }
}
