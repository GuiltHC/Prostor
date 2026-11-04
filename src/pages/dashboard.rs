//! "Обзор" page: drives with history, forecast and health, quick stats and recommendations
//! (port of ViewModels/DashboardViewModel.cs).

use crate::fmt;
use crate::pages::Navigate;
use crate::pages::cleanup::CleanupView;
use crate::pages::programs::ProgramsView;
use crate::pages::startup::StartupView;
use crate::services::health::{self, DiskHealth, Level};
use crate::services::history::{self, Forecast};
use crate::sys::drives::{self, Drive};
use crate::sys::shell;
use crate::ui::{self, Lucide, Tone};
use chrono::{Local, Timelike};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;

struct DriveInfo {
    drive: Drive,
    /// Free space, GB, last 90 days.
    history: Vec<f64>,
    forecast: Option<Forecast>,
    health: Option<DiskHealth>,
}

struct Recommendation {
    icon: Lucide,
    tone: Tone,
    title: String,
    detail: String,
    size: u64,
    action: Option<(&'static str, String)>,
}

pub struct DashboardView {
    programs: Entity<ProgramsView>,
    cleanup: Entity<CleanupView>,
    startup: Entity<StartupView>,
    drives: Vec<DriveInfo>,
    old_downloads: (u64, u64),
    _tasks: Vec<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<Navigate> for DashboardView {}

impl DashboardView {
    pub fn new(programs: Entity<ProgramsView>, cleanup: Entity<CleanupView>, startup: Entity<StartupView>, cx: &mut Context<Self>) -> Self {
        let subs = vec![
            cx.observe(&programs, |_, _, cx| cx.notify()),
            cx.observe(&cleanup, |_, _, cx| cx.notify()),
            cx.observe(&startup, |_, _, cx| cx.notify()),
        ];
        let mut v = Self { programs, cleanup, startup, drives: Vec::new(), old_downloads: (0, 0), _tasks: Vec::new(), _subs: subs };
        v.refresh(cx);
        v
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let work = cx.background_executor().spawn(async {
            let hist = history::load();
            drives::list()
                .into_iter()
                .map(|d| {
                    let points = hist.get(d.letter()).cloned().unwrap_or_default();
                    let cutoff = Local::now() - chrono::Duration::days(90);
                    let recent: Vec<f64> = points.iter().filter(|p| p.date >= cutoff).map(|p| p.free as f64 / (1u64 << 30) as f64).collect();
                    DriveInfo { forecast: history::forecast(&points, 90), history: if recent.len() >= 2 { recent } else { Vec::new() }, health: None, drive: d }
                })
                .collect::<Vec<_>>()
        });
        let health_work = cx.background_executor().spawn(async { health::load() });
        let dl_work = cx.background_executor().spawn(async { old_downloads() });
        let t = cx.spawn(async move |this, cx| {
            let drives = work.await;
            this.update(cx, |v, cx| {
                v.drives = drives;
                cx.notify();
            })
            .ok();
            let disks = health_work.await;
            this.update(cx, |v, cx| {
                for d in &mut v.drives {
                    let letter = d.drive.letter().chars().next().unwrap_or(' ').to_ascii_uppercase();
                    d.health = disks.iter().find(|x| x.letters.contains(&letter)).cloned();
                }
                cx.notify();
            })
            .ok();
            let dl = dl_work.await;
            this.update(cx, |v, cx| {
                v.old_downloads = dl;
                cx.notify();
            })
            .ok();
        });
        self._tasks = vec![t];
    }

    fn recommendations(&self, cx: &App) -> Vec<Recommendation> {
        let mut list = Vec::new();
        for d in &self.drives {
            if let Some(days) = d.forecast.as_ref().and_then(|f| f.days_until_full).filter(|&n| n < 120) {
                list.push(Recommendation {
                    icon: Lucide::TrendingUp,
                    tone: if days < 30 { Tone::Danger } else { Tone::Warning },
                    title: format!("Диск {} заполняется", d.drive.letter()),
                    detail: forecast_text(d.forecast.as_ref()),
                    size: 0,
                    action: Some(("Найти, что растёт", format!("analyze:{}", d.drive.root))),
                });
            }
            if let Some(h) = d.health.as_ref().filter(|h| matches!(h.level, Level::Warn | Level::Danger)) {
                list.push(Recommendation {
                    icon: Lucide::ShieldAlert,
                    tone: if h.level == Level::Danger { Tone::Danger } else { Tone::Warning },
                    title: format!("Диск {}: {} · {}", d.drive.letter(), h.media, h.status),
                    detail: format!("{}. Сделайте резервную копию важных данных.", if h.model.is_empty() { h.summary() } else { format!("{} — {}", h.model, h.summary()) }),
                    size: 0,
                    action: None,
                });
            }
        }
        let cleanup = self.cleanup.read(cx);
        let junk = cleanup.total_found();
        if junk >= 100 << 20 {
            list.push(Recommendation {
                icon: Lucide::Sparkles,
                tone: Tone::Success,
                title: "Временные файлы и кэши".into(),
                detail: "Их можно удалить без последствий — программы создадут их заново".into(),
                size: junk,
                action: Some(("Очистить", "cleanup".into())),
            });
        }
        let programs = self.programs.read(cx);
        let (unused, unused_size) = programs.unused();
        if unused > 0 {
            list.push(Recommendation {
                icon: Lucide::Clock,
                tone: Tone::Info,
                title: format!("{} давно не запускались", fmt::count(unused as u64, "программа", "программы", "программ")),
                detail: "Возможно, они больше не нужны — посмотрите список".into(),
                size: unused_size,
                action: Some(("Посмотреть", "programs:unused".into())),
            });
        }
        let (dl_count, dl_size) = self.old_downloads;
        if dl_count > 0 && dl_size >= 100 << 20 {
            list.push(Recommendation {
                icon: Lucide::Download,
                tone: Tone::Info,
                title: "Старые файлы в «Загрузках»".into(),
                detail: format!("{} крупнее 10 МБ лежат там дольше трёх месяцев — обычно это ненужные установщики и архивы", fmt::files(dl_count)),
                size: dl_size,
                action: Some(("Разобрать", format!("analyze-tab:downloads:{}", shell::downloads().display()))),
            });
        }
        let updates = programs.updates();
        if updates > 0 {
            list.push(Recommendation {
                icon: Lucide::RefreshCw,
                tone: Tone::Accent,
                title: format!("Доступны обновления: {}", fmt::count(updates as u64, "программа", "программы", "программ")),
                detail: "Новые версии исправляют ошибки и уязвимости".into(),
                size: 0,
                action: Some(("Обновить", "programs:updates".into())),
            });
        }
        let enabled = self.startup.read(cx).enabled_count();
        if enabled >= 12 {
            list.push(Recommendation {
                icon: Lucide::Rocket,
                tone: Tone::Warning,
                title: format!("{enabled} программ запускаются вместе с Windows"),
                detail: "Отключите лишние или запускайте их через минуту — компьютер будет загружаться быстрее".into(),
                size: 0,
                action: Some(("Настроить", "startup".into())),
            });
        }
        list
    }

    fn render_drive(&self, ix: usize, d: &DriveInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let used = d.drive.used_fraction();
        let color = usage_color(used, &theme);
        let root = PathBuf::from(&d.drive.root);
        let health = d.health.as_ref().map(|h| {
            let tone = match h.level {
                Level::Ok => Tone::Success,
                Level::Warn => Tone::Warning,
                Level::Danger => Tone::Danger,
                Level::Unknown => Tone::Neutral,
            };
            (h.status.clone(), tone)
        });
        let details = match &d.health {
            Some(h) => {
                let mut parts = vec![h.media.clone()];
                if let Some(w) = h.wear {
                    parts.push(format!("износ {w}%"));
                }
                if let Some(t) = h.temperature.filter(|t| *t > 0) {
                    parts.push(format!("{t} °C"));
                }
                parts.join(" · ")
            }
            None => d.drive.file_system.clone(),
        };
        ui::card(cx)
            .id(("drive", ix))
            .flex_1()
            .min_w(px(280.))
            .p_4()
            .gap_3()
            .cursor_pointer()
            .hover(|s| s.bg(ui::card_hover(&theme)))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(Navigate::to(format!("analyze:{}", root.display())))))
            .child(
                h_flex()
                    .gap_3()
                    .child(ui::icon_tile(if d.drive.removable { Lucide::Usb } else { Lucide::HardDrive }, if used > 0.9 { Tone::Danger } else { Tone::Accent }, 36., cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_semibold().truncate().child(d.drive.title()))
                            .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(details)),
                    )
                    .when_some(health, |s, (text, tone)| s.child(ui::badge(text, tone, cx))),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap_1()
                            .child(div().text_lg().font_semibold().child(fmt::size(d.drive.free)))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(format!("свободно из {}", fmt::size(d.drive.total))))
                            .child(div().flex_1())
                            .child(div().text_xs().font_medium().text_color(color).child(fmt::percent(used as f64))),
                    )
                    .child(ui::bar(used, color, cx)),
            )
            .when(d.history.len() >= 3, |s| s.child(ui::charts::area(d.history.clone(), color, 44.)))
            .child(div().text_xs().text_color(theme.muted_foreground).child(forecast_text(d.forecast.as_ref())))
    }

    fn render_hero(&self, junk: u64, scanning: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let system = drives::system_root();
        let letter = system.trim_end_matches('\\').to_string();
        let d = self.drives.iter().find(|d| d.drive.root.eq_ignore_ascii_case(&system));
        let (used, free, total) = d.map(|d| (d.drive.used_fraction(), d.drive.free, d.drive.total)).unwrap_or((0.0, 0, 0));
        let color = usage_color(used, &theme);
        let extra = self.old_downloads.1 + self.programs.read(cx).unused().1;
        let reclaim = junk + extra;
        let (headline, line) = if total == 0 {
            ("Смотрим на диск…".to_string(), String::new())
        } else if used > 0.9 {
            (format!("На диске {letter} осталось {}", fmt::size(free)), "Windows и программам становится тесно — освободите место".to_string())
        } else {
            (format!("Свободно {} из {}", fmt::size(free), fmt::size(total)), forecast_text(d.and_then(|d| d.forecast.as_ref())))
        };
        let accent = theme.primary;
        let target = system.clone();
        ui::card(cx)
            .p_6()
            .bg(linear_gradient(
                115.,
                linear_color_stop(accent.opacity(if theme.is_dark() { 0.16 } else { 0.10 }), 0.),
                linear_color_stop(ui::card_bg(&theme), 0.65),
            ))
            .child(
                h_flex()
                    .gap_6()
                    .items_center()
                    .flex_wrap()
                    .child(
                        div()
                            .relative()
                            .size(px(128.))
                            .flex_shrink_0()
                            .child(ui::charts::ring(used, color, theme.foreground.opacity(0.08), 128., 12.))
                            .child(
                                v_flex()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .size_full()
                                    .items_center()
                                    .justify_center()
                                    .child(div().text_size(px(26.)).font_semibold().child(if total == 0 { "…".into() } else { fmt::percent(used as f64) }))
                                    .child(div().text_xs().text_color(theme.muted_foreground).child("занято")),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(240.))
                            .gap_1()
                            .child(div().text_xs().font_medium().text_color(theme.muted_foreground).child(format!("Системный диск {letter}")))
                            .child(div().text_size(px(22.)).line_height(px(28.)).font_semibold().child(headline))
                            .when(!line.is_empty(), |s| s.child(div().text_sm().text_color(theme.muted_foreground).child(line)))
                            .child(
                                h_flex()
                                    .pt_3()
                                    .gap_2()
                                    .flex_wrap()
                                    .child(
                                        Button::new("hero-clean")
                                            .primary()
                                            .icon(Icon::new(Lucide::Sparkles))
                                            .label(if !(scanning && junk == 0) && reclaim > 0 {
                                                format!("Освободить до {}", fmt::size(reclaim))
                                            } else {
                                                "Освободить место".to_string()
                                            })
                                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Navigate::to("cleanup")))),
                                    )
                                    .child(Button::new("hero-analyze").outline().icon(Icon::new(Lucide::ChartPie)).label("Что занимает место").on_click(cx.listener(
                                        move |_, _, _, cx| cx.emit(Navigate::to(format!("analyze:{target}"))),
                                    ))),
                            ),
                    ),
            )
            .into_any_element()
    }
}

fn usage_color(used: f32, theme: &gpui_kit::component::Theme) -> Hsla {
    if used > 0.9 {
        theme.danger
    } else if used > 0.8 {
        theme.warning
    } else {
        theme.primary
    }
}

fn forecast_text(f: Option<&Forecast>) -> String {
    match f {
        None => "История и прогноз появятся после нескольких дней использования".into(),
        Some(f) => match f.days_until_full {
            Some(days) => format!(
                "Теряет ≈ {} в день — заполнится примерно через {}",
                fmt::size((-f.bytes_per_day).max(0.0) as u64),
                fmt::count(days.max(0) as u64, "день", "дня", "дней")
            ),
            None if f.bytes_per_day > 0.0 => format!("Свободного места становится больше · за {}", fmt::count(f.days.max(0) as u64, "день", "дня", "дней")),
            None => format!("Свободное место почти не меняется · за {}", fmt::count(f.days.max(0) as u64, "день", "дня", "дней")),
        },
    }
}

/// (count, size) of files over 10 MB lying in Downloads for more than three months.
fn old_downloads() -> (u64, u64) {
    use std::os::windows::fs::MetadataExt;
    let dl = shell::downloads();
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(90 * 86_400);
    let (mut n, mut size) = (0u64, 0u64);
    let never = std::sync::atomic::AtomicBool::new(false);
    crate::services::files::walk_files(&dl, &never, |_, md| {
        if md.len() < 10 << 20 {
            return;
        }
        let created = md.created().ok();
        let modified = md.modified().ok();
        let arrived = match (created, modified) {
            (Some(c), Some(m)) => c.min(m),
            (a, b) => a.or(b).unwrap_or(std::time::SystemTime::now()),
        };
        let _ = md.file_attributes();
        if arrived < cutoff {
            n += 1;
            size += md.len();
        }
    });
    (n, size)
}

impl Render for DashboardView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let h = Local::now().hour();
        let greeting = match h {
            5..=11 => "Доброе утро",
            12..=17 => "Добрый день",
            18..=22 => "Добрый вечер",
            _ => "Доброй ночи",
        };
        let user = std::env::var("USERNAME").unwrap_or_default();
        let system = format!("{} · {}", shell::windows_version_text(), std::env::var("COMPUTERNAME").unwrap_or_default());

        let (p_count, p_size) = {
            let p = self.programs.read(cx);
            (p.count(), p.total_size())
        };
        let (junk, scanning) = {
            let c = self.cleanup.read(cx);
            (c.total_found(), c.is_scanning())
        };
        let enabled = self.startup.read(cx).enabled_count();

        let tile = |icon: Lucide, tone: Tone, value: String, caption: String, page: &'static str, id: &'static str, cx: &mut Context<Self>| {
            let theme = cx.theme().clone();
            ui::card(cx)
                .id(id)
                .flex_1()
                .min_w(px(200.))
                .p_4()
                .cursor_pointer()
                .hover(|s| s.bg(ui::card_hover(&theme)))
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(Navigate::to(page))))
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(ui::icon_tile(icon, tone, 40., cx))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(if value == "—" {
                                    // still loading: a placeholder of the same height as the number
                                    div().py(px(5.)).child(ui::skeleton(px(72.), 18.)).into_any_element()
                                } else {
                                    div().text_xl().font_semibold().child(value).into_any_element()
                                })
                                .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(caption)),
                        )
                        .child(Icon::new(Lucide::ChevronRight).size_4().text_color(theme.muted_foreground.opacity(0.6))),
                )
        };
        let tiles = h_flex()
            .gap_3()
            .flex_wrap()
            .child(tile(
                Lucide::AppWindow,
                Tone::Accent,
                if p_count == 0 { "—".into() } else { p_count.to_string() },
                if p_count == 0 { "загружаем программы…".into() } else { format!("{} · ≈ {}", fmt::plural(p_count as u64, "программа", "программы", "программ"), fmt::size(p_size)) },
                "programs",
                "tile-programs",
                cx,
            ))
            .child(tile(
                Lucide::Sparkles,
                Tone::Success,
                if scanning && junk == 0 { "—".into() } else { fmt::size(junk) },
                if scanning { "ищем мусор…".into() } else { "мусор и кэши".into() },
                "cleanup",
                "tile-cleanup",
                cx,
            ))
            .child(tile(
                Lucide::Rocket,
                Tone::Warning,
                if enabled == 0 { "—".into() } else { enabled.to_string() },
                "в автозагрузке".into(),
                "startup",
                "tile-startup",
                cx,
            ));

        let mut drives = h_flex().gap_3().flex_wrap().items_start();
        let infos = std::mem::take(&mut self.drives);
        for (i, d) in infos.iter().enumerate() {
            drives = drives.child(self.render_drive(i, d, cx));
        }
        self.drives = infos;
        if self.drives.is_empty() {
            for _ in 0..2 {
                drives = drives.child(
                    ui::card(cx)
                        .flex_1()
                        .min_w(px(280.))
                        .p_4()
                        .gap_3()
                        .child(h_flex().gap_3().child(ui::skeleton(px(36.), 36.).rounded_lg()).child(v_flex().flex_1().gap_2().child(ui::skeleton(relative(0.5), 12.)).child(ui::skeleton(relative(0.3), 9.))))
                        .child(ui::skeleton(relative(0.35), 16.))
                        .child(ui::skeleton(relative(1.), 6.)),
                );
            }
        }
        let hero = self.render_hero(junk, scanning, cx);

        let recs = self.recommendations(cx);
        let total: u64 = recs.iter().map(|r| r.size).sum();
        let mut rec_card = ui::card(cx).py_1();
        for (i, r) in recs.iter().enumerate() {
            let action = r.action.clone();
            rec_card = rec_card.child(
                h_flex()
                    .gap_3()
                    .mx_4()
                    .py_3()
                    .when(i > 0, |s| s.border_t_1().border_color(ui::card_border(&theme)))
                    .child(ui::icon_tile(r.icon, r.tone, 34., cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(div().text_sm().font_medium().child(r.title.clone()))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(r.detail.clone())),
                    )
                    .when(r.size > 0, |s| s.child(div().text_sm().font_semibold().flex_shrink_0().child(fmt::size(r.size))))
                    .when_some(action, |s, (label, nav)| {
                        s.child(Button::new(("rec", i)).outline().small().label(label).on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(Navigate::to(nav.clone()));
                        })))
                    }),
            );
        }

        let largest = self.programs.read(cx).largest(7);
        let max = largest.first().map(|l| l.1).unwrap_or(1).max(1);
        let mut top = ui::card(cx).p_3().gap_1();
        for (i, (name, size, icon)) in largest.into_iter().enumerate() {
            let n2 = name.clone();
            top = top.child(
                h_flex()
                    .id(("top-app", i))
                    .gap_3()
                    .items_center()
                    .px_2()
                    .py_1p5()
                    .rounded_lg()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.foreground.opacity(0.045)))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(Navigate::to(format!("program:{n2}")))))
                    .child(ui::app_icon(icon.as_deref(), &name, 28., cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(h_flex().gap_2().justify_between().child(div().text_sm().truncate().child(name)).child(div().text_xs().font_medium().flex_shrink_0().child(fmt::size(size))))
                            .child(ui::bar(size as f32 / max as f32, theme.primary.opacity(0.75), cx).h(px(4.))),
                    ),
            );
        }

        let left = v_flex()
            .flex_basis(px(420.))
            .flex_grow(1.6)
            .min_w(px(380.))
            .gap_3()
            .child(ui::section_title(
                if total > 0 { format!("Рекомендации · можно освободить до {}", fmt::size(total)) } else { "Рекомендации".into() },
                None,
                cx,
            ))
            .when(!recs.is_empty(), |s| s.child(rec_card))
            .when(recs.is_empty() && p_count == 0, |s| s.child(ui::skeleton_rows(3, 34., cx)))
            .when(recs.is_empty() && p_count > 0, |s| {
                s.child(ui::banner(
                    Tone::Success,
                    Lucide::CircleCheck,
                    "Всё в порядке — рекомендаций нет",
                    None,
                    cx,
                ))
            });
        let right = v_flex()
            .flex_basis(px(340.))
            .flex_grow(1.)
            .min_w(px(300.))
            .gap_3()
            .child(ui::section_title("Самые большие программы", None, cx))
            .child(if p_count == 0 { ui::skeleton_rows(5, 28., cx) } else { top });

        let body = v_flex()
            .gap_4()
            .child(hero)
            .child(tiles)
            .child(ui::section_title("Диски", None, cx))
            .child(drives)
            .child(h_flex().gap_5().items_start().flex_wrap().child(left).child(right));

        ui::page(
            "dashboard-page",
            &format!("{greeting}, {user}"),
            &system,
            Button::new("refresh-dashboard").outline().icon(Icon::new(Lucide::RefreshCw)).label("Обновить").on_click(cx.listener(|this, _, _, cx| {
                this.refresh(cx);
                cx.emit(Navigate::to("reload:all"));
            })),
            body,
            cx,
        )
    }
}
