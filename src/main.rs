// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod categories;
mod cli;
mod data;
mod fmt;
mod notice;
mod pages;
mod scanner;
mod services;
mod sys;
mod treemap;
mod ui;

use app::{Page, ProstorApp};
use data::settings;
use gpui_kit::component::TitleBar;
use gpui_kit::*;

fn main() {
    let args = cli::parse(std::env::args().skip(1));

    if args.auto_clean {
        // scheduled run: clean, show a short notice, exit
        let freed = services::autoclean::run();
        services::history::record();
        let text = if freed > 0 { format!("Освобождено {}", fmt::size(freed)) } else { "Мусора не нашлось — всё чисто".into() };
        notice::show("Автоочистка Простора", &text);
        return;
    }

    // debug builds: the corner notice with sample text, to check how it looks
    #[cfg(debug_assertions)]
    if std::env::args().any(|a| a == "--demo-notice") {
        notice::show("Автоочистка Простора", "Освобождено 1,2 ГБ");
        return;
    }

    // one window per session: hand the arguments to the running copy
    let first = sys::instance::acquire();
    if !first && !args.new_window && sys::instance::send(&args.to_vec()) {
        return;
    }

    services::history::record_async();

    gpui_kit::application().with_assets(crate::ui::icons::AppAssets).run(move |cx: &mut App| {
        gpui_kit::init(cx);
        app::bind_keys(cx);
        pages::analyzer::bind_keys(cx);
        services::updater::cleanup_after_update();
        let theme = args.theme.clone().unwrap_or_else(|| settings::get().theme.clone());
        ui::theme::set_mode(&theme, None, cx);

        // the app lives exactly as long as its window
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        // PROSTOR_DISPLAY=2 opens the window on the second monitor (handy while developing)
        let display = std::env::var("PROSTOR_DISPLAY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .and_then(|n| {
                let primary = cx.primary_display().map(|d| d.id());
                let mut list: Vec<_> = cx.displays().iter().map(|d| d.id()).collect();
                // primary first, then the others in system order
                list.sort_by_key(|id| Some(*id) != primary);
                list.get(n.checked_sub(1)?).copied()
            });
        let saved = settings::get().window.clone().filter(|w| w.width >= 640.0 && w.height >= 480.0 && display.is_none());
        let window_bounds = match &saved {
            Some(w) => {
                let b = Bounds { origin: point(px(w.left as f32), px(w.top as f32)), size: size(px(w.width as f32), px(w.height as f32)) };
                if w.maximized { WindowBounds::Maximized(b) } else { WindowBounds::Windowed(b) }
            }
            None => {
                // PROSTOR_SIZE=1600x1200 sets the size of a test window (with PROSTOR_DISPLAY)
                let (w, h) = std::env::var("PROSTOR_SIZE")
                    .ok()
                    .and_then(|v| v.split_once('x').and_then(|(w, h)| Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))))
                    .unwrap_or((1280., 840.));
                WindowBounds::Windowed(Bounds::centered(display, size(px(w), px(h)), cx))
            }
        };
        let options = WindowOptions {
            display_id: display,
            window_bounds: Some(window_bounds),
            window_min_size: Some(size(px(960.), px(640.))),
            ..TitleBar::window_options()
        };
        let start_page = args.page.as_deref().and_then(Page::from_name);
        let scan = args.scan.clone();
        let (handle, app) = gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| {
                let mut app = ProstorApp::new(start_page, window, cx);
                if let Some(path) = scan {
                    app.analyze(path.into(), cx);
                }
                app
            })
        })
        .expect("не удалось открыть окно");
        cx.activate(true);
        cx.update_window(handle, |_, window, _| ui::theme::restore_if_minimized(window)).ok();

        // later launches: switch to the page or folder they asked for
        if first {
            let (tx, mut rx) = futures::channel::mpsc::unbounded::<Vec<String>>();
            sys::instance::listen(move |a| {
                let _ = tx.unbounded_send(a);
            });
            cx.spawn(async move |cx| {
                while let Some(raw) = futures::StreamExt::next(&mut rx).await {
                    let parsed = cli::parse(raw);
                    let app = app.clone();
                    cx.update_window(handle, |_, window, cx| {
                        app.update(cx, |a, cx| a.handle_args(&parsed, window, cx));
                        window.activate_window();
                    })
                    .ok();
                }
            })
            .detach();
        }
    });
}
