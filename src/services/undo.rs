//! Rolls back journal entries (port of Services/UndoService.cs).

use super::{deep, files, startup, tasks};
use super::programs::uninstall;
use crate::data::journal::{Entry, UndoKind};
use crate::sys::recycle;
use std::path::Path;

/// Ok, or a message for the user. `status` reports the item being processed.
pub fn undo(e: &Entry, status: impl Fn(&str)) -> Result<(), String> {
    match e.undo {
        UndoKind::RegImport => uninstall::import_reg(&e.undo_data),
        UndoKind::RecycleRestore => {
            let mut ok = 0;
            for p in &e.undo_data {
                status(p);
                if recycle::restore(Path::new(p)) {
                    ok += 1;
                }
            }
            match ok {
                0 => Err("В корзине не нашлось этих объектов — возможно, её уже очистили.".into()),
                n if n == e.undo_data.len() => Ok(()),
                n => Err(format!("Возвращено {n} из {} — остальных уже нет в корзине.", e.undo_data.len())),
            }
        }
        UndoKind::TaskRestore => {
            for pair in &e.undo_data {
                if let Some((path, file)) = pair.split_once('|') {
                    status(path);
                    let xml = std::fs::read(file).map_err(|e| e.to_string())?;
                    // exports are UTF-16 (from schtasks / PowerShell files written by C#) or UTF-8
                    let text = if xml.starts_with(&[0xFF, 0xFE]) {
                        String::from_utf16_lossy(&xml[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
                    } else {
                        String::from_utf8_lossy(&xml).trim_start_matches('\u{feff}').to_string()
                    };
                    tasks::register(path, &text)?;
                }
            }
            Ok(())
        }
        UndoKind::Decompress => {
            for f in &e.undo_data {
                status(f);
                files::compression::decompress(Path::new(f), &|_| {}, &std::sync::atomic::AtomicBool::new(false))?;
            }
            Ok(())
        }
        UndoKind::MoveBack => {
            if e.undo_data.len() < 2 {
                return Err("Нет данных для возврата.".into());
            }
            files::mover::move_back(Path::new(&e.undo_data[0]), Path::new(&e.undo_data[1]), &|_| {}, &std::sync::atomic::AtomicBool::new(false))
        }
        UndoKind::Hibernation => deep::enable_hibernation(),
        UndoKind::DelayedStart => {
            if e.undo_data.len() < 2 {
                return Err("Нет данных для возврата.".into());
            }
            startup::undo_delayed(&e.undo_data[0], &e.undo_data[1])
        }
        UndoKind::None => Err("Это действие нельзя отменить.".into()),
    }
}

/// What the confirmation explains for each kind of undo.
pub fn explain(kind: UndoKind) -> &'static str {
    match kind {
        UndoKind::RegImport => "Запись программы вернётся в реестр из резервной копии.",
        UndoKind::RecycleRestore => "Файлы и папки вернутся из корзины на прежние места.",
        UndoKind::Decompress => "Файлы будут распакованы и снова займут исходный объём.",
        UndoKind::MoveBack => "Папка будет скопирована обратно, ссылка на новое место удалится.",
        UndoKind::Hibernation => "Файл гибернации снова займёт место на системном диске.",
        UndoKind::DelayedStart => "Программа снова будет запускаться сразу при входе.",
        UndoKind::TaskRestore => "Задача планировщика будет восстановлена из резервной копии.",
        UndoKind::None => "",
    }
}
