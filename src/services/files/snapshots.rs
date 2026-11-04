//! Folder sizes of every scan, to show what grew since last time (port of Services/ScanHistory.cs,
//! same gzipped JSON files in %LocalAppData%\Prostor\Scans).
//!
//! Fixed compared to the C# version: folders under 1 MB are not stored, so a folder that grew from a
//! few kilobytes was reported as a "new folder". It is now described as "было меньше 1 МБ" unless its
//! parent was stored and the folder was absent there.

use crate::data;
use crate::scanner::DirNode;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::PathBuf;

const MAX_DEPTH: usize = 7;
const MIN_DIR_SIZE: u64 = 1 << 20;
const KEEP_PER_ROOT: usize = 6;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct Snapshot {
    pub root: String,
    pub time: DateTime<Local>,
    pub total: i64,
    /// Relative folder path → size (folders of at least 1 MB, up to 7 levels deep).
    pub dirs: BTreeMap<String, i64>,
}

#[derive(Clone, Debug)]
pub struct Change {
    pub relative: String,
    pub full: PathBuf,
    pub old: u64,
    pub new: u64,
    /// The parent was stored before but this folder was not there at all.
    pub really_new: bool,
}

impl Change {
    pub fn delta(&self) -> i64 {
        self.new as i64 - self.old as i64
    }
    pub fn detail(&self) -> String {
        if self.new == 0 {
            "папки больше нет".into()
        } else if self.old == 0 {
            if self.really_new { "новая папка".into() } else { "было меньше 1 МБ".into() }
        } else {
            format!("{} → {}", crate::fmt::size(self.old), crate::fmt::size(self.new))
        }
    }
}

#[derive(Clone, Debug)]
pub struct Comparison {
    pub previous_time: DateTime<Local>,
    pub previous_total: u64,
    pub current_total: u64,
    pub changes: Vec<Change>,
}

fn folder() -> PathBuf {
    data::data_dir().join("Scans")
}

/// Same key as the C# version (first 8 bytes of SHA-1 of the lower-case root), so old scans are found.
fn key(root: &str) -> String {
    let h = Sha1::digest(root.trim_end_matches('\\').to_lowercase().as_bytes());
    h[..8].iter().map(|b| format!("{b:02X}")).collect()
}

pub fn capture(root: &DirNode, root_path: &str) -> Snapshot {
    let mut dirs = BTreeMap::new();
    fn walk(n: &DirNode, rel: &str, depth: usize, out: &mut BTreeMap<String, i64>) {
        for d in &n.dirs {
            if d.is_virtual || d.size < MIN_DIR_SIZE {
                continue;
            }
            let p = if rel.is_empty() { d.name.to_string() } else { format!("{rel}\\{}", d.name) };
            out.insert(p.clone(), d.size as i64);
            if depth < MAX_DEPTH {
                walk(d, &p, depth + 1, out);
            }
        }
    }
    walk(root, "", 1, &mut dirs);
    Snapshot { root: root_path.trim_end_matches('\\').to_string(), time: Local::now(), total: root.size as i64, dirs }
}

pub fn save(s: &Snapshot) {
    let dir = folder();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let k = key(&s.root);
    let file = dir.join(format!("{k}_{}.json.gz", s.time.format("%Y%m%d_%H%M%S")));
    let Ok(f) = std::fs::File::create(&file) else { return };
    let mut gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
    if serde_json::to_writer(&mut gz, s).is_err() || gz.flush().is_err() {
        return;
    }
    let _ = gz.finish();
    let mut files = list(&k);
    files.sort();
    files.reverse();
    for old in files.into_iter().skip(KEEP_PER_ROOT) {
        let _ = std::fs::remove_file(old);
    }
}

fn list(k: &str) -> Vec<PathBuf> {
    std::fs::read_dir(folder())
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    let n = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                    n.starts_with(&format!("{k}_")) && n.ends_with(".json.gz")
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The newest earlier scan of the same folder that is at least `min_age` old.
pub fn load_previous(root: &str, min_age: chrono::Duration) -> Option<Snapshot> {
    let mut files = list(&key(root));
    files.sort();
    files.reverse();
    for f in files {
        let Ok(file) = std::fs::File::open(&f) else { continue };
        let mut text = String::new();
        if flate2::read::GzDecoder::new(file).read_to_string(&mut text).is_err() {
            continue;
        }
        if let Ok(s) = serde_json::from_str::<Snapshot>(&text)
            && Local::now() - s.time >= min_age {
                return Some(s);
            }
    }
    None
}

/// Folders whose size changed noticeably; a parent is dropped when one child explains most of its change.
pub fn compare(previous: &Snapshot, current: &Snapshot, max: usize) -> Comparison {
    let threshold = (50i64 << 20).max(previous.total.max(current.total) / 1000);
    let lower = |m: &BTreeMap<String, i64>| -> HashMap<String, (String, i64)> { m.iter().map(|(k, v)| (k.to_lowercase(), (k.clone(), *v))).collect() };
    let (prev, cur) = (lower(&previous.dirs), lower(&current.dirs));
    let mut deltas: HashMap<String, i64> = HashMap::new();
    for k in prev.keys().chain(cur.keys()) {
        let a = prev.get(k).map(|x| x.1).unwrap_or(0);
        let b = cur.get(k).map(|x| x.1).unwrap_or(0);
        if (b - a).abs() >= threshold {
            deltas.insert(k.clone(), b - a);
        }
    }
    let root = if current.root.len() <= 2 { format!("{}\\", current.root) } else { current.root.clone() };
    let mut result: Vec<Change> = Vec::new();
    for (path, delta) in &deltas {
        let prefix = format!("{path}\\");
        let explained = deltas.iter().any(|(c, v)| c.starts_with(&prefix) && v.signum() == delta.signum() && v.abs() as f64 >= delta.abs() as f64 * 0.8);
        if explained {
            continue;
        }
        let old = prev.get(path).map(|x| x.1).unwrap_or(0).max(0) as u64;
        let new = cur.get(path).map(|x| x.1).unwrap_or(0).max(0) as u64;
        let display = cur.get(path).or(prev.get(path)).map(|x| x.0.clone()).unwrap_or_else(|| path.clone());
        let parent_stored = match path.rsplit_once('\\') {
            Some((parent, _)) => prev.contains_key(parent),
            None => true, // top level: the root is always "stored"
        };
        result.push(Change { full: PathBuf::from(&root).join(&display), relative: display, old, new, really_new: old == 0 && parent_stored });
    }
    result.sort_by_key(|c| std::cmp::Reverse(c.delta().abs()));
    result.truncate(max);
    Comparison {
        previous_time: previous.time,
        previous_total: previous.total.max(0) as u64,
        current_total: current.total.max(0) as u64,
        changes: result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(dirs: &[(&str, i64)], total: i64) -> Snapshot {
        Snapshot { root: "C:".into(), time: Local::now(), total, dirs: dirs.iter().map(|(k, v)| (k.to_string(), *v)).collect() }
    }

    #[test]
    fn compares_and_explains_by_child() {
        let gb = 1i64 << 30;
        let a = snap(&[("Users", 10 * gb), ("Users\\me", 9 * gb), ("Games", 5 * gb)], 20 * gb);
        let b = snap(&[("Users", 13 * gb), ("Users\\me", 12 * gb), ("Games", 5 * gb), ("Users\\me\\Video", 3 * gb)], 23 * gb);
        let c = compare(&a, &b, 40);
        // "Users" and "Users\me" are explained by "Users\me\Video"
        assert_eq!(c.changes.len(), 1, "{:?}", c.changes);
        assert_eq!(c.changes[0].relative, "Users\\me\\Video");
        assert_eq!(c.changes[0].detail(), "новая папка", "its parent was stored and it was absent");
        let d = snap(&[("Users", 13 * gb), ("Logs", 2 * gb)], 23 * gb);
        let e = compare(&snap(&[("Users", 13 * gb)], 21 * gb), &d, 40);
        assert_eq!(e.changes[0].relative, "Logs");
    }

    #[test]
    fn small_folders_that_grew_are_not_new() {
        let gb = 1i64 << 30;
        let a = snap(&[("Data", 5 * gb)], 5 * gb);
        let b = snap(&[("Data", 5 * gb), ("Data\\Cache\\Deep", gb)], 6 * gb);
        let c = compare(&a, &b, 40);
        let ch = c.changes.iter().find(|x| x.relative == "Data\\Cache\\Deep").unwrap();
        assert_eq!(ch.detail(), "было меньше 1 МБ");
    }

    #[test]
    fn key_matches_csharp() {
        // SHA1("c:") = 3a7c... (first 8 bytes, upper-case hex as Convert.ToHexString)
        assert_eq!(key("C:\\").len(), 16);
        assert_eq!(key("C:"), key("c:\\"));
    }
}
