//! "Очистка" page (port of ViewModels/CleanupViewModel.cs + Views/CleanupView.xaml).

use crate::data::{journal, settings};
use crate::fmt;
use crate::pages::Navigate;
use crate::services::cleanup::{self, Icon as ItemIcon, Item, Measure, Size};
use crate::sys::shell;
use crate::ui::{self, Busy, Lucide, Tone};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct Row {
    item: Arc<Item>,
    checked: bool,
    measure: Measure,
    scanning: bool,
}

impl Row {
    fn bytes(&self) -> u64 {
        match self.measure.size {
            Size::Bytes(b) => b,
            _ => 0,
        }
    }
    fn unknown(&self) -> bool {
        self.measure.size == Size::Unknown
    }
    fn cleanable(&self) -> bool {
        !self.item.locked() && (self.bytes() > 0 || self.unknown())
    }
    fn size_text(&self) -> String {
        match self.measure.size {
            Size::Unmeasured => String::new(),
            Size::Unknown => "Узнаем после".into(),
            Size::Bytes(0) => "Чисто".into(),
            Size::Bytes(b) => format!("{}{}", if self.item.estimate { "≈ " } else { "" }, fmt::size(b)),
        }
    }
}

pub struct CleanupView {
    rows: Vec<Row>,
    scanning: bool,
    cleaning: bool,
    windows_old: u64,
    generation: u64,
    cancel: Arc<AtomicBool>,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<Navigate> for CleanupView {}

fn lucide(i: ItemIcon) -> Lucide {
    match i {
        ItemIcon::Trash => Lucide::Trash,
        ItemIcon::Clock => Lucide::Clock,
        ItemIcon::Image => Lucide::Image,
        ItemIcon::Download => Lucide::Download,
        ItemIcon::Cloud => Lucide::Cloud,
        ItemIcon::Bug => Lucide::Bug,
        ItemIcon::Monitor => Lucide::Monitor,
        ItemIcon::Logs => Lucide::FileClock,
        ItemIcon::Package => Lucide::Package,
        ItemIcon::Moon => Lucide::Moon,
        ItemIcon::Restore => Lucide::ArchiveRestore,
        ItemIcon::Chip => Lucide::Cpu,
        ItemIcon::Browser => Lucide::Globe,
        ItemIcon::Chat => Lucide::MessageCircle,
        ItemIcon::Send => Lucide::Send,
        ItemIcon::Game => Lucide::Gamepad2,
        ItemIcon::Music => Lucide::Music,
        ItemIcon::Code => Lucide::Code,
        ItemIcon::Users => Lucide::Users,
    }
}

impl CleanupView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut v = Self {
            rows: Vec::new(),
            scanning: false,
            cleaning: false,
            windows_old: 0,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            _tasks: Vec::new(),
        };
        v.scan(cx);
        v
    }

    /// Bytes the user can free with the items that have no side effects (for the dashboard).
    pub fn total_found(&self) -> u64 {
        self.rows.iter().filter(|r| !r.item.locked() && r.item.warning.is_none()).map(|r| r.bytes()).sum()
    }

    pub fn is_scanning(&self) -> bool {
        self.scanning
    }

    pub fn scan(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        let generation = self.generation;
        let cancel = self.cancel.clone();
        self.scanning = true;
        cx.notify();

        let build = cx.background_executor().spawn(async move { cleanup::build_items() });
        let task = cx.spawn(async move |this, cx| {
            let items = build.await;
            let ok = this
                .update(cx, |this, cx| {
                    if this.generation != generation {
                        return false;
                    }
                    let previous: Vec<(&'static str, bool)> = this.rows.iter().map(|r| (r.item.id, r.checked)).collect();
                    let s = settings::get();
                    this.rows = items
                        .into_iter()
                        .map(|item| {
                            let checked = if item.locked() {
                                false
                            } else if let Some((_, c)) = previous.iter().find(|(id, _)| *id == item.id) {
                                *c
                            } else {
                                s.cleanup_checked(item.id, item.default_checked)
                            };
                            Row { item: Arc::new(item), checked, measure: Measure::default(), scanning: true }
                        })
                        .collect();
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !ok {
                return;
            }

            // measure everything in parallel; rows fill in as results arrive
            let items: Vec<Arc<Item>> = this.update(cx, |this, _| this.rows.iter().map(|r| r.item.clone()).collect()).unwrap_or_default();
            let (tx, mut rx) = futures::channel::mpsc::unbounded::<(&'static str, Measure)>();
            for item in items {
                let (cancel, tx) = (cancel.clone(), tx.clone());
                cx.background_executor()
                    .spawn(async move {
                        let m = cleanup::measure(&item, &cancel);
                        let _ = tx.unbounded_send((item.id, m));
                    })
                    .detach();
            }
            drop(tx);
            while let Some((id, m)) = futures::StreamExt::next(&mut rx).await {
                let alive = this
                    .update(cx, |this, cx| {
                        if this.generation != generation {
                            return false;
                        }
                        if let Some(r) = this.rows.iter_mut().find(|r| r.item.id == id) {
                            r.measure = m;
                            r.scanning = false;
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    return;
                }
            }
            let old = cx.background_executor().spawn(async { cleanup::windows_old_size() }).await;
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.windows_old = old;
                    this.scanning = false;
                    cx.notify();
                }
            })
            .ok();
        });
        self._tasks = vec![task];
    }

    fn set_checked(&mut self, id: &'static str, checked: bool, cx: &mut Context<Self>) {
        if let Some(r) = self.rows.iter_mut().find(|r| r.item.id == id) {
            if r.item.locked() {
                return;
            }
            r.checked = checked;
            settings::update(|s| {
                s.cleanup_checks.retain(|k, _| !k.eq_ignore_ascii_case(id));
                s.cleanup_checks.insert(id.to_string(), checked);
            });
            cx.notify();
        }
    }

    /// The deep system items have side effects: "select all" leaves them alone.
    fn toggle_all(&mut self, cx: &mut Context<Self>) {
        let simple: Vec<&'static str> =
            self.rows.iter().filter(|r| !r.item.locked() && r.item.group != cleanup::GROUP_DEEP).map(|r| r.item.id).collect();
        let all = self.rows.iter().filter(|r| simple.contains(&r.item.id) && r.bytes() > 0).all(|r| r.checked);
        for id in simple {
            self.set_checked(id, !all, cx);
        }
    }

    fn selected(&self) -> Vec<&Row> {
        self.rows.iter().filter(|r| r.checked && r.cleanable()).collect()
    }

    fn start_clean(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.selected();
        if selected.is_empty() || self.cleaning || self.scanning {
            return;
        }
        let size: u64 = selected.iter().map(|r| r.bytes()).sum();
        let mut msg = String::from(
            "Будут удалены временные файлы и кэши выбранных категорий. Личные файлы, пароли и настройки не затрагиваются. \
             Файлы, занятые открытыми программами, будут пропущены — для полной очистки браузеров закройте их.",
        );
        let warnings: Vec<String> =
            selected.iter().filter_map(|r| r.item.warning.map(|w| format!("• {}: {w}", r.item.title))).collect();
        if !warnings.is_empty() {
            msg.push_str("\n\nОбратите внимание:\n");
            msg.push_str(&warnings.join("\n"));
        }
        if selected.iter().any(|r| r.item.slow) {
            msg.push_str("\n\nСистемная очистка может занять несколько минут.");
        }
        let title = if size > 0 { format!("Очистить {}?", fmt::size(size)) } else { "Запустить очистку?".into() };
        let this = cx.entity().downgrade();
        ui::confirm(window, cx, (Lucide::Sparkles, Tone::Accent), title, msg, "Очистить", move |window, cx| {
            this.update(cx, |this, cx| this.run_clean(window, cx)).ok();
        });
    }

    fn run_clean(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items: Vec<(Arc<Item>, u64, bool)> = self.selected().iter().map(|r| (r.item.clone(), r.bytes(), r.unknown())).collect();
        if items.is_empty() {
            return;
        }
        self.cleaning = true;
        let cancel = Arc::new(AtomicBool::new(false));
        let busy = cx.new(|_| {
            let mut b = Busy::new("Очищаем…");
            b.cancel = Some(cancel.clone());
            b.progress = Some(0.0);
            b
        });
        ui::open_busy(window, cx, busy.clone());

        let total_known: u64 = items.iter().map(|i| i.1).sum();
        // DISM has no size in advance: give it a fair share of the bar
        let weight = move |bytes: u64, unknown: bool| if unknown { (total_known / 4).max(1 << 30) } else { bytes.max(1) };
        let planned: u64 = items.iter().map(|i| weight(i.1, i.2)).sum::<u64>().max(1);

        let task = cx.spawn_in(window, async move |this, cx| {
            let (mut freed, mut failed, mut done) = (0u64, 0u64, 0u64);
            let mut cleaned: Vec<String> = Vec::new();
            let count = items.len();
            for (n, (item, bytes, unknown)) in items.into_iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let (title, slow) = (item.title.clone(), item.slow || item.special.is_some());
                busy.update(cx, |b, cx| {
                    b.message = if item.slow { format!("{title} — это займёт несколько минут") } else { title.clone() };
                    b.detail = format!("Категория {} из {count} · освобождено {}", n + 1, fmt::size(freed));
                    if slow {
                        b.progress = None;
                    }
                    cx.notify();
                });
                let c = cancel.clone();
                let it = item.clone();
                let r = cx.background_executor().spawn(async move { cleanup::clean(&it, &c) }).await;
                freed += r.freed;
                failed += r.failed;
                done += weight(bytes, unknown);
                if r.freed > 0 || r.deleted > 0 {
                    cleaned.push(item.title.clone());
                }
                if item.id == "hibernation" && r.deleted > 0 {
                    journal::add(
                        journal::Entry::new("cleanup", "Выключена гибернация")
                            .detail("Удалён файл hiberfil.sys")
                            .bytes(r.freed)
                            .undo(journal::UndoKind::Hibernation, Vec::new()),
                    );
                }
                busy.update(cx, |b, cx| {
                    b.progress = Some((done as f32 / planned as f32).min(1.0));
                    cx.notify();
                });
            }
            if freed > 0 {
                let mut detail = cleaned.iter().take(6).cloned().collect::<Vec<_>>().join(", ");
                if cleaned.len() > 6 {
                    detail.push_str(&format!(" и ещё {}", cleaned.len() - 6));
                }
                journal::add(journal::Entry::new("cleanup", "Очистка").detail(detail).bytes(freed));
            }
            crate::services::history::record_async();
            this.update_in(cx, |this, window, cx| {
                this.cleaning = false;
                ui::close_busy(window, cx);
                let text = if failed > 0 {
                    format!("{} пропущено — они используются запущенными программами. Это нормально.", fmt::files(failed))
                } else {
                    "Всё выбранное успешно очищено.".into()
                };
                ui::message(window, cx, (Lucide::CircleCheck, Tone::Success), format!("Освобождено {}", fmt::size(freed)), text);
                this.scan(cx);
            })
            .ok();
        });
        self._tasks.push(task);
    }

    fn restart_as_admin(&mut self, cx: &mut Context<Self>) {
        if shell::run_self_as_admin("--new-window --page cleanup") {
            cx.quit();
        }
    }

    // ---------------- rendering

    fn render_summary(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let selected = self.selected();
        let size: u64 = selected.iter().map(|r| r.bytes()).sum();
        let unknown = selected.iter().any(|r| r.unknown());
        let total = self.total_found();
        let big = if self.scanning && total == 0 { "Анализируем…".to_string() } else { format!("{}{}", fmt::size(size), if unknown { " +" } else { "" }) };
        let summary = format!(
            "Выбрано {} · всего найдено {}{}",
            fmt::count(selected.len() as u64, "категория", "категории", "категорий"),
            fmt::size(total),
            if unknown { " · размер части пунктов станет известен после очистки" } else { "" }
        );
        let can_clean = !self.scanning && !self.cleaning && !selected.is_empty();

        ui::card(cx).p_6().child(
            h_flex()
                .gap_5()
                .items_center()
                .child(
                    div()
                        .size(px(64.))
                        .flex_shrink_0()
                        .rounded(px(18.))
                        .bg(ui::tone_soft(Tone::Success, &theme))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if self.scanning {
                            Spinner::new().color(theme.success).large().into_any_element()
                        } else {
                            gpui_kit::component::Icon::new(Lucide::Sparkles).size_8().text_color(theme.success).into_any_element()
                        }),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .gap_0p5()
                        .child(div().text_sm().text_color(theme.muted_foreground).child("Можно освободить"))
                        .child(div().text_3xl().font_semibold().text_color(theme.success).child(big))
                        .child(div().text_xs().text_color(theme.muted_foreground).child(summary)),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("clean")
                                .primary()
                                .large()
                                .icon(Lucide::Sparkles)
                                .label("Очистить выбранное")
                                .disabled(!can_clean)
                                .on_click(cx.listener(|this, _, window, cx| this.start_clean(window, cx))),
                        )
                        .child(
                            Button::new("toggle-all")
                                .ghost()
                                .small()
                                .label("Выбрать всё / снять")
                                .disabled(self.cleaning)
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_all(cx))),
                        ),
                ),
        )
        .when(size > 0, |card| card.child(self.render_breakdown(&selected, size, cx)))
    }

    /// Where the selected space comes from: one stacked bar and a legend of the largest parts.
    fn render_breakdown(&self, selected: &[&Row], total: u64, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let palette = [theme.primary, theme.success, theme.info, theme.warning, hsla(330. / 360., 0.7, 0.62, 1.0)];
        let mut parts: Vec<(String, u64)> = selected.iter().map(|r| (r.item.title.to_string(), r.bytes())).filter(|(_, b)| *b > 0).collect();
        parts.sort_by_key(|p| std::cmp::Reverse(p.1));
        let rest: u64 = parts.iter().skip(palette.len()).map(|p| p.1).sum();
        parts.truncate(palette.len());
        if rest > 0 {
            parts.push(("Остальное".into(), rest));
        }
        let color = |i: usize| palette.get(i).copied().unwrap_or(theme.muted_foreground.opacity(0.5));
        let mut bar = h_flex().h(px(8.)).w_full().rounded_full().overflow_hidden().gap(px(2.));
        let mut legend = h_flex().gap_x_4().gap_y_1().flex_wrap();
        for (i, (name, bytes)) in parts.iter().enumerate() {
            bar = bar.child(div().h_full().flex_grow(*bytes as f32 / total as f32).flex_basis(px(2.)).bg(color(i)));
            legend = legend.child(
                h_flex()
                    .gap_1p5()
                    .text_xs()
                    .child(div().size(px(8.)).rounded_full().bg(color(i)))
                    .child(div().text_color(theme.muted_foreground).child(name.clone()))
                    .child(div().font_medium().child(fmt::size(*bytes))),
            );
        }
        v_flex().pt_5().gap_2().child(bar).child(legend)
    }

    fn render_row(&self, ix: usize, row: &Row, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let id = row.item.id;
        let locked = row.item.locked();
        let dim = row.measure.size == Size::Bytes(0);
        h_flex()
            .id(("cleanup-row", ix))
            .gap_3()
            .px_3()
            .py_2p5()
            .rounded_lg()
            .when(!locked, |s| s.cursor_pointer().hover(|s| s.bg(theme.foreground.opacity(0.04))))
            .on_click(cx.listener(move |this, _, _, cx| {
                let now = this.rows.iter().find(|r| r.item.id == id).map(|r| r.checked).unwrap_or(false);
                this.set_checked(id, !now, cx);
            }))
            .child(
                Checkbox::new(("chk", ix))
                    .checked(row.checked)
                    .disabled(locked)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| this.set_checked(id, *checked, cx))),
            )
            .child(ui::icon_tile(lucide(row.item.icon), if dim { Tone::Neutral } else { Tone::Accent }, 36., cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().font_medium().child(row.item.title.clone()))
                            .when(row.item.requires_admin, |s| {
                                // as administrator the category simply works: only mention rights when they are missing
                                s.when(locked, |s| s.child(ui::badge("нужны права администратора", Tone::Warning, cx)))
                            })
                            .when(row.item.slow, |s| s.child(ui::badge("несколько минут", Tone::Info, cx))),
                    )
                    .child(div().text_xs().text_color(theme.muted_foreground).child(row.item.description))
                    .when_some(row.item.warning, |s, w| {
                        s.child(
                            h_flex()
                                .gap_1()
                                .text_xs()
                                .text_color(theme.warning)
                                .child(gpui_kit::component::Icon::new(Lucide::TriangleAlert).size_3())
                                .child(w),
                        )
                    }),
            )
            .child(
                v_flex()
                    .items_end()
                    .flex_shrink_0()
                    .min_w(px(110.))
                    .child(if row.scanning {
                        Spinner::new().small().color(theme.muted_foreground).into_any_element()
                    } else {
                        div()
                            .font_semibold()
                            .when(dim, |s| s.text_color(theme.muted_foreground).font_normal())
                            .child(row.size_text())
                            .into_any_element()
                    })
                    .child(div().text_xs().text_color(theme.muted_foreground).child(row.item.count_text(row.measure.count))),
            )
    }
}

impl Render for CleanupView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex().gap_3().child(self.render_summary(cx));

        if !shell::is_admin() && self.rows.iter().any(|r| r.item.requires_admin) {
            body = body.child(ui::banner(
                Tone::Warning,
                Lucide::ShieldAlert,
                "Системные категории (обновления Windows, системный TEMP, глубокая очистка) доступны только с правами администратора.",
                Some(
                    Button::new("admin")
                        .small()
                        .label("Перезапустить как администратор")
                        .on_click(cx.listener(|this, _, _, cx| this.restart_as_admin(cx)))
                        .into_any_element(),
                ),
                cx,
            ));
        }
        let excl = settings::get().exclusions.len() as u64;
        if excl > 0 {
            body = body.child(ui::banner(
                Tone::Neutral,
                Lucide::Ban,
                format!("{} в исключениях — их Простор не трогает", fmt::count(excl, "папка", "папки", "папок")),
                None,
                cx,
            ));
        }
        if self.windows_old > 0 {
            body = body.child(ui::banner(
                Tone::Info,
                Lucide::Info,
                format!(
                    "Папка Windows.old занимает {}. Её безопасно удалить через «Параметры → Система → Память → Временные файлы».",
                    fmt::size(self.windows_old)
                ),
                Some(
                    Button::new("storage")
                        .small()
                        .label("Открыть параметры")
                        .on_click(|_, _, _| {
                            shell::open("ms-settings:storagesense");
                        })
                        .into_any_element(),
                ),
                cx,
            ));
        }

        for group in cleanup::GROUPS {
            let rows: Vec<(usize, &Row)> = self.rows.iter().enumerate().filter(|(_, r)| r.item.group == group).collect();
            if rows.is_empty() {
                continue;
            }
            body = body.child(ui::section_title(group, cleanup::group_hint(group), cx));
            let mut card = ui::card(cx).p_1p5();
            for (ix, row) in rows {
                card = card.child(self.render_row(ix, row, cx));
            }
            body = body.child(card);
        }
        if self.rows.is_empty() {
            body = body.child(ui::skeleton_rows(6, 36., cx));
        }

        let actions = h_flex()
            .gap_2()
            .child(
                Button::new("to-settings")
                    .ghost()
                    .icon(Lucide::Settings2)
                    .label("Исключения и расписание")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Navigate::to("settings:cleanup")))),
            )
            .child(
                Button::new("rescan")
                    .outline()
                    .icon(Lucide::RefreshCw)
                    .label("Проверить снова")
                    .disabled(self.scanning || self.cleaning)
                    .on_click(cx.listener(|this, _, _, cx| this.scan(cx))),
            );

        ui::page(
            "cleanup-page",
            "Очистка",
            "Временные файлы и кэши, которые программы создадут заново. Личные файлы, пароли и настройки не затрагиваются.",
            actions,
            body,
            cx,
        )
    }
}
