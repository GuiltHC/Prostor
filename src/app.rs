//! Main window: title bar, navigation sidebar, pages, hotkeys and navigation between pages.

use crate::data::settings;
use crate::pages::Navigate;
use crate::pages::analyzer::AnalyzerView;
use crate::pages::cleanup::CleanupView;
use crate::pages::dashboard::DashboardView;
use crate::pages::journal::JournalView;
use crate::pages::programs::ProgramsView;
use crate::pages::settings::SettingsView;
use crate::pages::startup::StartupView;
use crate::ui::{self, Lucide};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, StyledExt as _, ThemeMode, TitleBar, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;

gpui_kit::actions!(prostor, [GoDashboard, GoPrograms, GoAnalyzer, GoCleanup, GoStartup, GoJournal, GoSettings, RefreshPage, FocusSearch, DeleteSelected]);

/// Registers the window hotkeys (call once at start).
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-1", GoDashboard, None),
        KeyBinding::new("ctrl-2", GoPrograms, None),
        KeyBinding::new("ctrl-3", GoAnalyzer, None),
        KeyBinding::new("ctrl-4", GoCleanup, None),
        KeyBinding::new("ctrl-5", GoStartup, None),
        KeyBinding::new("ctrl-6", GoJournal, None),
        KeyBinding::new("ctrl-,", GoSettings, None),
        KeyBinding::new("f5", RefreshPage, None),
        KeyBinding::new("ctrl-f", FocusSearch, None),
        // a focused text field handles Delete itself (its key context is deeper)
        KeyBinding::new("delete", DeleteSelected, None),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Dashboard,
    Programs,
    Analyzer,
    Cleanup,
    Startup,
    Journal,
    Settings,
}

impl Page {
    const NAV: [Page; 6] = [Page::Dashboard, Page::Programs, Page::Analyzer, Page::Cleanup, Page::Startup, Page::Journal];

    pub fn name(self) -> &'static str {
        match self {
            Page::Dashboard => "dashboard",
            Page::Programs => "programs",
            Page::Analyzer => "analyzer",
            Page::Cleanup => "cleanup",
            Page::Startup => "startup",
            Page::Journal => "journal",
            Page::Settings => "settings",
        }
    }

    pub fn from_name(s: &str) -> Option<Page> {
        Self::NAV.into_iter().chain([Page::Settings]).find(|p| p.name().eq_ignore_ascii_case(s))
    }

    fn title(self) -> &'static str {
        match self {
            Page::Dashboard => "Обзор",
            Page::Programs => "Программы",
            Page::Analyzer => "Анализ диска",
            Page::Cleanup => "Очистка",
            Page::Startup => "Автозагрузка",
            Page::Journal => "Журнал",
            Page::Settings => "Настройки",
        }
    }

    fn icon(self) -> Lucide {
        match self {
            Page::Dashboard => Lucide::LayoutDashboard,
            Page::Programs => Lucide::AppWindow,
            Page::Analyzer => Lucide::ChartPie,
            Page::Cleanup => Lucide::Sparkles,
            Page::Startup => Lucide::Rocket,
            Page::Journal => Lucide::BookOpen,
            Page::Settings => Lucide::Settings,
        }
    }
}

pub struct ProstorApp {
    page: Page,
    focus: FocusHandle,
    dashboard: Entity<DashboardView>,
    programs: Entity<ProgramsView>,
    analyzer: Entity<AnalyzerView>,
    cleanup: Entity<CleanupView>,
    startup: Entity<StartupView>,
    journal: Entity<JournalView>,
    settings: Entity<SettingsView>,
    _subscriptions: Vec<Subscription>,
}

impl ProstorApp {
    pub fn new(start_page: Option<Page>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let analyzer = cx.new(|cx| AnalyzerView::new(window, cx));
        let cleanup = cx.new(CleanupView::new);
        let startup = cx.new(|cx| StartupView::new(window, cx));
        let programs = cx.new(|cx| ProgramsView::new(window, cx));
        let journal = cx.new(JournalView::new);
        let settings_view = cx.new(|cx| SettingsView::new(window, cx));
        let dashboard = cx.new(|cx| DashboardView::new(programs.clone(), cleanup.clone(), startup.clone(), cx));

        let mut subs = Vec::new();
        // follow the system light/dark setting while the window is open (unless the user picked one)
        subs.push(cx.observe_window_appearance(window, |_, window, cx| {
            if settings::get().theme == "system" {
                ui::theme::set_mode("system", Some(window), cx);
            }
        }));
        subs.push(cx.subscribe(&cleanup, |this: &mut Self, _, ev: &Navigate, cx| this.navigate(&ev.0, cx)));
        subs.push(cx.subscribe(&dashboard, |this: &mut Self, _, ev: &Navigate, cx| this.navigate(&ev.0, cx)));
        subs.push(cx.subscribe(&journal, |this: &mut Self, _, ev: &Navigate, cx| this.navigate(&ev.0, cx)));
        subs.push(cx.subscribe(&settings_view, |this: &mut Self, _, ev: &Navigate, cx| this.navigate(&ev.0, cx)));
        // counters in the navigation follow the pages
        subs.push(cx.observe(&programs, |_, _, cx| cx.notify()));
        subs.push(cx.observe(&cleanup, |_, _, cx| cx.notify()));
        subs.push(cx.observe(&startup, |_, _, cx| cx.notify()));
        if let Ok(exe) = std::env::current_exe() {
            crate::sys::icons::load(&exe);
        }
        ui::theme::apply_window(window, cx);
        // whoever changes the theme, the backdrop and the window frame follow it
        subs.push(cx.observe_global_in::<gpui_kit::component::Theme>(window, |_, window, cx| ui::theme::apply_window(window, cx)));

        let page = start_page.or_else(|| settings::get().last_page.as_deref().and_then(Page::from_name)).unwrap_or(Page::Dashboard);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        // remember the size and position for the next start
        window.on_window_should_close(cx, |window, _| {
            if std::env::var("PROSTOR_DISPLAY").is_err() {
                let (b, maximized) = match window.window_bounds() {
                    WindowBounds::Windowed(b) => (b, false),
                    WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => (b, true),
                };
                settings::update(|s| {
                    s.window = Some(settings::WindowBounds {
                        left: b.origin.x.as_f32() as f64,
                        top: b.origin.y.as_f32() as f64,
                        width: b.size.width.as_f32() as f64,
                        height: b.size.height.as_f32() as f64,
                        maximized,
                    })
                });
            }
            true
        });

        if settings::get().check_updates {
            settings_view.update(cx, |s, cx| s.check_updates(false, window, cx));
        }
        Self { page, focus, dashboard, programs, analyzer, cleanup, startup, journal, settings: settings_view, _subscriptions: subs }
    }

    /// Arguments of a later launch handed over by `sys::instance`.
    pub fn handle_args(&mut self, args: &crate::cli::Args, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = &args.theme {
            ui::theme::set_mode(t, Some(window), cx);
        }
        if let Some(p) = args.page.as_deref().and_then(Page::from_name) {
            self.open(p, cx);
        }
        if let Some(s) = &args.scan {
            self.analyze(PathBuf::from(s), cx);
        }
        if let Some(cmd) = &args.go {
            #[cfg(debug_assertions)]
            if let Some(what) = cmd.strip_prefix("demo:") {
                return demo(what, window, cx);
            }
            self.navigate(cmd, cx);
        }
    }

    /// Page names and commands from other pages (see `Navigate`).
    pub fn navigate(&mut self, cmd: &str, cx: &mut Context<Self>) {
        if let Some(p) = Page::from_name(cmd) {
            return self.open(p, cx);
        }
        let (verb, arg) = cmd.split_once(':').unwrap_or((cmd, ""));
        match verb {
            "analyze" => self.analyze(PathBuf::from(arg), cx),
            "settings" => {
                self.open(Page::Settings, cx);
                self.settings.update(cx, |v, cx| v.show_section(arg, cx));
            }
            "tab" => {
                self.open(Page::Analyzer, cx);
                self.analyzer.update(cx, |a, cx| a.show_tab(arg, cx));
            }
            "analyze-tab" => {
                let (tab, path) = arg.split_once(':').unwrap_or(("", arg));
                let (tab, path) = (tab.to_string(), PathBuf::from(path));
                self.open(Page::Analyzer, cx);
                self.analyzer.update(cx, |a, cx| a.start_scan_with_tab(path, &tab, cx));
            }
            "programs" => {
                self.open(Page::Programs, cx);
                self.programs.update(cx, |p, cx| match arg {
                    "unused" => p.show_unused(cx),
                    "updates" => p.show_updates(cx),
                    _ => {}
                });
            }
            "program" => {
                let name = arg.to_string();
                self.open(Page::Programs, cx);
                self.programs.update(cx, |p, cx| p.select_by_name(&name, cx));
            }
            "reload" => match arg {
                "programs" => self.programs.update(cx, |p, cx| p.load(cx)),
                "startup" => self.startup.update(cx, |p, cx| p.load(cx)),
                "cleanup" => self.cleanup.update(cx, |p, cx| p.scan(cx)),
                "unused" => self.programs.update(cx, |p, cx| {
                    p.refresh_unused();
                    cx.notify();
                }),
                "all" => {
                    self.programs.update(cx, |p, cx| p.load(cx));
                    self.startup.update(cx, |p, cx| p.load(cx));
                    self.cleanup.update(cx, |p, cx| p.scan(cx));
                }
                _ => {}
            },
            _ => {}
        }
    }

    pub fn open(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.page == page {
            return;
        }
        self.page = page;
        settings::update(|s| s.last_page = Some(page.name().to_string()));
        if page == Page::Settings {
            self.settings.update(cx, |s, cx| s.refresh(cx));
        }
        cx.notify();
    }

    pub fn analyze(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.open(Page::Analyzer, cx);
        self.analyzer.update(cx, |a, cx| a.start_scan(path, cx));
    }

    fn refresh_page(&mut self, cx: &mut Context<Self>) {
        match self.page {
            Page::Dashboard => {
                self.dashboard.update(cx, |d, cx| d.refresh(cx));
                self.navigate("reload:all", cx);
            }
            Page::Programs => self.programs.update(cx, |p, cx| p.load(cx)),
            Page::Cleanup => self.cleanup.update(cx, |p, cx| p.scan(cx)),
            Page::Startup => self.startup.update(cx, |p, cx| p.load(cx)),
            Page::Settings => self.settings.update(cx, |p, cx| p.refresh(cx)),
            Page::Analyzer | Page::Journal => {}
        }
    }

    /// Small counter next to a navigation item.
    fn nav_badge(&self, page: Page, cx: &App) -> Option<(String, bool)> {
        match page {
            Page::Programs => {
                let p = self.programs.read(cx);
                let updates = p.updates();
                if updates > 0 {
                    Some((updates.to_string(), true))
                } else {
                    (p.count() > 0).then(|| (p.count().to_string(), false))
                }
            }
            Page::Cleanup => {
                let c = self.cleanup.read(cx);
                let found = c.total_found();
                (!c.is_scanning() && found >= 50 << 20).then(|| (crate::fmt::size(found), true))
            }
            Page::Startup => {
                let n = self.startup.read(cx).enabled_count();
                (n > 0).then(|| (n.to_string(), false))
            }
            _ => None,
        }
    }

    fn nav_item(&self, page: Page, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active = self.page == page;
        let badge = self.nav_badge(page, cx);
        let soft = if theme.is_dark() { 0.075 } else { 0.06 };
        h_flex()
            .id(page.name())
            .relative()
            .h(px(36.))
            .px_3()
            .gap_3()
            .rounded_lg()
            .cursor_pointer()
            .text_sm()
            .when(active, |s| s.bg(theme.foreground.opacity(soft)).font_medium())
            .when(!active, |s| s.text_color(theme.foreground.opacity(0.78)).hover(|s| s.bg(theme.foreground.opacity(soft * 0.6)).text_color(theme.foreground)))
            .active(|s| s.bg(theme.foreground.opacity(soft * 1.4)))
            .child(
                // the accent pill that marks the open page, as in Windows 11 navigation
                div()
                    .absolute()
                    .left_0()
                    .top(px(10.))
                    .w(px(3.))
                    .h(px(16.))
                    .rounded_full()
                    .bg(if active { theme.primary } else { gpui_kit::transparent_black() }),
            )
            .child(Icon::new(page.icon()).size_4().text_color(if active { theme.primary } else { theme.muted_foreground }))
            .child(div().flex_1().min_w_0().truncate().child(page.title()))
            .when_some(badge, |s, (text, strong)| {
                s.child(
                    div()
                        .flex_shrink_0()
                        .px(px(7.))
                        .py(px(1.))
                        .rounded_full()
                        .text_xs()
                        .font_medium()
                        .when(strong, |b| b.bg(theme.primary.opacity(0.16)).text_color(theme.primary))
                        .when(!strong, |b| b.text_color(theme.muted_foreground))
                        .flex()
                        .items_center()
                        .gap_0p5()
                        // programs: an arrow says "updates", not "how many programs"
                        .when(strong && page == Page::Programs, |b| b.child(Icon::new(Lucide::ArrowUp).size_3()))
                        .child(text),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| this.open(page, cx)))
    }

    /// The system drive at a glance, at the bottom of the navigation.
    fn render_disk(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let root = crate::sys::drives::system_root();
        let space = crate::sys::drives::space(&root);
        let (total, free) = space.map(|s| (s.total, s.free)).unwrap_or((0, 0));
        let used = if total == 0 { 0.0 } else { 1.0 - free as f32 / total as f32 };
        let color = if used > 0.9 { theme.danger } else if used > 0.8 { theme.warning } else { theme.primary };
        let target = root.clone();
        v_flex()
            .id("nav-disk")
            .mx_1()
            .p_3()
            .gap_2()
            .rounded_xl()
            .bg(theme.foreground.opacity(if theme.is_dark() { 0.045 } else { 0.035 }))
            .border_1()
            .border_color(theme.foreground.opacity(0.06))
            .cursor_pointer()
            .hover(|s| s.bg(theme.foreground.opacity(if theme.is_dark() { 0.07 } else { 0.055 })))
            .on_click(cx.listener(move |this, _, _, cx| this.analyze(PathBuf::from(&target), cx)))
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(Lucide::HardDrive).size_3p5().text_color(theme.muted_foreground))
                    .child(div().text_xs().font_medium().child(format!("Диск {}", root.trim_end_matches('\\'))))
                    .child(div().flex_1())
                    .child(div().text_xs().text_color(color).font_medium().child(crate::fmt::percent(used as f64))),
            )
            .child(ui::bar(used, color, cx).h(px(5.)))
            .child(div().text_xs().text_color(theme.muted_foreground).child(format!("Свободно {} из {}", crate::fmt::size(free), crate::fmt::size(total))))
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let dark = theme.is_dark();
        let group = |label: &'static str| div().px_3().pt_4().pb_1().text_xs().font_medium().text_color(theme.muted_foreground.opacity(0.8)).child(label);
        v_flex()
            .w(px(236.))
            .flex_shrink_0()
            .h_full()
            .px_2()
            .pb_3()
            .gap_0p5()
            .child(self.nav_item(Page::Dashboard, cx))
            .child(group("Место на диске"))
            .child(self.nav_item(Page::Analyzer, cx))
            .child(self.nav_item(Page::Cleanup, cx))
            .child(group("Система"))
            .child(self.nav_item(Page::Programs, cx))
            .child(self.nav_item(Page::Startup, cx))
            .child(self.nav_item(Page::Journal, cx))
            .child(div().flex_1())
            .child(self.render_disk(cx))
            .child(
                h_flex()
                    .pt_2()
                    .gap_1()
                    .child(div().flex_1().child(self.nav_item(Page::Settings, cx)))
                    .child(
                        // the title bar is a drag area on Windows, so the theme switch lives here
                        Button::new("theme")
                            .ghost()
                            .icon(Icon::new(if dark { Lucide::Sun } else { Lucide::Moon }))
                            .tooltip(if dark { "Светлая тема" } else { "Тёмная тема" })
                            .on_click(|_, window, cx| {
                                let mode = if cx.theme().is_dark() { ThemeMode::Light } else { ThemeMode::Dark };
                                settings::update(|s| s.theme = if mode.is_dark() { "dark".into() } else { "light".into() });
                                crate::ui::theme::set_mode(if mode.is_dark() { "dark" } else { "light" }, Some(window), cx);
                            }),
                    ),
            )
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let exe = std::env::current_exe().ok();
        TitleBar::new().bg(gpui_kit::transparent_black()).border_0().child(
            h_flex()
                .gap_2()
                .child(ui::app_icon(exe.as_deref(), "Простор", 16., cx))
                .child(div().text_xs().font_medium().child("Простор"))
                .when(crate::sys::shell::is_admin(), |s| {
                    s.child(
                        div()
                            .px_1p5()
                            .rounded_sm()
                            .text_size(px(10.))
                            .bg(theme.warning.opacity(0.15))
                            .text_color(theme.warning)
                            .child("администратор"),
                    )
                }),
        )
    }
}

impl Render for ProstorApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.page {
            Page::Dashboard => self.dashboard.clone().into_any_element(),
            Page::Programs => self.programs.clone().into_any_element(),
            Page::Analyzer => self.analyzer.clone().into_any_element(),
            Page::Cleanup => self.cleanup.clone().into_any_element(),
            Page::Startup => self.startup.clone().into_any_element(),
            Page::Journal => self.journal.clone().into_any_element(),
            Page::Settings => self.settings.clone().into_any_element(),
        };
        let theme = cx.theme().clone();
        let mica = ui::theme::mica_enabled();
        // without Mica the window gets the slightly darker "chrome" colour the content layer sits on
        let chrome = if theme.is_dark() { hsla(240. / 360., 0.06, 0.06, 1.0) } else { hsla(240. / 360., 0.16, 0.95, 1.0) };
        v_flex()
            .id("prostor-root")
            .track_focus(&self.focus)
            .size_full()
            .when(!mica, |s| s.bg(chrome))
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &GoDashboard, _, cx| this.open(Page::Dashboard, cx)))
            .on_action(cx.listener(|this, _: &GoPrograms, _, cx| this.open(Page::Programs, cx)))
            .on_action(cx.listener(|this, _: &GoAnalyzer, _, cx| this.open(Page::Analyzer, cx)))
            .on_action(cx.listener(|this, _: &GoCleanup, _, cx| this.open(Page::Cleanup, cx)))
            .on_action(cx.listener(|this, _: &GoStartup, _, cx| this.open(Page::Startup, cx)))
            .on_action(cx.listener(|this, _: &GoJournal, _, cx| this.open(Page::Journal, cx)))
            .on_action(cx.listener(|this, _: &GoSettings, _, cx| this.open(Page::Settings, cx)))
            .on_action(cx.listener(|this, _: &RefreshPage, _, cx| this.refresh_page(cx)))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| match this.page {
                Page::Programs => this.programs.update(cx, |p, cx| p.focus_search(window, cx)),
                Page::Startup => this.startup.update(cx, |p, cx| p.focus_search(window, cx)),
                _ => {}
            }))
            .on_action(cx.listener(|this, _: &DeleteSelected, window, cx| match this.page {
                Page::Programs => this.programs.update(cx, |p, cx| p.uninstall_selection(window, cx)),
                Page::Analyzer => this.analyzer.update(cx, |a, cx| a.delete_selection(window, cx)),
                _ => {}
            }))
            // a folder or drive dropped on the window is analysed
            .drag_over::<ExternalPaths>(|style, _, _, cx| style.bg(cx.theme().primary.opacity(0.06)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                if let Some(p) = paths.paths().iter().find(|p| p.is_dir()).or_else(|| paths.paths().first()) {
                    let target = if p.is_dir() { p.clone() } else { p.parent().map(|x| x.to_path_buf()).unwrap_or_else(|| p.clone()) };
                    this.analyze(target, cx);
                }
            }))
            .child(self.render_title_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_sidebar(cx))
                    .child(
                        // the content layer: lifted off the window, rounded where it meets the chrome
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .rounded_tl(px(10.))
                            .border_t_1()
                            .border_l_1()
                            .border_color(theme.foreground.opacity(if theme.is_dark() { 0.07 } else { 0.08 }))
                            .bg(if mica { theme.background.opacity(if theme.is_dark() { 0.86 } else { 0.78 }) } else { theme.background })
                            .overflow_hidden()
                            .child(
                                // a short fade and lift when the page changes
                                div().size_full().child(content).with_animation(
                                    self.page.name(),
                                    Animation::new(std::time::Duration::from_millis(180)).with_easing(ease_out_quint()),
                                    |el, t| el.opacity(t).mt(px((1.0 - t) * 6.0)),
                                ),
                            ),
                    ),
            )
    }
}

/// Debug builds only: shows the dialogs and notices with harmless fake content, so their look
/// can be checked without deleting anything (`--go demo:busy|confirm|notify`).
#[cfg(debug_assertions)]
fn demo(what: &str, window: &mut Window, cx: &mut Context<ProstorApp>) {
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::component::notification::Notification;
    match what {
        "busy" => {
            let busy = cx.new(|_| {
                let mut b = ui::Busy::new("Удаляем «Пример»");
                b.message = "Ждём, пока закончит работу деинсталлятор…".into();
                b.detail = "Это демонстрация — ничего не удаляется".into();
                b.progress = Some(0.62);
                b.cancel = Some(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)));
                b
            });
            ui::open_busy(window, cx, busy);
            cx.spawn_in(window, async move |_, cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(6)).await;
                cx.update(ui::close_busy).ok();
            })
            .detach();
        }
        "confirm" => ui::confirm(
            window,
            cx,
            (Lucide::Trash, ui::Tone::Danger),
            "Удалить «Пример»?",
            "Это демонстрация диалога.\nКнопка «Удалить» ничего не делает.",
            "Удалить",
            |_, _| {},
        ),
        "notify" => window.push_notification(Notification::success("Освобождено 1,2 ГБ"), cx),
        _ => {}
    }
}
