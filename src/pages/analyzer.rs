//! "Анализ диска" page (port of ViewModels/AnalyzerViewModel.cs + Views/AnalyzerView.xaml):
//! drive picker, scan (MFT when possible), treemap with actions, file types, old files, old downloads,
//! duplicates and what grew since the previous scan.

use crate::categories;
use crate::data::journal;
use crate::fmt;
use crate::scanner::{self, DirNode, LargeFile, Progress, Stats};
use crate::services::files::{compression, duplicates, mover, snapshots};
use crate::services::history;
use crate::sys::drives::{self, Drive};
use crate::sys::{recycle, shell};
use crate::treemap::{self, Rect};
use crate::ui::ask::{self, Ask, Kind as Btn};
use crate::ui::{self, Busy, Lucide, Tone};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

const TOP_FILES: usize = 300;
/// Items drawn per level; the rest is merged into one "остальное" tile.
const MAX_TILES: usize = 400;
const HEADER_H: f32 = 18.0;
/// How many folder levels the treemap opens inside one view.
const MAX_DEPTH: usize = 5;

gpui_kit::actions!(analyzer, [GoUp]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("backspace", GoUp, Some("Analyzer")), KeyBinding::new("alt-left", GoUp, Some("Analyzer"))]);
}

type Tree = Arc<RwLock<DirNode>>;

#[derive(Clone, PartialEq, Debug)]
enum Target {
    Dir(Vec<usize>),
    /// parent folder path, file name
    File(Vec<usize>, String),
    Rest,
}

struct Hit {
    bounds: Bounds<Pixels>,
    target: Target,
    label: String,
    size: u64,
    category: Option<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Map,
    Types,
    Old,
    Downloads,
    Duplicates,
    Growth,
}

enum ScanState {
    Idle,
    Scanning { progress: Arc<Progress>, started: Instant, target: PathBuf, mft: bool },
    Done,
}

struct Row {
    file: LargeFile,
    checked: bool,
}

enum Dups {
    None,
    Running(Arc<duplicates::Progress>, Arc<AtomicBool>),
    Done(Vec<(duplicates::Group, Vec<bool>)>),
}

pub struct AnalyzerView {
    drives: Vec<Drive>,
    state: ScanState,
    tree: Option<Tree>,
    nav: Vec<usize>,
    stats: Option<Stats>,
    elapsed: Duration,
    used_mft: bool,
    hits: Rc<RefCell<Vec<Hit>>>,
    hovered: Option<usize>,
    selected: Option<Target>,
    /// Category picked in the legend: other tiles on the map are dimmed.
    highlight: Option<u8>,
    /// The window is narrow: the folder list next to the map gets less room.
    narrow: bool,
    tab: Tab,
    pending_tab: Option<Tab>,
    table: Entity<TableState<FilesDelegate>>,
    old_rows: Vec<Row>,
    dl_rows: Vec<Row>,
    dups: Dups,
    growth: Option<Option<snapshots::Comparison>>,
    focus: FocusHandle,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

fn tab_from(s: &str) -> Option<Tab> {
    Some(match s {
        "map" => Tab::Map,
        "types" => Tab::Types,
        "old" | "files" => Tab::Old,
        "downloads" => Tab::Downloads,
        "duplicates" => Tab::Duplicates,
        "growth" => Tab::Growth,
        _ => return None,
    })
}

impl AnalyzerView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| TableState::new(FilesDelegate::default(), window, cx).row_selectable(true));
        let sub = cx.subscribe(&table, |_: &mut Self, table, ev: &TableEvent, cx| {
            if let TableEvent::DoubleClickedRow(ix) = ev
                && let Some(f) = table.read(cx).delegate().files.get(*ix) {
                    shell::show_in_explorer(&f.full_path());
                }
        });
        Self {
            drives: drives::list(),
            state: ScanState::Idle,
            tree: None,
            nav: Vec::new(),
            stats: None,
            elapsed: Duration::ZERO,
            used_mft: false,
            hits: Rc::default(),
            hovered: None,
            selected: None,
            highlight: None,
            narrow: false,
            tab: Tab::Map,
            pending_tab: None,
            table,
            old_rows: Vec::new(),
            dl_rows: Vec::new(),
            dups: Dups::None,
            growth: None,
            focus: cx.focus_handle(),
            _tasks: Vec::new(),
            _subscriptions: vec![sub],
        }
    }

    pub fn is_scanning(&self) -> bool {
        matches!(self.state, ScanState::Scanning { .. })
    }

    /// Switches the result tab without scanning again (the `tab:` command).
    pub fn show_tab(&mut self, tab: &str, cx: &mut Context<Self>) {
        if let Some(t) = tab_from(tab) {
            self.tab = t;
            cx.notify();
        }
    }

    pub fn start_scan_with_tab(&mut self, target: PathBuf, tab: &str, cx: &mut Context<Self>) {
        self.pending_tab = tab_from(tab);
        self.start_scan(target, cx);
    }

    pub fn start_scan(&mut self, target: PathBuf, cx: &mut Context<Self>) {
        self.cancel_scan();
        let progress = Arc::new(Progress::default());
        // reading the MFT covers the whole volume: worth it for a drive, slower than a walk for a small folder
        let is_root = target.to_string_lossy().trim_end_matches('\\').len() <= 2;
        let mft = is_root && scanner::can_use_mft(&target);
        self.state = ScanState::Scanning { progress: progress.clone(), started: Instant::now(), target: target.clone(), mft };
        self.dups = Dups::None;
        self.growth = None;
        self.selected = None;
        self.highlight = None;
        cx.notify();

        let work = cx.background_executor().spawn({
            let progress = progress.clone();
            async move {
                let mut used_mft = false;
                let tree = if mft {
                    match scanner::scan_mft(&target, &progress) {
                        Ok(Some(t)) => {
                            used_mft = true;
                            Some(t)
                        }
                        Ok(None) => None,
                        Err(e) => {
                            crate::data::log_error("mft", &e);
                            progress.set_mft_failed();
                            scanner::scan(&target, &progress)
                        }
                    }
                } else {
                    scanner::scan(&target, &progress)
                }?;
                let root_path = PathBuf::from(&*tree.name);
                let stats = scanner::stats(&tree, &root_path, TOP_FILES);
                // what grew: compare with a scan at least an hour old, then remember this one
                let snap = snapshots::capture(&tree, &tree.name);
                let previous = snapshots::load_previous(&tree.name, chrono::Duration::hours(1));
                let growth = previous.map(|p| snapshots::compare(&p, &snap, 40));
                snapshots::save(&snap);
                Some((Arc::new(RwLock::new(tree)), stats, growth, used_mft))
            }
        });
        let finish = cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| this.finish_scan(result, cx)).ok();
        });
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(120)).await;
                let running = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        this.is_scanning()
                    })
                    .unwrap_or(false);
                if !running {
                    break;
                }
            }
        });
        self._tasks = vec![finish, ticker];
    }

    fn cancel_scan(&mut self) {
        if let ScanState::Scanning { progress, .. } = &self.state {
            progress.cancel.store(true, Ordering::Relaxed);
        }
        if let Dups::Running(_, c) = &self.dups {
            c.store(true, Ordering::Relaxed);
        }
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.cancel_scan();
        self.state = if self.tree.is_some() { ScanState::Done } else { ScanState::Idle };
        self._tasks.clear();
        cx.notify();
    }

    fn finish_scan(&mut self, result: Option<(Tree, Stats, Option<snapshots::Comparison>, bool)>, cx: &mut Context<Self>) {
        let ScanState::Scanning { started, .. } = &self.state else { return };
        self.elapsed = started.elapsed();
        match result {
            Some((tree, stats, growth, mft)) => {
                self.tree = Some(tree);
                self.nav.clear();
                self.used_mft = mft;
                self.growth = Some(growth);
                self.set_stats(stats, cx);
                self.state = ScanState::Done;
                self.tab = self.pending_tab.take().unwrap_or(Tab::Map);
            }
            None => self.state = if self.tree.is_some() { ScanState::Done } else { ScanState::Idle },
        }
        self.drives = drives::list();
        history::record_async();
        cx.notify();
    }

    fn set_stats(&mut self, mut stats: Stats, cx: &mut Context<Self>) {
        let files = std::mem::take(&mut stats.largest);
        self.old_rows = stats.old_files.iter().cloned().map(|file| Row { file, checked: false }).collect();
        self.dl_rows = stats.old_downloads.iter().cloned().map(|file| Row { file, checked: false }).collect();
        self.table.update(cx, |t, cx| {
            t.delegate_mut().files = files;
            t.refresh(cx);
            cx.notify();
        });
        self.stats = Some(stats);
    }

    fn recompute_stats(&mut self, cx: &mut Context<Self>) {
        let Some(tree) = self.tree.clone() else { return };
        let stats = {
            let t = tree.read().unwrap();
            let node = t.at(&self.nav);
            scanner::stats(node, &t.full_path(&self.nav), TOP_FILES)
        };
        self.set_stats(stats, cx);
    }

    fn navigate(&mut self, nav: Vec<usize>, cx: &mut Context<Self>) {
        if self.tree.is_none() {
            return;
        }
        self.nav = nav;
        self.hovered = None;
        self.selected = None;
        self.recompute_stats(cx);
        cx.notify();
    }

    fn go_up(&mut self, cx: &mut Context<Self>) {
        if !self.nav.is_empty() {
            let came_from = self.nav.clone();
            let mut nav = self.nav.clone();
            nav.pop();
            self.navigate(nav, cx);
            self.selected = Some(Target::Dir(came_from));
        }
    }

    /// A row of the folder list: one click selects, a double click opens.
    fn on_row_click(&mut self, target: Target, clicks: usize, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if clicks >= 2 {
            match &target {
                Target::Dir(nav) => {
                    let openable = self.tree.as_ref().is_some_and(|t| {
                        let t = t.read().unwrap();
                        let n = t.at(nav);
                        !n.is_virtual && (!n.dirs.is_empty() || !n.files.is_empty())
                    });
                    if openable {
                        self.navigate(nav.clone(), cx);
                    }
                }
                Target::File(parent, name) => {
                    if let Some(p) = self.path_of(parent) {
                        shell::show_in_explorer(&p.join(name));
                    }
                }
                Target::Rest => {}
            }
            return;
        }
        self.selected = (target != Target::Rest).then_some(target);
        cx.notify();
    }

    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Анализировать".into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.into_iter().next() {
                    this.update(cx, |this, cx| this.start_scan(p, cx)).ok();
                }
        })
        .detach();
    }

    fn hit_at(&self, pos: Point<Pixels>) -> Option<usize> {
        self.hits.borrow().iter().rposition(|h| h.bounds.contains(&pos))
    }

    fn on_tree_click(&mut self, pos: Point<Pixels>, clicks: usize, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(ix) = self.hit_at(pos) else { return };
        let target = self.hits.borrow()[ix].target.clone();
        if clicks >= 2 {
            match &target {
                Target::Dir(nav) => {
                    let is_virtual = self.tree.as_ref().is_some_and(|t| t.read().unwrap().at(nav).is_virtual);
                    if !is_virtual {
                        self.navigate(nav.clone(), cx);
                    }
                }
                Target::File(parent, name) => {
                    if let Some(p) = self.path_of(parent) {
                        shell::show_in_explorer(&p.join(name));
                    }
                }
                Target::Rest => {}
            }
            return;
        }
        self.selected = (target != Target::Rest).then_some(target);
        cx.notify();
    }

    fn path_of(&self, nav: &[usize]) -> Option<PathBuf> {
        Some(self.tree.as_ref()?.read().ok()?.full_path(nav))
    }

    /// (path, is folder, size) of the selected tile.
    fn selection(&self) -> Option<(PathBuf, bool, u64, Target)> {
        let tree = self.tree.as_ref()?.read().ok()?;
        match self.selected.clone()? {
            Target::Dir(nav) => {
                let n = tree.at(&nav);
                (!n.is_virtual).then(|| (tree.full_path(&nav), true, n.size, Target::Dir(nav.clone())))
            }
            Target::File(parent, name) => {
                let f = tree.at(&parent).files.iter().find(|f| *f.name == name)?;
                Some((tree.full_path(&parent).join(&name), false, f.size, Target::File(parent.clone(), name)))
            }
            Target::Rest => None,
        }
    }

    /// Removes deleted paths from the tree and the lists, then refreshes the statistics.
    fn forget_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        if let Some(tree) = self.tree.clone() {
            let mut t = tree.write().unwrap();
            for p in paths {
                if let Some(nav) = t.find(p)
                    && let Some((last, parent)) = nav.split_last() {
                        t.remove(parent, Some(*last), None);
                        // folder indices shift: leave a navigation that went through it
                        if self.nav.starts_with(&nav) {
                            self.nav = parent.to_vec();
                        }
                        continue;
                    }
                if let (Some(parent), Some(name)) = (p.parent(), p.file_name())
                    && let Some(nav) = t.find(parent) {
                        t.remove(&nav, None, Some(&name.to_string_lossy()));
                    }
            }
        }
        self.selected = None;
        let gone = |f: &LargeFile| paths.iter().any(|p| *p == f.full_path());
        self.old_rows.retain(|r| !gone(&r.file));
        self.dl_rows.retain(|r| !gone(&r.file));
        if let Dups::Done(groups) = &mut self.dups {
            for (g, checks) in groups.iter_mut() {
                let mut keep = Vec::new();
                let mut kc = Vec::new();
                for (f, c) in g.files.iter().zip(checks.iter()) {
                    if !paths.contains(&f.path) {
                        keep.push(f.clone());
                        kc.push(*c);
                    }
                }
                g.files = keep;
                *checks = kc;
            }
            groups.retain(|(g, _)| g.files.len() > 1);
        }
        let old_rows = std::mem::take(&mut self.old_rows);
        let dl_rows = std::mem::take(&mut self.dl_rows);
        self.recompute_stats(cx);
        // keep the user's checks in the lists
        self.old_rows = old_rows;
        self.dl_rows = dl_rows;
        cx.notify();
    }

    // ---------------- actions

    /// Delete: the tile selected on the map goes to the Recycle Bin (after the usual confirmation).
    pub fn delete_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((path, ..)) = self.selection() {
            self.delete_paths(vec![path], "delete", window, cx);
        }
    }

    fn delete_paths(&mut self, paths: Vec<PathBuf>, kind: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let total: u64 = paths.iter().map(|p| path_size(self, p)).sum();
        let title = if paths.len() == 1 {
            format!("Переместить «{}» в корзину?", paths[0].file_name().unwrap_or_default().to_string_lossy())
        } else {
            format!("Переместить {} в корзину?", fmt::count(paths.len() as u64, "объект", "объекта", "объектов"))
        };
        let msg = format!("{}Их можно будет вернуть из корзины или из «Журнала», пока корзина не очищена.", if total > 0 { format!("Освободится {} после очистки корзины. ", fmt::size(total)) } else { String::new() });
        let this = cx.entity().downgrade();
        ui::confirm(window, cx, (Lucide::Trash, Tone::Danger), title, msg, "В корзину", move |window, cx| {
            let paths = paths.clone();
            this.update(cx, |v, cx| v.run_delete(paths, total, kind, window, cx)).ok();
        });
    }

    fn run_delete(&mut self, paths: Vec<PathBuf>, total: u64, kind: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let excluded: Vec<PathBuf> = {
            let s = crate::data::settings::get();
            paths.iter().filter(|p| s.is_excluded(&p.to_string_lossy())).cloned().collect()
        };
        let busy = cx.new(|_| Busy::new("Перемещаем в корзину…"));
        ui::open_busy(window, cx, busy);
        let list: Vec<PathBuf> = paths.into_iter().filter(|p| !excluded.contains(p)).collect();
        let work = cx.background_executor().spawn(async move { list.into_iter().map(|p| (recycle::move_to_bin(&p), p)).collect::<Vec<_>>() });
        let t = cx.spawn_in(window, async move |this, cx| {
            let results = work.await;
            this.update_in(cx, |v, window, cx| {
                ui::close_busy(window, cx);
                let done: Vec<PathBuf> = results.iter().filter(|(ok, _)| *ok).map(|(_, p)| p.clone()).collect();
                let failed = results.len() - done.len() + excluded.len();
                if !done.is_empty() {
                    journal::add(
                        journal::Entry::new(
                            kind,
                            if done.len() == 1 {
                                format!("«{}» перемещён в корзину", done[0].file_name().unwrap_or_default().to_string_lossy())
                            } else {
                                format!("{} перемещены в корзину", fmt::count(done.len() as u64, "объект", "объекта", "объектов"))
                            },
                        )
                        .detail(done.first().and_then(|p| p.parent()).map(|p| p.display().to_string()).unwrap_or_default())
                        .bytes(total)
                        .undo(journal::UndoKind::RecycleRestore, done.iter().map(|p| p.to_string_lossy().to_string()).collect()),
                    );
                    v.forget_paths(&done, cx);
                    history::record_async();
                }
                if failed > 0 {
                    window.push_notification(
                        Notification::warning(format!("Не удалось удалить {} — заняты программами, нет прав или в исключениях", fmt::count(failed as u64, "объект", "объекта", "объектов"))),
                        cx,
                    );
                } else {
                    window.push_notification(Notification::success("Перемещено в корзину"), cx);
                }
            })
            .ok();
        });
        self._tasks.push(t);
    }

    fn compress_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, true, size, _)) = self.selection() else { return };
        if !compression::is_supported(&path) {
            ui::error(window, cx, "Сжатие недоступно", "Сжатие NTFS работает только на дисках с файловой системой NTFS.");
            return;
        }
        let t = cx.spawn_in(window, async move |this, cx| {
            let busy = cx.new(|_| Busy::new("Оцениваем, сколько удастся сэкономить…"));
            let b = busy.clone();
            cx.update(|w, cx| ui::open_busy(w, cx, b)).ok();
            let p = path.clone();
            let (saving, physical) = cx
                .background_executor()
                .spawn(async move {
                    let never = AtomicBool::new(false);
                    (compression::estimate_saving(&p, &never), compression::physical_size(&p, &never))
                })
                .await;
            cx.update(ui::close_busy).ok();
            let expected = (physical as f64 * saving) as u64;
            let q = Ask::new(
                (Lucide::Archive, Tone::Accent),
                format!("Сжать «{}»?", path.file_name().unwrap_or_default().to_string_lossy()),
                format!(
                    "Windows сожмёт файлы папки встроенным сжатием NTFS (LZX). Программы и игры продолжат работать как обычно, а место освободится.\n\nСейчас папка занимает {}. Ожидаемая экономия — около {} ({}).\n\nОтменить можно в «Журнале».",
                    fmt::size(physical),
                    fmt::size(expected),
                    fmt::percent(saving)
                ),
            )
            .button("Отмена", Btn::Plain)
            .button("Сжать", Btn::Primary);
            if ask::ask(cx, q).await != Some(1) {
                return;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            let pct = Arc::new(AtomicU32::new(0));
            let busy = cx.new(|_| {
                let mut b = Busy::new(format!("Сжимаем «{}»…", path.file_name().unwrap_or_default().to_string_lossy()));
                b.progress = Some(0.0);
                b.cancel = Some(cancel.clone());
                b
            });
            let b = busy.clone();
            cx.update(|w, cx| ui::open_busy(w, cx, b)).ok();
            let (p, c, pc) = (path.clone(), cancel.clone(), pct.clone());
            let done = Arc::new(Mutex::new(None));
            let d2 = done.clone();
            cx.background_executor()
                .spawn(async move {
                    let r = compression::compress(&p, &|x| pc.store((x * 1000.0) as u32, Ordering::Relaxed), &c);
                    let after = compression::physical_size(&p, &AtomicBool::new(false));
                    *d2.lock().unwrap() = Some((r, after));
                })
                .detach();
            let (r, after) = loop {
                if let Some(x) = done.lock().unwrap().take() {
                    break x;
                }
                let v = pct.load(Ordering::Relaxed) as f32 / 1000.0;
                busy.update(cx, |b, cx| {
                    b.progress = Some(v);
                    b.detail = format!("Выполнено {}", fmt::percent(v as f64));
                    cx.notify();
                });
                cx.background_executor().timer(Duration::from_millis(250)).await;
            };
            cx.update(ui::close_busy).ok();
            let saved = physical.saturating_sub(after);
            if saved > 0 || r.is_ok() {
                journal::add(
                    journal::Entry::new("compress", format!("Сжата папка «{}»", path.file_name().unwrap_or_default().to_string_lossy()))
                        .detail(format!("{} → {}", fmt::size(physical), fmt::size(after)))
                        .bytes(saved)
                        .undo(journal::UndoKind::Decompress, vec![path.to_string_lossy().to_string()]),
                );
            }
            let _ = size;
            match r {
                Ok(()) => {
                    cx.update(|w, cx| w.push_notification(Notification::success(format!("Сжато: освобождено {}", fmt::size(saved))), cx)).ok();
                }
                Err(e) => {
                    cx.update(|w, cx| ui::error(w, cx, "Сжатие прервано", e)).ok();
                }
            }
            history::record_async();
            this.update(cx, |_, cx| cx.notify()).ok();
        });
        self._tasks.push(t);
    }

    fn move_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, true, size, _)) = self.selection() else { return };
        if let Some(reason) = mover::check_can_move(&path) {
            ui::error(window, cx, "Эту папку нельзя перенести", reason);
            return;
        }
        let targets = mover::target_drives(&path, size);
        if targets.is_empty() {
            ui::error(window, cx, "Некуда перенести", "Нет другого диска NTFS, на котором хватило бы места.");
            return;
        }
        let t = cx.spawn_in(window, async move |this, cx| {
            let mut q = Ask::new(
                (Lucide::FolderInput, Tone::Accent),
                format!("Перенести «{}» на другой диск?", path.file_name().unwrap_or_default().to_string_lossy()),
                format!(
                    "Папка ({}) будет скопирована на выбранный диск, а на старом месте останется ссылка — программы продолжат её находить. Вернуть обратно можно в «Журнале».\n\nВыберите диск:",
                    fmt::size(size)
                ),
            )
            .button("Отмена", Btn::Plain);
            for d in targets.iter().take(4) {
                q = q.button(format!("{} · свободно {}", d.letter(), fmt::size(d.free)), Btn::Primary);
            }
            let Some(choice) = ask::ask(cx, q).await.filter(|&c| c > 0) else { return };
            let target = PathBuf::from(&targets[choice - 1].root).join("Prostor").join("Перенесено");
            let _ = std::fs::create_dir_all(&target);
            let cancel = Arc::new(AtomicBool::new(false));
            let pct = Arc::new(AtomicU32::new(0));
            let busy = cx.new(|_| {
                let mut b = Busy::new("Переносим папку…");
                b.message = format!("{} → {}", path.display(), target.display());
                b.progress = Some(0.0);
                b.cancel = Some(cancel.clone());
                b
            });
            let b = busy.clone();
            cx.update(|w, cx| ui::open_busy(w, cx, b)).ok();
            let (p, tg, c, pc) = (path.clone(), target.clone(), cancel.clone(), pct.clone());
            let done = Arc::new(Mutex::new(None));
            let d2 = done.clone();
            cx.background_executor()
                .spawn(async move {
                    let r = mover::move_folder(&p, &tg, &|x| pc.store((x * 1000.0) as u32, Ordering::Relaxed), &c);
                    *d2.lock().unwrap() = Some(r);
                })
                .detach();
            let r = loop {
                if let Some(x) = done.lock().unwrap().take() {
                    break x;
                }
                let v = pct.load(Ordering::Relaxed) as f32 / 1000.0;
                busy.update(cx, |b, cx| {
                    b.progress = Some(v);
                    b.detail = format!("Скопировано {}", fmt::percent(v as f64));
                    cx.notify();
                });
                cx.background_executor().timer(Duration::from_millis(250)).await;
            };
            cx.update(ui::close_busy).ok();
            match r {
                Ok(new) => {
                    journal::add(
                        journal::Entry::new("move", format!("Папка «{}» перенесена", path.file_name().unwrap_or_default().to_string_lossy()))
                            .detail(format!("{} → {}", path.display(), new.display()))
                            .bytes(size)
                            .undo(journal::UndoKind::MoveBack, vec![path.to_string_lossy().to_string(), new.to_string_lossy().to_string()]),
                    );
                    cx.update(|w, cx| w.push_notification(Notification::success(format!("Перенесено: на диске освободилось {}", fmt::size(size))), cx)).ok();
                    history::record_async();
                    this.update(cx, |v, cx| v.forget_paths(std::slice::from_ref(&path), cx)).ok();
                }
                Err(e) => {
                    cx.update(|w, cx| ui::error(w, cx, "Перенос не выполнен", e)).ok();
                }
            }
        });
        self._tasks.push(t);
    }

    fn find_duplicates(&mut self, cx: &mut Context<Self>) {
        let Some(tree) = self.tree.clone() else { return };
        let nav = self.nav.clone();
        let progress = Arc::new(duplicates::Progress::default());
        let cancel = Arc::new(AtomicBool::new(false));
        self.dups = Dups::Running(progress.clone(), cancel.clone());
        cx.notify();
        let work = cx.background_executor().spawn(async move {
            let t = tree.read().unwrap();
            let node = t.at(&nav);
            let path = t.full_path(&nav);
            let skip = duplicates::default_skip(&[]);
            duplicates::find(node, &path, 1 << 20, &skip, &progress, &cancel)
        });
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                let running = this.update(cx, |v, cx| {
                    cx.notify();
                    matches!(v.dups, Dups::Running(..))
                });
                if !running.unwrap_or(false) {
                    break;
                }
            }
        });
        let t = cx.spawn(async move |this, cx| {
            let groups = work.await;
            this.update(cx, |v, cx| {
                // check every copy except the newest one
                v.dups = Dups::Done(
                    groups
                        .into_iter()
                        .map(|g| {
                            let newest = g.files.iter().enumerate().max_by_key(|(_, f)| f.modified).map(|(i, _)| i).unwrap_or(0);
                            let checks = (0..g.files.len()).map(|i| i != newest).collect();
                            (g, checks)
                        })
                        .collect(),
                );
                cx.notify();
            })
            .ok();
        });
        self._tasks.push(ticker);
        self._tasks.push(t);
    }

    // ---------------- rendering

    fn render_start(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mut cards = v_flex().gap_2p5().w_full().max_w(px(600.));
        for (i, d) in self.drives.iter().enumerate() {
            let root = PathBuf::from(&d.root);
            let used = d.used_fraction();
            let color = if used > 0.9 { theme.danger } else if used > 0.8 { theme.warning } else { theme.primary };
            cards = cards.child(
                h_flex()
                    .id(("drive", i))
                    .gap_4()
                    .px_4()
                    .py_3p5()
                    .rounded_xl()
                    .border_1()
                    .border_color(ui::card_border(&theme))
                    .bg(ui::card_bg(&theme))
                    .cursor_pointer()
                    .hover(|s| s.bg(ui::card_hover(&theme)).border_color(theme.primary.opacity(0.4)))
                    .on_click(cx.listener(move |this, _, _, cx| this.start_scan(root.clone(), cx)))
                    .child(ui::icon_tile(if d.removable { Lucide::Usb } else { Lucide::HardDrive }, if used > 0.9 { Tone::Danger } else { Tone::Accent }, 40., cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_baseline()
                                    .child(div().font_semibold().child(d.title()))
                                    .child(div().text_xs().text_color(theme.muted_foreground).child(d.file_system.clone()))
                                    .child(div().flex_1())
                                    .child(div().text_sm().text_color(theme.muted_foreground).child(format!("свободно {} из {}", fmt::size(d.free), fmt::size(d.total)))),
                            )
                            .child(ui::bar(used, color, cx)),
                    )
                    .child(Icon::new(Lucide::ChevronRight).size_4().text_color(theme.muted_foreground)),
            );
        }
        let docs = shell::known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Documents).unwrap_or_else(|| shell::home().join("Documents"));
        let desktop = shell::known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Desktop).unwrap_or_else(|| shell::home().join("Desktop"));
        let videos = shell::known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Videos).unwrap_or_else(|| shell::home().join("Videos"));
        let quick = [
            ("Загрузки", Lucide::Download, shell::downloads()),
            ("Документы", Lucide::FileText, docs),
            ("Рабочий стол", Lucide::Monitor, desktop),
            ("Видео", Lucide::Film, videos),
            ("Папка пользователя", Lucide::User, shell::home()),
        ];
        let mut quick_row = h_flex().gap_2().flex_wrap().justify_center();
        for (i, (label, icon, path)) in quick.into_iter().enumerate() {
            quick_row = quick_row.child(
                Button::new(("quick", i))
                    .outline()
                    .small()
                    .icon(Icon::new(icon))
                    .label(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.start_scan(path.clone(), cx))),
            );
        }
        let mft_note = if shell::is_admin() {
            "Режим администратора: диски NTFS читаются напрямую через MFT — весь диск за секунды."
        } else {
            "В режиме администратора Простор читает таблицу MFT напрямую — это в разы быстрее и видны все папки."
        };
        v_flex()
            .id("analyzer-start")
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .min_h_full()
                    .px_8()
                    .py_10()
                    .items_center()
                    .justify_center()
                    .gap_6()
                    .child(
                        v_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size(px(64.))
                                    .rounded(px(18.))
                                    .bg(linear_gradient(135., linear_color_stop(theme.primary.opacity(0.32), 0.), linear_color_stop(theme.primary.opacity(0.08), 1.)))
                                    .border_1()
                                    .border_color(theme.primary.opacity(0.3))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Icon::new(Lucide::ChartPie).size_8().text_color(theme.primary)),
                            )
                            .child(div().pt_2().text_size(px(26.)).font_semibold().child("Что занимает место?"))
                            .child(div().text_color(theme.muted_foreground).text_center().child("Выберите диск или папку — Простор построит карту и покажет самые крупные файлы.")),
                    )
                    .child(cards)
                    .child(
                        v_flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_xs().font_medium().text_color(theme.muted_foreground).child("Или одну папку"))
                            .child(quick_row)
                            .child(Button::new("pick-folder").ghost().small().icon(Icon::new(Lucide::FolderOpen)).label("Выбрать другую папку…").on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx)))),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(Icon::new(Lucide::Zap).size_3())
                            .child(mft_note),
                    ),
            )
    }

    fn render_scanning(&self, progress: &Progress, started: Instant, target: &Path, mft: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let files = progress.files.load(Ordering::Relaxed);
        let dirs = progress.dirs.load(Ordering::Relaxed);
        let bytes = progress.bytes.load(Ordering::Relaxed);
        let (rec, rec_total) = (progress.records.load(Ordering::Relaxed), progress.records_total.load(Ordering::Relaxed));
        let current = progress.current.lock().map(|c| c.clone()).unwrap_or_default();
        let secs = started.elapsed().as_secs();
        let reading_mft = mft && rec_total > 0 && rec < rec_total && !progress.mft_failed();
        let fraction = if reading_mft {
            Some(rec as f32 / rec_total.max(1) as f32)
        } else {
            let t = target.to_string_lossy().to_string();
            self.drives
                .iter()
                .find(|d| d.root.eq_ignore_ascii_case(&t) || d.letter().eq_ignore_ascii_case(t.trim_end_matches('\\')))
                .map(|d| (bytes as f32 / d.used().max(1) as f32).min(1.0))
        };
        let stats = if reading_mft {
            vec![format!("Читаем MFT: {} из {} записей", fmt::number(rec), fmt::number(rec_total))]
        } else {
            vec![fmt::files(files), fmt::folders(dirs), fmt::size(bytes)]
        };
        let ring = match fraction {
            Some(f) => div()
                .relative()
                .size(px(140.))
                .child(ui::charts::ring(f, theme.primary, theme.foreground.opacity(0.08), 140., 10.))
                .child(
                    v_flex()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .child(div().text_size(px(28.)).font_semibold().child(format!("{}%", (f * 100.0).floor() as u32)))
                        .child(div().text_xs().text_color(theme.muted_foreground).child(fmt::clock(secs))),
                )
                .into_any_element(),
            None => v_flex()
                .gap_2()
                .w(px(420.))
                .child(gpui_kit::component::progress::Progress::new("scan-progress").loading(true))
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .px_8()
            .child(ring)
            .child(div().pt_2().text_xl().font_semibold().child(format!("Сканирую {}", target.display())))
            .when(mft, |s| s.child(ui::badge(if progress.mft_failed() { "MFT недоступна — обходим папки" } else { "быстрый режим MFT" }, Tone::Accent, cx)))
            .child(h_flex().gap_5().text_sm().text_color(theme.muted_foreground).children(stats))
            .child(div().w_full().max_w(px(640.)).text_xs().text_color(theme.muted_foreground).text_center().truncate().child(current))
            .child(Button::new("cancel").outline().label("Остановить").on_click(cx.listener(|this, _, _, cx| this.stop(cx))))
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let labeled = |name: &str, bytes: u64| if bytes > 0 { format!("{name} · {}", fmt::size(bytes)) } else { name.to_string() };
        let s = self.stats.as_ref();
        let growth_n = match &self.growth {
            Some(Some(c)) => c.changes.len(),
            _ => 0,
        };
        let tabs: Vec<(Tab, String)> = vec![
            (Tab::Map, "Карта".into()),
            (Tab::Types, "Типы файлов".into()),
            (Tab::Old, labeled("Старые файлы", s.map(|s| s.old_files_size).unwrap_or(0))),
            (Tab::Downloads, labeled("Загрузки", s.map(|s| s.old_downloads_size).unwrap_or(0))),
            (Tab::Duplicates, "Дубликаты".into()),
            (Tab::Growth, if growth_n > 0 { format!("Что выросло · {growth_n}") } else { "Что выросло".into() }),
        ];
        let opts: Vec<(String, bool)> = tabs.iter().map(|(t, l)| (l.clone(), *t == self.tab)).collect();
        let keys: Vec<Tab> = tabs.iter().map(|(t, _)| *t).collect();
        ui::segmented(
            "analyzer-tabs",
            &opts,
            move |this: &mut Self, i, cx| {
                this.tab = keys[i];
                cx.notify();
            },
            cx,
        )
    }

    fn render_toolbar(&self, tree: &DirNode, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let node = tree.at(&self.nav);
        let mut crumbs = h_flex().gap_1().items_center().min_w_0().overflow_hidden();
        let mut names: Vec<(String, Vec<usize>)> = vec![(tree.name.trim_end_matches('\\').to_string(), vec![])];
        let mut n: &DirNode = tree;
        for (depth, &i) in self.nav.iter().enumerate() {
            n = &n.dirs[i];
            names.push((n.name.to_string(), self.nav[..=depth].to_vec()));
        }
        let last = names.len() - 1;
        for (k, (name, nav)) in names.into_iter().enumerate() {
            if k > 0 {
                crumbs = crumbs.child(div().text_color(theme.muted_foreground).child("›"));
            }
            crumbs = crumbs.child(
                div()
                    .id(("crumb", k))
                    .px_1()
                    .rounded_sm()
                    .when(k == last, |s| s.font_semibold())
                    .when(k != last, |s| s.cursor_pointer().text_color(theme.muted_foreground).hover(|s| s.bg(ui::row_hover(&theme))))
                    .child(name)
                    .on_click(cx.listener(move |this, _, _, cx| this.navigate(nav.clone(), cx))),
            );
        }
        let summary = format!(
            "{} · {} · {}{}",
            fmt::size(node.size),
            fmt::files(node.file_count),
            fmt::folders(node.dir_count),
            if self.nav.is_empty() {
                format!(" · {:.1} с{}", self.elapsed.as_secs_f32(), if self.used_mft { " · MFT" } else { "" }).replace('.', ",")
            } else {
                String::new()
            }
        );
        h_flex()
            .gap_2()
            .items_center()
            .child(
                Button::new("up")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::ArrowUp))
                    .tooltip("На уровень выше (Backspace)")
                    .disabled(self.nav.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.go_up(cx))),
            )
            .child(crumbs.flex_1())
            .child(div().text_sm().text_color(theme.muted_foreground).flex_shrink_0().child(summary))
            .child(Button::new("rescan").ghost().small().icon(Icon::new(Lucide::RefreshCw)).tooltip("Сканировать заново").on_click(cx.listener(|this, _, _, cx| {
                if let Some(p) = this.path_of(&this.nav) {
                    this.start_scan(p, cx);
                }
            })))
            .child(Button::new("new-scan").ghost().small().icon(Icon::new(Lucide::HardDrive)).tooltip("Другой диск или папка").on_click(cx.listener(|this, _, _, cx| {
                this.tree = None;
                this.stats = None;
                this.state = ScanState::Idle;
                this.drives = drives::list();
                cx.notify();
            })))
    }

    fn render_map(&self, tree: &Tree, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let node_size = tree.read().unwrap().at(&self.nav).size;
        let hover_line = {
            let hits = self.hits.borrow();
            match self.hovered.and_then(|i| hits.get(i)) {
                Some(h) => {
                    let pct = if node_size > 0 { fmt::percent(h.size as f64 / node_size as f64) } else { String::new() };
                    let cat = h.category.map(|c| format!(" · {}", categories::info(c).title)).unwrap_or_default();
                    format!("{} — {} ({pct}){cat}", h.label, fmt::size(h.size))
                }
                None => "Щелчок выбирает, двойной щелчок по папке — войти, правая кнопка — на уровень выше.".into(),
            }
        };
        let selected = self.selection();
        let canvas_el = treemap_canvas(tree.clone(), self.nav.clone(), self.hits.clone(), self.hovered, self.selected.clone(), self.highlight, theme.background, theme.foreground, theme.is_dark());
        let map = div()
            .id("treemap")
            .flex_1()
            .size_full()
            .rounded_lg()
            .overflow_hidden()
            .bg(theme.muted)
            .child(canvas_el.size_full())
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                let h = this.hit_at(ev.position);
                if h != this.hovered {
                    this.hovered = h;
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| this.on_tree_click(ev.position, ev.click_count, window, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _: &MouseDownEvent, _, cx| this.go_up(cx)));

        let actions = match &selected {
            Some((path, is_dir, size, _)) => {
                let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
                let is_dir = *is_dir;
                h_flex()
                    .gap_2()
                    .items_center()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(theme.secondary)
                    .child(Icon::new(if is_dir { Lucide::Folder } else { Lucide::File }).size_4().text_color(theme.muted_foreground))
                    .child(div().flex_1().min_w_0().text_sm().truncate().child(format!("{} — {}", path.display(), fmt::size(*size))))
                    .child(Button::new("sel-open").ghost().small().icon(Icon::new(Lucide::ExternalLink)).label("Открыть").on_click(move |_, _, _| {
                        if is_dir {
                            shell::open_folder(&p1);
                        } else {
                            shell::open(&p1.to_string_lossy());
                        }
                    }))
                    .child(Button::new("sel-show").ghost().small().icon(Icon::new(Lucide::FolderOpen)).label("В Проводнике").on_click(move |_, _, _| shell::show_in_explorer(&p2)))
                    .when(is_dir, |s| {
                        s.child(Button::new("sel-compress").ghost().small().icon(Icon::new(Lucide::Archive)).label("Сжать").on_click(cx.listener(|this, _, window, cx| this.compress_selected(window, cx))))
                            .child(Button::new("sel-move").ghost().small().icon(Icon::new(Lucide::FolderInput)).label("Перенести").on_click(cx.listener(|this, _, window, cx| this.move_selected(window, cx))))
                    })
                    .child(Button::new("sel-delete").danger().small().icon(Icon::new(Lucide::Trash)).label("В корзину").on_click(cx.listener(move |this, _, window, cx| {
                        this.delete_paths(vec![p3.clone()], "delete", window, cx)
                    })))
                    .into_any_element()
            }
            None => div().flex_shrink_0().text_sm().text_color(theme.muted_foreground).truncate().child(hover_line.clone()).into_any_element(),
        };

        let list_w = if self.narrow { 290. } else { 350. };
        v_flex()
            .flex_1()
            .min_h_0()
            .gap_2()
            .child(
                h_flex()
                    .flex_1()
                    .min_h(px(240.))
                    .gap_3()
                    .items_stretch()
                    .child(div().w(px(list_w)).flex_shrink_0().h_full().child(ui::card(cx).size_full().overflow_hidden().child(self.render_list(tree, cx))))
                    .child(v_flex().flex_1().min_w_0().h_full().child(map)),
            )
            .child(actions)
            .when(selected.is_some(), |s| s.child(div().text_xs().text_color(theme.muted_foreground).truncate().child(hover_line)))
            .child(self.render_legend(cx).flex_shrink_0())
            .child(
                v_flex()
                    .flex_shrink_0()
                    .h(px(200.))
                    .gap_1()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(div().font_semibold().child("Самые крупные файлы"))
                            .child(div().text_xs().text_color(theme.muted_foreground).child("Двойной щелчок — показать в Проводнике")),
                    )
                    .child(div().flex_1().min_h_0().child(DataTable::new(&self.table).stripe(true).bordered(true).small())),
            )
    }

    /// What the current folder holds, largest first: the list next to the map (as in 1.x).
    fn render_list(&self, tree: &Tree, cx: &mut Context<Self>) -> impl IntoElement {
        const MAX_FILE_ROWS: usize = 200;
        let theme = cx.theme().clone();
        let t = tree.read().unwrap();
        let node = t.at(&self.nav);
        let total = node.size.max(1) as f64;
        struct Row {
            target: Target,
            name: String,
            size: u64,
            icon: Lucide,
            color: Hsla,
            details: String,
        }
        let folder: Hsla = hsla(40. / 360., 0.85, if theme.is_dark() { 0.6 } else { 0.48 }, 1.0);
        let grey: Hsla = hsla(220. / 360., 0.1, 0.55, 1.0);
        let mut rows: Vec<Row> = Vec::with_capacity(node.dirs.len() + node.files.len().min(MAX_FILE_ROWS) + 1);
        for (i, d) in node.dirs.iter().enumerate() {
            let mut nav = self.nav.clone();
            nav.push(i);
            let details = if d.is_virtual {
                "Теневые копии, метаданные и файлы без доступа".to_string()
            } else {
                let mut s = fmt::files(d.file_count);
                if d.access_denied {
                    s.push_str(" · нет доступа");
                }
                s
            };
            rows.push(Row {
                target: Target::Dir(nav),
                name: d.name.to_string(),
                size: d.size,
                icon: if d.is_virtual { Lucide::Lock } else { Lucide::Folder },
                color: if d.is_virtual { grey } else { folder },
                details,
            });
        }
        for f in node.files.iter().take(MAX_FILE_ROWS) {
            let info = categories::info(f.category);
            rows.push(Row {
                target: Target::File(self.nav.clone(), f.name.to_string()),
                name: f.name.to_string(),
                size: f.size,
                icon: Lucide::File,
                color: rgb(info.color).into(),
                details: if f.modified > 0 { fmt::filetime_date(f.modified) } else { info.title.to_string() },
            });
        }
        if node.files.len() > MAX_FILE_ROWS {
            let rest = &node.files[MAX_FILE_ROWS..];
            rows.push(Row {
                target: Target::Rest,
                name: format!("Ещё {}", fmt::files(rest.len() as u64)),
                size: rest.iter().map(|f| f.size).sum(),
                icon: Lucide::File,
                color: grey,
                details: "мелкие файлы этой папки".into(),
            });
        }
        rows.sort_by_key(|r| std::cmp::Reverse(r.size));
        // the path behind each row, for its context menu (none for the virtual folder and "Ещё N")
        let paths: Vec<Option<PathBuf>> = rows
            .iter()
            .map(|r| match &r.target {
                Target::Dir(nav) => (!t.at(nav).is_virtual).then(|| t.full_path(nav)),
                Target::File(parent, name) => Some(t.full_path(parent).join(name)),
                Target::Rest => None,
            })
            .collect();
        drop(t);
        let this = cx.entity().downgrade();

        let mut list = v_flex().gap_0p5().p_1p5();
        for (k, (r, path)) in rows.into_iter().zip(paths).enumerate() {
            // a row is lit when it is the selection or holds it (a tile picked deeper on the map)
            let lit = match (&self.selected, &r.target) {
                (Some(Target::Dir(sel)), Target::Dir(nav)) => sel.starts_with(nav),
                (Some(Target::File(sel_parent, _)), Target::Dir(nav)) => sel_parent.starts_with(nav),
                (Some(sel), target) => sel == target,
                _ => false,
            };
            let fraction = r.size as f64 / total;
            let target = r.target.clone();
            let right_target = r.target.clone();
            let row = h_flex()
                    .id(("node-row", k))
                    .gap_3()
                    .px_2p5()
                    .py_2()
                    .rounded_lg()
                    .cursor_pointer()
                    .when(lit, |s| s.bg(theme.primary.opacity(if theme.is_dark() { 0.16 } else { 0.1 })))
                    .when(!lit, |s| s.hover(|s| s.bg(ui::row_hover(&theme))))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, window, cx| this.on_row_click(target.clone(), ev.click_count, window, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        // the row the menu is for is the selection, as in Explorer
                        if right_target != Target::Rest {
                            this.selected = Some(right_target.clone());
                            cx.notify();
                        }
                    }))
                    .child(Icon::new(r.icon).size_5().flex_shrink_0().text_color(r.color))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(div().flex_1().min_w_0().text_sm().font_medium().truncate().child(r.name))
                                    .child(div().flex_shrink_0().text_sm().font_semibold().child(fmt::size(r.size))),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(div().flex_1().child(ui::bar(fraction as f32, r.color.opacity(0.85), cx).h(px(4.))))
                                    .child(div().max_w(px(130.)).text_xs().text_color(theme.muted_foreground).truncate().child(r.details))
                                    .child(div().w(px(38.)).flex_shrink_0().text_right().text_xs().text_color(theme.muted_foreground).child(fmt::percent(fraction))),
                            ),
                    );
            list = list.child(match path {
                Some(path) => {
                    let (this, target) = (this.clone(), r.target.clone());
                    row.context_menu(move |menu, _, cx| row_menu(menu, cx.theme().danger, this.clone(), target.clone(), path.clone())).into_any_element()
                }
                None => row.into_any_element(),
            });
        }
        div()
            .id("node-list")
            .size_full()
            .overflow_y_scrollbar()
            .child(list)
    }

    fn render_legend(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let mut row = h_flex().gap_x_1().gap_y_0p5().flex_wrap().text_xs();
        if let Some(stats) = &self.stats {
            let total: u64 = stats.categories.iter().map(|c| c.size).sum::<u64>().max(1);
            for c in stats.categories.iter().take(9) {
                let info = categories::info(c.category);
                let cat = c.category;
                let dimmed = self.highlight.is_some_and(|h| h != cat);
                let on = self.highlight == Some(cat);
                row = row.child(
                    h_flex()
                        .id(("legend", cat as usize))
                        .gap_1()
                        .items_center()
                        .px_1p5()
                        .py_0p5()
                        .rounded_md()
                        .cursor_pointer()
                        .when(dimmed, |s| s.opacity(0.45))
                        .when(on, |s| s.bg(theme.foreground.opacity(0.08)))
                        .hover(|s| s.bg(theme.foreground.opacity(0.05)))
                        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Подсветить на карте только этот тип файлов").build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.highlight = if this.highlight == Some(cat) { None } else { Some(cat) };
                            cx.notify();
                        }))
                        .child(div().size(px(10.)).rounded_sm().bg(rgb(info.color)))
                        .child(info.title)
                        .child(div().text_color(theme.muted_foreground).child(format!("{} · {}", fmt::size(c.size), fmt::percent(c.size as f64 / total as f64)))),
                );
            }
        }
        row
    }

    fn render_types(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let Some(stats) = &self.stats else { return div().into_any_element() };
        let total: u64 = stats.categories.iter().map(|c| c.size).sum::<u64>().max(1);
        let parts: Vec<(f32, Hsla)> = stats.categories.iter().map(|c| (c.size as f32 / total as f32, rgb(categories::info(c.category).color).into())).collect();
        let top = stats.categories.first().map(|c| (categories::info(c.category).title, c.size as f64 / total as f64));
        let mut cats = ui::card(cx).p_4().gap_3().child(
            h_flex()
                .gap_5()
                .pb_2()
                .child(
                    div()
                        .relative()
                        .size(px(112.))
                        .child(ui::charts::donut(parts, theme.foreground.opacity(0.06), 112., 14.))
                        .child(
                            v_flex()
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .child(div().text_sm().font_semibold().child(fmt::size(total)))
                                .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{} типов", stats.categories.len()))),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(div().font_semibold().child("По типам"))
                        .when_some(top, |s, (title, share)| {
                            s.child(div().text_sm().text_color(theme.muted_foreground).child(format!("Больше всего места занимает «{title}» — {}", fmt::percent(share))))
                        }),
                ),
        );
        for c in &stats.categories {
            let info = categories::info(c.category);
            let color: Hsla = rgb(info.color).into();
            cats = cats.child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().size(px(10.)).rounded_sm().bg(color))
                            .child(div().flex_1().text_sm().child(info.title))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(fmt::files(c.count)))
                            .child(div().w(px(90.)).text_right().text_sm().font_medium().child(fmt::size(c.size))),
                    )
                    .child(ui::bar(c.size as f32 / total as f32, color, cx).h(px(5.))),
            );
        }
        let max_ext = stats.extensions.first().map(|e| e.size).unwrap_or(1).max(1);
        let mut exts = ui::card(cx).p_4().gap_2().child(div().font_semibold().child("Расширения"));
        for e in stats.extensions.iter().take(30) {
            let color: Hsla = rgb(categories::info(e.category).color).into();
            exts = exts.child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().w(px(90.)).text_sm().font_medium().truncate().child(e.ext.clone()))
                    .child(div().flex_1().child(ui::bar(e.size as f32 / max_ext as f32, color.opacity(0.8), cx).h(px(5.))))
                    .child(div().w(px(80.)).text_xs().text_color(theme.muted_foreground).text_right().child(fmt::files(e.count)))
                    .child(div().w(px(80.)).text_right().text_sm().child(fmt::size(e.size))),
            );
        }
        div()
            .id("types-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .child(h_flex().gap_4().items_start().flex_wrap().child(div().flex_1().min_w(px(340.)).child(cats)).child(div().flex_1().min_w(px(340.)).child(exts)))
            .into_any_element()
    }

    fn render_rows(&self, downloads: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let rows = if downloads { &self.dl_rows } else { &self.old_rows };
        let (count, size) = (rows.iter().filter(|r| r.checked).count(), rows.iter().filter(|r| r.checked).map(|r| r.file.size).sum::<u64>());
        let header = if downloads {
            "Файлы крупнее 10 МБ, которые лежат в «Загрузках» дольше трёх месяцев — обычно это ненужные установщики и архивы."
        } else {
            "Файлы крупнее 10 МБ, которые не открывали и не меняли больше года. Папки Windows и программ не учитываются."
        };
        if rows.is_empty() {
            return v_flex()
                .flex_1()
                .child(ui::empty_state(
                    Lucide::CircleCheck,
                    Tone::Success,
                    if downloads { "В «Загрузках» порядок" } else { "Забытых файлов нет" },
                    header,
                    None,
                    cx,
                ))
                .into_any_element();
        }
        let mut list = v_flex().gap_0p5();
        for (i, r) in rows.iter().enumerate() {
            let path = r.file.full_path();
            let when = if downloads { r.file.created } else { r.file.used.max(r.file.modified) };
            list = list.child(
                h_flex()
                    .id(("file-row", i))
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .hover(|s| s.bg(ui::row_hover(&theme)))
                    .child(Checkbox::new(("file-chk", i)).checked(r.checked).on_click(cx.listener(move |this, c: &bool, _, cx| {
                        let rows = if downloads { &mut this.dl_rows } else { &mut this.old_rows };
                        if let Some(r) = rows.get_mut(i) {
                            r.checked = *c;
                        }
                        cx.notify();
                    })))
                    .child(div().flex_shrink_0().size(px(8.)).rounded_full().bg(rgb(categories::info(r.file.category).color)))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().truncate().child(r.file.name.clone()))
                            .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(r.file.folder.display().to_string())),
                    )
                    .child(div().w(px(120.)).text_xs().text_color(theme.muted_foreground).text_right().child(format!(
                        "{} {}",
                        if downloads { "скачан" } else { "открывался" },
                        fmt::filetime_date(when)
                    )))
                    .child(div().w(px(80.)).text_right().text_sm().font_medium().child(fmt::size(r.file.size)))
                    .child(Button::new(("show-file", i)).ghost().xsmall().icon(Icon::new(Lucide::FolderOpen)).on_click(move |_, _, _| shell::show_in_explorer(&path))),
            );
        }
        let checked_paths: Vec<PathBuf> = rows.iter().filter(|r| r.checked).map(|r| r.file.full_path()).collect();
        v_flex()
            .flex_1()
            .min_h_0()
            .gap_2()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().flex_1().min_w_0().text_sm().text_color(theme.muted_foreground).child(header))
                    .child(Button::new("check-all").ghost().small().label("Отметить все").disabled(rows.is_empty()).on_click(cx.listener(move |this, _, _, cx| {
                        let rows = if downloads { &mut this.dl_rows } else { &mut this.old_rows };
                        let all = rows.iter().all(|r| r.checked);
                        for r in rows.iter_mut() {
                            r.checked = !all;
                        }
                        cx.notify();
                    })))
                    .child(
                        Button::new("delete-files")
                            .danger()
                            .small()
                            .icon(Icon::new(Lucide::Trash))
                            .label(if count > 0 { format!("В корзину · {}", fmt::size(size)) } else { "В корзину".into() })
                            .disabled(count == 0)
                            .on_click(cx.listener(move |this, _, window, cx| this.delete_paths(checked_paths.clone(), "delete", window, cx))),
                    ),
            )
            .child(div().id(if downloads { "dl-scroll" } else { "old-scroll" }).flex_1().min_h_0().overflow_y_scrollbar().child(ui::card(cx).p_1p5().child(list)))
            .into_any_element()
    }

    fn render_dups(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        match &self.dups {
            Dups::None => ui::empty_state(
                Lucide::Copy,
                Tone::Accent,
                "Поиск одинаковых файлов",
                "Простор сравнит файлы крупнее 1 МБ в текущей папке: сначала по размеру, затем по началу и концу, затем по полному содержимому. Папки Windows и программ пропускаются, жёсткие ссылки не считаются копиями.",
                Some(Button::new("find-dups").primary().icon(Icon::new(Lucide::Search)).label("Найти дубликаты").on_click(cx.listener(|this, _, _, cx| this.find_duplicates(cx))).into_any_element()),
                cx,
            )
            .into_any_element(),
            Dups::Running(p, cancel) => {
                let (done, total) = (p.groups_done.load(Ordering::Relaxed), p.groups_total.load(Ordering::Relaxed));
                let cancel = cancel.clone();
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .child(Spinner::new().large())
                    .child(div().font_medium().child(if total == 0 { "Сравниваем размеры…".to_string() } else { format!("Сравниваем содержимое: {done} из {total} групп") }))
                    .child(div().text_sm().text_color(theme.muted_foreground).child(format!("Прочитано {}", fmt::size(p.hashed.load(Ordering::Relaxed)))))
                    .child(Button::new("stop-dups").outline().label("Остановить").on_click(cx.listener(move |this, _, _, cx| {
                        cancel.store(true, Ordering::Relaxed);
                        this.dups = Dups::None;
                        cx.notify();
                    })))
                    .into_any_element()
            }
            Dups::Done(groups) => {
                let wasted: u64 = groups.iter().map(|(g, _)| g.wasted()).sum();
                let checked: Vec<PathBuf> = groups.iter().flat_map(|(g, c)| g.files.iter().zip(c).filter(|(_, c)| **c).map(|(f, _)| f.path.clone())).collect();
                let checked_size: u64 = groups.iter().map(|(g, c)| g.size * c.iter().filter(|x| **x).count() as u64).sum();
                let all_marked: bool = groups.iter().all(|(_, c)| c.iter().filter(|x| !**x).count() >= 1);
                let mut list = v_flex().gap_3();
                if groups.is_empty() {
                    return ui::empty_state(
                        Lucide::CircleCheck,
                        Tone::Success,
                        "Одинаковых файлов не нашлось",
                        "Среди файлов крупнее 1 МБ в этой папке копий нет.",
                        Some(Button::new("redo-dups-empty").outline().icon(Icon::new(Lucide::RefreshCw)).label("Искать заново").on_click(cx.listener(|this, _, _, cx| this.find_duplicates(cx))).into_any_element()),
                        cx,
                    )
                    .into_any_element();
                }
                for (gi, (g, checks)) in groups.iter().enumerate().take(300) {
                    let mut card = ui::card(cx).p_3().gap_1().child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().font_medium().truncate().child(g.name()))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(format!(
                                "{} по {} · лишние {}",
                                fmt::count(g.files.len() as u64, "копия", "копии", "копий"),
                                fmt::size(g.size),
                                fmt::size(g.wasted())
                            ))),
                    );
                    for (fi, (f, c)) in g.files.iter().zip(checks).enumerate() {
                        let path = f.path.clone();
                        card = card.child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(Checkbox::new(("dup", gi * 1000 + fi)).checked(*c).on_click(cx.listener(move |this, v: &bool, _, cx| {
                                    if let Dups::Done(groups) = &mut this.dups
                                        && let Some((_, checks)) = groups.get_mut(gi) {
                                            checks[fi] = *v;
                                        }
                                    cx.notify();
                                })))
                                .child(div().flex_1().min_w_0().text_xs().truncate().child(f.path.parent().map(|p| p.display().to_string()).unwrap_or_default()))
                                .child(div().text_xs().text_color(theme.muted_foreground).child(fmt::filetime_date(f.modified)))
                                .child(Button::new(("dup-show", gi * 1000 + fi)).ghost().xsmall().icon(Icon::new(Lucide::FolderOpen)).on_click(move |_, _, _| shell::show_in_explorer(&path))),
                        );
                    }
                    list = list.child(card);
                }
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(div().flex_1().min_w_0().text_sm().text_color(theme.muted_foreground).child(format!(
                                "{} одинаковых файлов · лишние копии занимают {}. Отмечены все копии, кроме самой новой.",
                                groups.len(),
                                fmt::size(wasted)
                            )))
                            .child(Button::new("redo-dups").ghost().small().icon(Icon::new(Lucide::RefreshCw)).label("Искать заново").on_click(cx.listener(|this, _, _, cx| this.find_duplicates(cx))))
                            .child(
                                Button::new("delete-dups")
                                    .danger()
                                    .small()
                                    .icon(Icon::new(Lucide::Trash))
                                    .label(if checked.is_empty() { "В корзину".to_string() } else { format!("В корзину · {}", fmt::size(checked_size)) })
                                    .disabled(checked.is_empty())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if !all_marked {
                                            ui::error(window, cx, "Нельзя удалить все копии", "В каждой группе оставьте хотя бы один файл неотмеченным.");
                                            return;
                                        }
                                        this.delete_paths(checked.clone(), "duplicates", window, cx)
                                    })),
                            ),
                    )
                    .child(div().id("dups-scroll").flex_1().min_h_0().overflow_y_scrollbar().child(list))
                    .into_any_element()
            }
        }
    }

    fn render_growth(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let Some(Some(c)) = &self.growth else {
            return ui::empty_state(
                Lucide::FileClock,
                Tone::Info,
                "Сравнивать пока не с чем",
                "Простор запомнил это сканирование. Просканируйте эту же папку снова через некоторое время — здесь появятся папки, которые выросли или уменьшились.",
                None,
                cx,
            )
            .into_any_element();
        };
        let delta = c.current_total as i64 - c.previous_total as i64;
        let mut list = v_flex().gap_0p5();
        if c.changes.is_empty() {
            return ui::empty_state(
                Lucide::CircleCheck,
                Tone::Success,
                "Заметных изменений нет",
                format!("С прошлого сканирования ({}) папки почти не изменились.", c.previous_time.format("%d.%m.%Y %H:%M")),
                None,
                cx,
            )
            .into_any_element();
        }
        for (i, ch) in c.changes.iter().enumerate() {
            let full = ch.full.clone();
            let grew = ch.delta() > 0;
            list = list.child(
                h_flex()
                    .id(("growth", i))
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .cursor_pointer()
                    .hover(|s| s.bg(ui::row_hover(&theme)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let found = this.tree.as_ref().and_then(|t| t.read().unwrap().find(&full));
                        if let Some(nav) = found {
                            this.tab = Tab::Map;
                            this.navigate(nav, cx);
                        }
                    }))
                    .child(Icon::new(if grew { Lucide::TrendingUp } else { Lucide::TrendingDown }).size_4().text_color(if grew { theme.danger } else { theme.success }))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().truncate().child(ch.relative.clone()))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(ch.detail())),
                    )
                    .child(div().text_sm().font_semibold().text_color(if grew { theme.danger } else { theme.success }).child(format!(
                        "{}{}",
                        if grew { "+" } else { "−" },
                        fmt::size(ch.delta().unsigned_abs())
                    ))),
            );
        }
        v_flex()
            .flex_1()
            .min_h_0()
            .gap_2()
            .child({
                let up = delta >= 0;
                let color = if up { theme.danger } else { theme.success };
                ui::card(cx).px_4().py_3().child(
                    h_flex()
                        .gap_3()
                        .child(ui::icon_tile(if up { Lucide::TrendingUp } else { Lucide::TrendingDown }, if up { Tone::Danger } else { Tone::Success }, 36., cx))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_baseline()
                                        .child(div().text_xl().font_semibold().text_color(color).child(format!("{}{}", if up { "+" } else { "−" }, fmt::size(delta.unsigned_abs()))))
                                        .child(div().text_sm().text_color(theme.muted_foreground).child(format!("с {}", c.previous_time.format("%d.%m.%Y %H:%M")))),
                                )
                                .child(div().text_xs().text_color(theme.muted_foreground).child("Папки, которые изменились сильнее всего. Щелчок по папке — показать её на карте.")),
                        ),
                )
            })
            .child(div().id("growth-scroll").flex_1().min_h_0().overflow_y_scrollbar().child(ui::card(cx).p_1p5().child(list)))
            .into_any_element()
    }
}

/// Right-click menu of a row in the folder list (the node menu of the 1.x tree).
fn row_menu(menu: PopupMenu, danger: Hsla, this: WeakEntity<AnalyzerView>, target: Target, path: PathBuf) -> PopupMenu {
    let is_dir = matches!(target, Target::Dir(_));
    let excluded = crate::data::settings::get().is_excluded(&path.to_string_lossy());
    // the menu acts on the row it was opened on, whatever got selected since
    let act = move |f: fn(&mut AnalyzerView, &mut Window, &mut Context<AnalyzerView>)| {
        let (this, target) = (this.clone(), target.clone());
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            this.update(cx, |v, cx| {
                v.selected = Some(target.clone());
                f(v, window, cx);
            })
            .ok();
        }
    };
    let (p1, p2, p3, p4) = (path.clone(), path.clone(), path.clone(), path.clone());
    menu.item(PopupMenuItem::new("Открыть").icon(Icon::new(Lucide::ExternalLink)).on_click(move |_, _, _| {
        if is_dir {
            shell::open_folder(&p1);
        } else {
            shell::open(&p1.to_string_lossy());
        }
    }))
    .item(PopupMenuItem::new("Показать в Проводнике").icon(Icon::new(Lucide::FolderOpen)).on_click(move |_, _, _| shell::show_in_explorer(&p2)))
    .item(PopupMenuItem::new("Копировать путь").icon(Icon::new(Lucide::Copy)).on_click(move |_, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(p3.to_string_lossy().to_string()))
    }))
    .when(is_dir, |m| {
        m.separator()
            .item(PopupMenuItem::new("Сжать папку…").icon(Icon::new(Lucide::Archive)).on_click(act(|v, window, cx| v.compress_selected(window, cx))))
            .item(PopupMenuItem::new("Перенести на другой диск…").icon(Icon::new(Lucide::FolderInput)).on_click(act(|v, window, cx| v.move_selected(window, cx))))
    })
    .item(
        PopupMenuItem::new(if excluded { "Уже в исключениях" } else { "Никогда не трогать (в исключения)" })
            .icon(Icon::new(Lucide::ShieldCheck))
            .disabled(excluded)
            .on_click(move |_, window, cx| {
                let p = p4.to_string_lossy().to_string();
                crate::data::settings::update(|s| {
                    if !s.exclusions.iter().any(|e| e.eq_ignore_ascii_case(&p)) {
                        s.exclusions.push(p.clone());
                    }
                });
                let name = p4.file_name().unwrap_or_default().to_string_lossy().to_string();
                window.push_notification(Notification::success(format!("«{name}» в исключениях: Простор не будет её чистить и удалять")), cx);
            }),
    )
    .separator()
    .item(PopupMenuItem::new("Переместить в корзину…").icon(Icon::new(Lucide::Trash).text_color(danger)).on_click(act(|v, window, cx| v.delete_selection(window, cx))))
}

fn path_size(v: &AnalyzerView, p: &Path) -> u64 {
    let Some(tree) = v.tree.as_ref() else { return 0 };
    let t = tree.read().unwrap();
    if let Some(nav) = t.find(p) {
        return t.at(&nav).size;
    }
    let (Some(parent), Some(name)) = (p.parent(), p.file_name()) else { return 0 };
    let name = name.to_string_lossy();
    t.find(parent).and_then(|nav| t.at(&nav).files.iter().find(|f| f.name.eq_ignore_ascii_case(&name)).map(|f| f.size)).unwrap_or(0)
}

impl Render for AnalyzerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.narrow = window.viewport_size().width < px(1150.);
        let body = match &self.state {
            ScanState::Scanning { progress, started, target, mft } => {
                let (progress, started, target, mft) = (progress.clone(), *started, target.clone(), *mft);
                self.render_scanning(&progress, started, &target, mft, cx).into_any_element()
            }
            ScanState::Done | ScanState::Idle if self.tree.is_some() => {
                let tree = self.tree.clone().unwrap();
                let toolbar = {
                    let t = tree.read().unwrap();
                    self.render_toolbar(&t, cx).into_any_element()
                };
                let content = match self.tab {
                    Tab::Map => self.render_map(&tree, cx).into_any_element(),
                    Tab::Types => self.render_types(cx).into_any_element(),
                    Tab::Old => self.render_rows(false, cx).into_any_element(),
                    Tab::Downloads => self.render_rows(true, cx).into_any_element(),
                    Tab::Duplicates => self.render_dups(cx).into_any_element(),
                    Tab::Growth => self.render_growth(cx).into_any_element(),
                };
                v_flex().size_full().gap_3().px_6().pt_5().pb_4().child(toolbar).child(self.render_tabs(cx)).child(content).into_any_element()
            }
            _ => self.render_start(cx).into_any_element(),
        };
        div()
            .size_full()
            .key_context("Analyzer")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &GoUp, _, cx| this.go_up(cx)))
            .child(body)
    }
}

// ---------- treemap painting ----------

struct Tile {
    rect: Rect,
    color: Hsla,
    /// File category, or the dominant one of a collapsed folder (None for frames and "the rest").
    cat: Option<u8>,
    label: Option<String>,
    frame: bool,
    hit: usize,
    selected: bool,
}

fn category_color(c: u8) -> Hsla {
    rgb(categories::info(c).color).into()
}

#[allow(clippy::too_many_arguments)]
fn treemap_canvas(
    tree: Tree,
    nav: Vec<usize>,
    hits: Rc<RefCell<Vec<Hit>>>,
    hovered: Option<usize>,
    selected: Option<Target>,
    highlight: Option<u8>,
    background: Hsla,
    foreground: Hsla,
    dark: bool,
) -> Canvas<Vec<Tile>> {
    canvas(
        move |bounds, _window, _cx| {
            let mut out_hits = Vec::new();
            let mut tiles = Vec::new();
            let t = tree.read().unwrap();
            let node = t.at(&nav);
            let root = Rect::new(bounds.origin.x.as_f32(), bounds.origin.y.as_f32(), bounds.size.width.as_f32(), bounds.size.height.as_f32());
            layout_level(node, &nav, root, 0, dark, &mut tiles, &mut out_hits);
            for tile in &mut tiles {
                tile.selected = selected.as_ref().is_some_and(|s| out_hits.get(tile.hit).is_some_and(|h| h.target == *s));
            }
            *hits.borrow_mut() = out_hits;
            tiles
        },
        move |_bounds, tiles, window, cx| {
            let font = window.text_style().font();
            for t in &tiles {
                let r = t.rect.inset(0.5, 0.5, 0.5, 0.5);
                if r.w < 1.0 || r.h < 1.0 {
                    continue;
                }
                let mut color = t.color;
                if hovered == Some(t.hit) {
                    color.l = (color.l + 0.12).min(0.95);
                }
                // a legend pick fades everything of other types into the background
                if !t.frame && highlight.is_some() && t.cat != highlight {
                    color.s *= 0.2;
                    color.a = 0.22;
                }
                let b = bounds(point(px(r.x), px(r.y)), size(px(r.w), px(r.h)));
                let radius = px(if r.w > 8.0 && r.h > 8.0 { 3.0 } else { 0.0 });
                if t.frame {
                    window.paint_quad(fill(b, color).corner_radii(radius));
                } else {
                    // a soft vertical shade gives the flat tiles some volume, like cushion treemaps
                    let top = Hsla { l: (color.l + 0.06).min(0.95), ..color };
                    let bottom = Hsla { l: color.l * 0.84, ..color };
                    window.paint_quad(fill(b, linear_gradient(180., linear_color_stop(top, 0.), linear_color_stop(bottom, 1.))).corner_radii(radius));
                }
                if t.frame {
                    let body = t.rect.inset(2.0, HEADER_H, 2.0, 2.0);
                    if body.w > 0.0 && body.h > 0.0 {
                        window.paint_quad(fill(bounds(point(px(body.x), px(body.y)), size(px(body.w), px(body.h))), background).corner_radii(px(2.)));
                    }
                }
                if let Some(label) = &t.label {
                    let fg: Hsla = if color.l > 0.62 { hsla(0., 0., 0.08, 0.9) } else { hsla(0., 0., 1., 0.95) };
                    let text: SharedString = label.clone().into();
                    let run = TextRun { len: text.len(), font: font.clone(), color: fg, background_color: None, underline: None, strikethrough: None };
                    let line = window.text_system().shape_line(text, px(12.), &[run], None);
                    window.with_content_mask(Some(ContentMask { bounds: b }), |window| {
                        let _ = line.paint(point(px(r.x + 4.0), px(r.y + 2.0)), px(15.), TextAlign::Left, None, window, cx);
                    });
                }
            }
            // the selected tile gets an outline on top of everything
            for t in tiles.iter().filter(|t| t.selected) {
                let r = t.rect;
                window.paint_quad(outline(bounds(point(px(r.x), px(r.y)), size(px(r.w), px(r.h))), foreground, BorderStyle::Solid).corner_radii(px(3.)).border_widths(px(2.)));
            }
        },
    )
}

fn layout_level(node: &DirNode, nav: &[usize], rect: Rect, depth: usize, dark: bool, tiles: &mut Vec<Tile>, hits: &mut Vec<Hit>) {
    enum Item<'a> {
        Dir(usize, &'a DirNode),
        File(&'a scanner::FileEntry),
    }
    let mut items: Vec<(u64, Item)> = Vec::with_capacity(node.dirs.len() + node.files.len());
    items.extend(node.dirs.iter().enumerate().filter(|(_, d)| d.size > 0).map(|(i, d)| (d.size, Item::Dir(i, d))));
    items.extend(node.files.iter().filter(|f| f.size > 0).map(|f| (f.size, Item::File(f))));
    items.sort_unstable_by_key(|i| std::cmp::Reverse(i.0));
    if items.is_empty() {
        return;
    }
    // drop tiles that would be smaller than ~3x3 px and merge them into one
    let area = (rect.w * rect.h) as f64;
    let total: u64 = items.iter().map(|i| i.0).sum();
    let min_size = (total as f64 * 9.0 / area.max(1.0)) as u64;
    let keep = items.iter().take_while(|i| i.0 >= min_size).count().clamp(1, MAX_TILES);
    let rest: u64 = items[keep..].iter().map(|i| i.0).sum();
    let rest_count = items.len() - keep;
    let mut values: Vec<f64> = items[..keep].iter().map(|i| i.0 as f64).collect();
    if rest > 0 {
        values.push(rest as f64);
    }
    let rects = treemap::squarify(&values, rect);

    for (k, r) in rects.iter().enumerate() {
        if r.w < 1.0 || r.h < 1.0 {
            continue;
        }
        let hit_ix = hits.len();
        if k >= keep {
            hits.push(Hit {
                bounds: to_bounds(r),
                target: Target::Rest,
                label: format!("Остальное: {}", fmt::count(rest_count as u64, "элемент", "элемента", "элементов")),
                size: rest,
                category: None,
            });
            tiles.push(Tile { rect: *r, color: hsla(0., 0., if dark { 0.35 } else { 0.62 }, 1.), cat: None, label: None, frame: false, hit: hit_ix, selected: false });
            continue;
        }
        let (size, item) = (items[k].0, &items[k].1);
        let show_label = r.w > 46.0 && r.h > 18.0;
        match item {
            Item::Dir(i, d) => {
                let mut child_nav = nav.to_vec();
                child_nav.push(*i);
                let mut color = category_color(d.dominant);
                if d.is_virtual {
                    color = hsla(0., 0., if dark { 0.30 } else { 0.55 }, 1.);
                }
                let label = format!("{}  {}", d.name, fmt::size(size));
                hits.push(Hit { bounds: to_bounds(r), target: Target::Dir(child_nav.clone()), label: format!("{}\\", d.name), size, category: None });
                let nested = depth < MAX_DEPTH && !d.is_virtual && r.w >= 60.0 && r.h >= 44.0 && r.w * r.h >= 9000.0 && (!d.dirs.is_empty() || d.files.len() > 1);
                if nested {
                    let mut frame = color;
                    frame.l *= if dark { 0.55 } else { 0.7 };
                    tiles.push(Tile { rect: *r, color: frame, cat: None, label: show_label.then_some(label), frame: true, hit: hit_ix, selected: false });
                    layout_level(d, &child_nav, r.inset(2.0, HEADER_H, 2.0, 2.0), depth + 1, dark, tiles, hits);
                } else {
                    color.s *= 0.75;
                    tiles.push(Tile { rect: *r, color, cat: (!d.is_virtual).then_some(d.dominant), label: show_label.then_some(label), frame: false, hit: hit_ix, selected: false });
                }
            }
            Item::File(f) => {
                hits.push(Hit { bounds: to_bounds(r), target: Target::File(nav.to_vec(), f.name.to_string()), label: f.name.to_string(), size, category: Some(f.category) });
                tiles.push(Tile {
                    rect: *r,
                    color: category_color(f.category),
                    cat: Some(f.category),
                    label: show_label.then(|| format!("{}  {}", f.name, fmt::size(size))),
                    frame: false,
                    hit: hit_ix,
                    selected: false,
                });
            }
        }
    }
}

fn to_bounds(r: &Rect) -> Bounds<Pixels> {
    bounds(point(px(r.x), px(r.y)), size(px(r.w), px(r.h)))
}

// ---------- largest files table ----------

#[derive(Default)]
pub struct FilesDelegate {
    pub files: Vec<LargeFile>,
}

const COLS: [(&str, &str, f32); 5] = [("name", "Имя", 300.), ("size", "Размер", 100.), ("kind", "Тип", 170.), ("date", "Изменён", 120.), ("folder", "Папка", 520.)];

impl TableDelegate for FilesDelegate {
    fn columns_count(&self, _: &App) -> usize {
        COLS.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.files.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (key, name, w) = COLS[col_ix];
        let c = Column::new(key, name).width(px(w));
        match col_ix {
            1 => c.text_right().descending(),
            4 => c,
            _ => c.sortable(),
        }
    }

    fn perform_sort(&mut self, col_ix: usize, sort: ColumnSort, _: &mut Window, _: &mut Context<TableState<Self>>) {
        let asc = match sort {
            ColumnSort::Ascending => true,
            ColumnSort::Descending => false,
            ColumnSort::Default => {
                self.files.sort_unstable_by_key(|f| std::cmp::Reverse(f.size));
                return;
            }
        };
        match col_ix {
            0 => self.files.sort_by_cached_key(|f| f.name.to_lowercase()),
            1 => self.files.sort_unstable_by_key(|f| f.size),
            2 => self.files.sort_unstable_by_key(|f| f.category),
            3 => self.files.sort_unstable_by_key(|f| f.modified),
            _ => {}
        }
        if !asc {
            self.files.reverse();
        }
    }

    fn render_td(&mut self, row_ix: usize, col_ix: usize, _: &mut Window, cx: &mut Context<TableState<Self>>) -> impl IntoElement {
        let Some(f) = self.files.get(row_ix) else { return div().into_any_element() };
        match col_ix {
            0 => h_flex()
                .gap_2()
                .items_center()
                .overflow_hidden()
                .child(div().flex_shrink_0().size(px(8.)).rounded_full().bg(rgb(categories::info(f.category).color)))
                .child(div().truncate().child(f.name.clone()))
                .into_any_element(),
            1 => div().w_full().text_right().child(fmt::size(f.size)).into_any_element(),
            2 => div().text_color(cx.theme().muted_foreground).child(categories::info(f.category).title).into_any_element(),
            3 => div().text_color(cx.theme().muted_foreground).child(fmt::filetime_date(f.modified)).into_any_element(),
            _ => div().truncate().text_color(cx.theme().muted_foreground).child(f.folder.display().to_string()).into_any_element(),
        }
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        let Some(f) = self.files.get(row_ix) else { return String::new() };
        match col_ix {
            0 => f.name.clone(),
            1 => fmt::size(f.size),
            2 => categories::info(f.category).title.into(),
            3 => fmt::filetime_date(f.modified),
            _ => f.folder.display().to_string(),
        }
    }
}
