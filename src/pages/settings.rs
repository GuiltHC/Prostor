//! "Настройки" page (port of ViewModels/SettingsViewModel.cs).

use crate::data::{self, settings};
use crate::fmt;
use crate::pages::Navigate;
use crate::services::{autoclean, explorer, updater};
use crate::sys::shell;
use crate::ui::ask::{self, Ask, Kind as Btn};
use crate::ui::{self, Busy, Lucide, Tone};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::*;

pub struct SettingsView {
    repo: Entity<InputState>,
    explorer_menu: bool,
    auto_clean_registered: bool,
    auto_clean_elsewhere: bool,
    checking: bool,
    scroll: ScrollHandle,
    _tasks: Vec<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<Navigate> for SettingsView {}

const HOTKEYS: &[(&str, &str)] = &[
    ("Ctrl + 1…5", "Обзор, Программы, Анализ, Очистка, Автозагрузка"),
    ("Ctrl + 6", "Журнал"),
    ("Ctrl + ,", "Настройки"),
    ("F5", "Обновить страницу"),
    ("Ctrl + F", "Поиск в программах и автозагрузке"),
    ("Delete", "Удалить выбранное: программы или файл на карте диска"),
    ("Backspace  /  Alt + ←", "На уровень выше в анализе диска"),
    ("Esc", "Закрыть окно диалога"),
];

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let repo_value = settings::get().update_repo.clone();
        let repo = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("владелец/репозиторий");
            s.set_value(repo_value, window, cx);
            s
        });
        let sub = cx.subscribe(&repo, |_, state, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                let v = state.read(cx).value().trim().to_string();
                settings::update(|s| s.update_repo = v);
            }
        });
        let mut v = Self { repo, explorer_menu: false, auto_clean_registered: false, auto_clean_elsewhere: false, checking: false, scroll: ScrollHandle::new(), _tasks: Vec::new(), _subs: vec![sub] };
        v.refresh(cx);
        v
    }

    /// Scrolls to a section: appearance, programs, cleanup, windows, updates, keys.
    pub fn show_section(&mut self, name: &str, cx: &mut Context<Self>) {
        // child 0 is the page title
        let ix = match name {
            "appearance" => 1,
            "programs" => 2,
            "cleanup" => 3,
            "windows" => 4,
            "updates" => 5,
            "keys" => 6,
            _ => return,
        };
        self.scroll.scroll_to_top_of_item(ix);
        cx.notify();
    }

    /// Re-reads the state of the menu and the scheduled task (they can change outside the app).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let work = cx.background_executor().spawn(async { (explorer::is_enabled(), autoclean::is_registered(), autoclean::points_elsewhere()) });
        let t = cx.spawn(async move |this, cx| {
            let (menu, task, elsewhere) = work.await;
            this.update(cx, |v, cx| {
                v.explorer_menu = menu;
                v.auto_clean_registered = task;
                v.auto_clean_elsewhere = elsewhere;
                cx.notify();
            })
            .ok();
        });
        self._tasks.push(t);
    }

    fn set_auto_clean(&mut self, mode: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let work = cx.background_executor().spawn(async move { autoclean::apply(mode) });
        let t = cx.spawn_in(window, async move |this, cx| {
            let r = work.await;
            this.update_in(cx, |v, window, cx| {
                match r {
                    Ok(()) => {
                        settings::update(|s| s.auto_clean = mode.into());
                        v.auto_clean_registered = mode != "off";
                        window.push_notification(
                            Notification::info(match mode {
                                "weekly" => "Автоочистка: по воскресеньям в 12:00 (или при следующем включении компьютера)",
                                "monthly" => "Автоочистка: 1-го числа каждого месяца",
                                _ => "Автоочистка выключена",
                            }),
                            cx,
                        );
                    }
                    Err(e) => ui::error(window, cx, "Не удалось настроить расписание", e),
                }
                cx.notify();
            })
            .ok();
        });
        self._tasks.push(t);
    }

    fn add_exclusion(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: true, prompt: Some("Исключить".into()) });
        let t = cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                settings::update(|s| {
                    for p in paths {
                        let p = p.to_string_lossy().to_string();
                        if !s.exclusions.iter().any(|e| e.eq_ignore_ascii_case(&p)) {
                            s.exclusions.push(p);
                        }
                    }
                });
                this.update(cx, |_, cx| {
                    cx.emit(Navigate::to("reload:cleanup"));
                    cx.notify();
                })
                .ok();
            }
        });
        self._tasks.push(t);
    }

    pub fn check_updates(&mut self, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.checking = true;
        cx.notify();
        let repo = settings::get().update_repo.clone();
        let t = cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_executor().spawn(async move { updater::check(&repo) }).await;
            this.update(cx, |v, cx| {
                v.checking = false;
                cx.notify();
            })
            .ok();
            let release = match r {
                Ok(Some(rel)) => rel,
                Ok(None) => {
                    if manual {
                        cx.update(|w, cx| w.push_notification(Notification::success(format!("У вас последняя версия — {}", updater::VERSION)), cx)).ok();
                    }
                    return;
                }
                Err(e) => {
                    if manual {
                        cx.update(|w, cx| ui::error(w, cx, "Не удалось проверить обновления", e)).ok();
                    }
                    return;
                }
            };
            if !manual && settings::get().skipped_version.as_deref() == Some(release.tag.as_str()) {
                return;
            }
            let notes: String = release.notes.lines().take(12).collect::<Vec<_>>().join("\n");
            let q = Ask::new(
                (Lucide::Download, Tone::Accent),
                format!("Доступна версия {}", release.tag.trim_start_matches(['v', 'V'])),
                format!("Сейчас установлена {}.{}", updater::VERSION, if notes.is_empty() { String::new() } else { format!("\n\n{notes}") }),
            )
            .button("Пропустить версию", Btn::Plain)
            .button("Позже", Btn::Plain)
            .button("Обновить", Btn::Primary)
            .width(560.);
            match ask::ask(cx, q).await {
                Some(0) => settings::update(|s| s.skipped_version = Some(release.tag.clone())),
                Some(2) => {
                    let busy = cx.new(|_| {
                        let mut b = Busy::new("Скачиваем обновление…");
                        b.progress = Some(0.0);
                        b
                    });
                    let b2 = busy.clone();
                    cx.update(|w, cx| ui::open_busy(w, cx, b2)).ok();
                    let pct = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
                    let p2 = pct.clone();
                    let rel = release.clone();
                    let work = cx.background_executor().spawn(async move { updater::install(&rel, |x| p2.store((x * 1000.0) as u32, std::sync::atomic::Ordering::Relaxed)) });
                    let done = std::sync::Arc::new(std::sync::Mutex::new(None));
                    let d2 = done.clone();
                    cx.background_executor()
                        .spawn(async move {
                            let r = work.await;
                            *d2.lock().unwrap() = Some(r);
                        })
                        .detach();
                    let r = loop {
                        if let Some(r) = done.lock().unwrap().take() {
                            break r;
                        }
                        let v = pct.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0;
                        busy.update(cx, |b, cx| {
                            b.progress = Some(v);
                            cx.notify();
                        });
                        cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
                    };
                    cx.update(ui::close_busy).ok();
                    match r {
                        Ok(exe) => {
                            let _ = std::process::Command::new(exe).arg("--new-window").spawn();
                            cx.update(|_, cx| cx.quit()).ok();
                        }
                        Err(e) => {
                            cx.update(|w, cx| ui::error(w, cx, "Обновление не установилось", e)).ok();
                        }
                    }
                }
                _ => {}
            }
        });
        self._tasks.push(t);
    }

    fn section(title: &str, icon: Lucide, cx: &App) -> Div {
        ui::card(cx).p_5().gap_4().child(
            h_flex()
                .gap_3()
                .items_center()
                .child(ui::icon_tile(icon, Tone::Accent, 32., cx))
                .child(div().text_lg().font_semibold().child(title.to_string())),
        )
    }

    fn row(title: &str, note: &str, control: impl IntoElement, cx: &App) -> Div {
        h_flex()
            .gap_4()
            .items_center()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().font_medium().child(title.to_string()))
                    .when(!note.is_empty(), |s| s.child(div().text_xs().text_color(cx.theme().muted_foreground).child(note.to_string()))),
            )
            .child(div().flex_shrink_0().child(control))
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let s = settings::get().clone();
        let admin = shell::is_admin();

        // ---- appearance
        let themes = [("system", "Как в Windows"), ("light", "Светлая"), ("dark", "Тёмная")];
        let theme_opts: Vec<(String, bool)> = themes.iter().map(|(k, l)| (l.to_string(), *k == s.theme)).collect();
        let theme_seg = ui::segmented(
            "theme-mode",
            &theme_opts,
            |_: &mut Self, i, cx| {
                let key = ["system", "light", "dark"][i];
                settings::update(|s| s.theme = key.into());
                ui::theme::set_mode(key, None, cx);
                cx.notify();
            },
            cx,
        );
        let system_accent = s.extra.get("UseSystemAccent").and_then(|v| v.as_bool()).unwrap_or(false);
        let appearance = Self::section("Внешний вид", Lucide::Palette, cx).child(Self::row("Тема", "", theme_seg, cx)).child(Self::row(
            "Цвет акцента Windows",
            if ui::theme::system_accent().is_some() { "Кнопки и выделение в цвет, выбранный в «Параметрах → Персонализация»" } else { "Windows не сообщает цвет акцента" },
            Switch::new("system-accent").checked(system_accent).on_click(|c: &bool, _, cx| {
                ui::theme::set_use_system_accent(*c);
                ui::theme::apply_accent(cx);
            }),
            cx,
        ));
        let appearance = if ui::theme::mica_supported() {
            appearance.child(Self::row(
                "Полупрозрачное окно",
                "Материал Mica из Windows 11: окно слегка окрашивается в цвета обоев",
                Switch::new("mica").checked(ui::theme::mica_enabled()).on_click(|c: &bool, window, cx| {
                    ui::theme::set_use_mica(*c);
                    let mode = settings::get().theme.clone();
                    ui::theme::set_mode(&mode, Some(window), cx);
                }),
                cx,
            ))
        } else {
            appearance
        };

        // ---- programs
        let months = [(3, "3 месяца"), (6, "Полгода"), (12, "Год")];
        let month_opts: Vec<(String, bool)> = months.iter().map(|(m, l)| (l.to_string(), *m == s.unused_months)).collect();
        let month_seg = ui::segmented(
            "unused-months",
            &month_opts,
            |_: &mut Self, i, cx| {
                let m = [3, 6, 12][i];
                settings::update(|s| s.unused_months = m);
                cx.emit(Navigate::to("reload:unused"));
                cx.notify();
            },
            cx,
        );
        let programs = Self::section("Программы", Lucide::AppWindow, cx)
            .child(Self::row(
                "Точка восстановления перед удалением",
                if admin { "Если удаление что-то сломает, систему можно будет вернуть" } else { "Доступно в режиме администратора" },
                Switch::new("restore-point").checked(s.restore_point_before_uninstall && admin).disabled(!admin).on_click(|c: &bool, _, _| {
                    let v = *c;
                    settings::update(|s| s.restore_point_before_uninstall = v);
                }),
                cx,
            ))
            .child(Self::row(
                "Искать остатки после удаления",
                "Папки, записи реестра, ярлыки, задачи и службы, которые оставил деинсталлятор",
                Switch::new("leftovers").checked(s.search_leftovers).on_click(|c: &bool, _, _| {
                    let v = *c;
                    settings::update(|s| s.search_leftovers = v);
                }),
                cx,
            ))
            .child(Self::row("«Не используются» — не запускались дольше", "", month_seg, cx));

        // ---- cleanup
        let mut excl = v_flex().gap_1();
        for (i, e) in s.exclusions.iter().enumerate() {
            let path = e.clone();
            excl = excl.child(
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .bg(theme.muted)
                    .child(Icon::new(Lucide::Ban).size_3p5().text_color(theme.muted_foreground))
                    .child(div().flex_1().min_w_0().text_sm().truncate().child(e.clone()))
                    .child(Button::new(("remove-excl", i)).ghost().xsmall().icon(Icon::new(Lucide::X)).tooltip("Убрать из исключений").on_click(cx.listener(
                        move |_, _, _, cx| {
                            settings::update(|s| s.exclusions.retain(|x| !x.eq_ignore_ascii_case(&path)));
                            cx.emit(Navigate::to("reload:cleanup"));
                            cx.notify();
                        },
                    ))),
            );
        }
        if s.exclusions.is_empty() {
            excl = excl.child(div().text_sm().text_color(theme.muted_foreground).child("Исключений нет"));
        }
        let auto_mode = if self.auto_clean_registered { s.auto_clean.as_str() } else { "off" };
        // the schedule needs the window (for messages), so these pills are built here rather than with ui::segmented
        let mut auto_buttons = h_flex().gap_1().p_1().rounded_lg().bg(theme.muted);
        for (k, l) in [("off", "Выключена"), ("weekly", "Раз в неделю"), ("monthly", "Раз в месяц")] {
            let selected = k == auto_mode;
            auto_buttons = auto_buttons.child(
                div()
                    .id(k)
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .when(selected, |s| s.bg(theme.background).shadow_xs().font_medium())
                    .when(!selected, |s| s.text_color(theme.muted_foreground))
                    .child(l)
                    .on_click(cx.listener(move |this, _, window, cx| this.set_auto_clean(k, window, cx))),
            );
        }
        let mut auto_note = String::from(
            "Очищаются безопасные категории, отмеченные на странице «Очистка» (корзина, гибернация, точки восстановления и драйверы — никогда). Простор запускается Планировщиком Windows, чистит и сразу закрывается.",
        );
        if let Some(last) = s.last_auto_clean {
            auto_note.push_str(&format!("\nПоследний запуск: {} в {}, освобождено {}.", last.format("%d.%m.%Y"), last.format("%H:%M"), fmt::size(s.last_auto_clean_freed.max(0) as u64)));
        }
        if self.auto_clean_registered && self.auto_clean_elsewhere {
            auto_note.push_str("\nВнимание: расписание запускает другую копию программы — выберите режим ещё раз, чтобы обновить путь.");
        }
        let cleanup = Self::section("Очистка", Lucide::Sparkles, cx)
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                v_flex()
                                    .child(div().font_medium().child("Исключения"))
                                    .child(div().text_xs().text_color(theme.muted_foreground).child("Папки, которые не трогает ни очистка, ни поиск остатков и дубликатов")),
                            )
                            .child(Button::new("add-excl").outline().small().icon(Icon::new(Lucide::Plus)).label("Добавить папку").on_click(cx.listener(|this, _, _, cx| this.add_exclusion(cx)))),
                    )
                    .child(excl),
            )
            .child(Self::row("Автоочистка по расписанию", "", auto_buttons, cx))
            .child(div().text_xs().text_color(theme.muted_foreground).children(auto_note.lines().map(|l| div().child(l.to_string()))))
            .child(h_flex().child(Button::new("run-auto").ghost().small().icon(Icon::new(Lucide::Play)).label("Запустить автоочистку сейчас").on_click(|_, window, cx| {
                match std::env::current_exe().and_then(|e| std::process::Command::new(e).arg("--auto-clean").spawn()) {
                    Ok(_) => window.push_notification(Notification::info("Автоочистка запущена — уведомление появится в углу экрана"), cx),
                    Err(e) => ui::error(window, cx, "Не удалось запустить", e.to_string()),
                }
            })));

        // ---- integration
        let menu_note = if self.explorer_menu && explorer::points_elsewhere() {
            "Пункт меню запускает другую копию программы — выключите и включите его снова."
        } else {
            "Пункт «Анализировать в «Простор»» в меню папок и дисков. В Windows 11 — в разделе «Показать дополнительные параметры»."
        };
        let integration = Self::section("Интеграция с Windows", Lucide::MonitorCog, cx).child(Self::row(
            "Меню Проводника",
            menu_note,
            Switch::new("explorer-menu").checked(self.explorer_menu).on_click(cx.listener(|this, c: &bool, window, cx| {
                let r = if *c { explorer::enable() } else { explorer::disable();
                Ok(()) };
                match r {
                    Ok(()) => this.explorer_menu = *c,
                    Err(e) => ui::error(window, cx, "Не удалось изменить меню Проводника", e),
                }
                cx.notify();
            })),
            cx,
        ));

        // ---- updates
        let updates = Self::section("Обновления", Lucide::RefreshCw, cx)
            .child(Self::row(
                "Проверять обновления при запуске",
                "Простор спросит, прежде чем что-то скачивать",
                Switch::new("check-updates").checked(s.check_updates).on_click(|c: &bool, _, _| {
                    let v = *c;
                    settings::update(|s| s.check_updates = v);
                }),
                cx,
            ))
            .child(Self::row("Репозиторий GitHub", "Откуда брать новые версии", div().w(px(260.)).child(Input::new(&self.repo)), cx))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().text_sm().text_color(theme.muted_foreground).child(format!("Версия {} · Rust + GPUI", updater::VERSION)))
                    .child(
                        Button::new("check-now")
                            .outline()
                            .small()
                            .label(if self.checking { "Проверяем…" } else { "Проверить сейчас" })
                            .disabled(self.checking)
                            .on_click(cx.listener(|this, _, window, cx| this.check_updates(true, window, cx))),
                    )
                    .child(Button::new("data-folder").ghost().small().icon(Icon::new(Lucide::FolderOpen)).label("Папка данных").on_click(|_, _, _| {
                        let d = data::data_dir();
                        let _ = std::fs::create_dir_all(&d);
                        shell::open_folder(&d);
                    })),
            );

        // ---- hotkeys
        let mut keys = v_flex().gap_2();
        for (k, a) in HOTKEYS {
            // "Ctrl + 6  /  Ctrl + ," → keycaps, with a quiet "или" between the alternatives
            let mut caps = h_flex().w(px(220.)).flex_shrink_0().gap_1().flex_wrap();
            for (alt_ix, alt) in k.split("  /  ").enumerate() {
                if alt_ix > 0 {
                    caps = caps.child(div().px_1().text_xs().text_color(theme.muted_foreground).child("или"));
                }
                for key in alt.split(" + ") {
                    caps = caps.child(
                        div()
                            .min_w(px(22.))
                            .px_1p5()
                            .py(px(1.))
                            .rounded_md()
                            .border_1()
                            .border_b_2()
                            .border_color(ui::card_border(&theme))
                            .bg(theme.foreground.opacity(0.04))
                            .text_xs()
                            .font_medium()
                            .text_center()
                            .child(key.to_string()),
                    );
                }
            }
            keys = keys.child(
                h_flex()
                    .gap_4()
                    .child(caps)
                    .child(div().text_sm().text_color(theme.muted_foreground).child(a.to_string())),
            );
        }
        let hotkeys = Self::section("Горячие клавиши", Lucide::Keyboard, cx).child(keys);

        // sections are direct children of the scroll area, so a section can be scrolled to
        let column = |e: Div| e.w_full().max_w(px(1240.)).mx_auto();
        div()
            .size_full()
            .relative()
            .child(
                v_flex()
                    .id("settings-page")
                    .size_full()
                    .track_scroll(&self.scroll)
                    .overflow_y_scroll()
                    .px_8()
                    .pt_7()
                    .pb_10()
                    .gap_4()
                    .child(column(div().pb_1().text_size(px(26.)).line_height(px(32.)).font_semibold().child("Настройки")))
                    .child(column(appearance))
                    .child(column(programs))
                    .child(column(cleanup))
                    .child(column(integration))
                    .child(column(updates))
                    .child(column(hotkeys)),
            )
            .vertical_scrollbar(&self.scroll)
    }
}
