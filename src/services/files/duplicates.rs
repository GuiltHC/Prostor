//! Identical files: same size → same first and last 64 KB → same SHA-256 → not the same file
//! (hard link) (port of Services/DuplicateFinder.cs).

use crate::data::settings;
use crate::scanner::DirNode;
use crate::sys::shell;
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

const EDGE: u64 = 64 * 1024;

#[derive(Clone, Debug)]
pub struct DupFile {
    pub path: PathBuf,
    pub modified: u64,
}

#[derive(Clone, Debug)]
pub struct Group {
    pub size: u64,
    pub files: Vec<DupFile>,
}

impl Group {
    pub fn wasted(&self) -> u64 {
        self.size * (self.files.len() as u64).saturating_sub(1)
    }
    pub fn name(&self) -> String {
        self.files.first().and_then(|f| f.path.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    }
}

#[derive(Default)]
pub struct Progress {
    pub groups_total: AtomicUsize,
    pub groups_done: AtomicUsize,
    pub hashed: AtomicU64,
    pub candidate_bytes: AtomicU64,
}

fn quick_hash(path: &Path, size: u64, hashed: &AtomicU64) -> Option<[u8; 32]> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = Sha256::new();
    let first = size.min(EDGE) as usize;
    let mut buf = vec![0u8; first];
    f.read_exact(&mut buf).ok()?;
    h.update(&buf);
    let mut n = first as u64;
    if size > 2 * EDGE {
        f.seek(SeekFrom::End(-(EDGE as i64))).ok()?;
        let mut tail = vec![0u8; EDGE as usize];
        f.read_exact(&mut tail).ok()?;
        h.update(&tail);
        n += EDGE;
    } else if size > EDGE {
        let mut rest = Vec::new();
        f.read_to_end(&mut rest).ok()?;
        h.update(&rest);
        n += rest.len() as u64;
    }
    hashed.fetch_add(n, Ordering::Relaxed);
    Some(h.finalize().into())
}

fn full_hash(path: &Path, hashed: &AtomicU64, cancel: &AtomicBool) -> Option<[u8; 32]> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        hashed.fetch_add(n as u64, Ordering::Relaxed);
    }
    Some(h.finalize().into())
}

/// (volume serial, file index): equal for hard links of one file.
fn file_id(path: &Path) -> Option<(u32, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
    let f = std::fs::File::open(path).ok()?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(f.as_raw_handle() as _, &mut info) };
    (ok != 0).then_some((info.dwVolumeSerialNumber, (info.nFileIndexHigh as u64) << 32 | info.nFileIndexLow as u64))
}

fn without_hard_links(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    paths.into_iter().filter(|p| file_id(p).is_none_or(|id| seen.insert(id))).collect()
}

fn modified(p: &Path) -> u64 {
    use std::os::windows::fs::MetadataExt;
    std::fs::metadata(p).map(|m| m.last_write_time()).unwrap_or(0)
}

/// Folders never searched: Windows, program folders, app data, exclusions.
pub fn default_skip(program_folders: &[PathBuf]) -> impl Fn(&str) -> bool + Sync {
    let mut roots: Vec<String> = [
        shell::windows_dir(),
        shell::program_files(),
        shell::program_files_x86(),
        shell::program_data(),
        shell::roaming_app_data(),
        shell::local_app_data(),
        shell::home().join(r"AppData\LocalLow"),
    ]
    .iter()
    .chain(program_folders.iter())
    .map(|p| p.to_string_lossy().to_string())
    .chain(settings::get().exclusions.iter().cloned())
    .filter(|s| !s.trim().is_empty())
    .map(|s| s.trim_end_matches('\\').to_lowercase())
    .collect();
    roots.sort();
    roots.dedup();
    move |path: &str| {
        let name = path.rsplit('\\').next().unwrap_or_default();
        if ["$Recycle.Bin", "System Volume Information", "$WinREAgent", "Recovery", "node_modules", ".git", "WindowsApps", "XboxGames"]
            .iter()
            .any(|n| n.eq_ignore_ascii_case(name))
        {
            return true;
        }
        let l = path.to_lowercase();
        roots.iter().any(|r| l == *r || l.starts_with(&format!("{r}\\")))
    }
}

pub fn find(root: &DirNode, root_path: &Path, min_size: u64, skip: &(dyn Fn(&str) -> bool + Sync), progress: &Progress, cancel: &AtomicBool) -> Vec<Group> {
    // 1. by size
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    let mut stack: Vec<(&DirNode, PathBuf)> = vec![(root, root_path.to_path_buf())];
    while let Some((node, path)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Vec::new();
        }
        for f in &node.files {
            if f.size < min_size {
                break; // sorted by size, largest first
            }
            by_size.entry(f.size).or_default().push(path.join(&*f.name));
        }
        for d in node.dirs.iter().filter(|d| !d.is_virtual) {
            let sub = path.join(&*d.name);
            if !skip(&sub.to_string_lossy()) {
                stack.push((d, sub));
            }
        }
    }
    let candidates: Vec<(u64, Vec<PathBuf>)> = by_size.into_iter().filter(|(_, v)| v.len() > 1).collect();
    progress.groups_total.store(candidates.len(), Ordering::Relaxed);
    progress.candidate_bytes.store(candidates.iter().map(|(s, v)| s * v.len() as u64).sum(), Ordering::Relaxed);

    // 2. edges, 3. full content
    let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().ok();
    let work = || {
        candidates
            .par_iter()
            .flat_map_iter(|(size, paths)| {
                let mut out = Vec::new();
                if !cancel.load(Ordering::Relaxed) {
                    let mut quick: HashMap<[u8; 32], Vec<PathBuf>> = HashMap::new();
                    for p in paths {
                        if let Some(h) = quick_hash(p, *size, &progress.hashed) {
                            quick.entry(h).or_default().push(p.clone());
                        }
                    }
                    for same in quick.into_values().filter(|v| v.len() > 1) {
                        let fulls: Vec<Vec<PathBuf>> = if *size <= 2 * EDGE {
                            vec![same]
                        } else {
                            let mut m: HashMap<[u8; 32], Vec<PathBuf>> = HashMap::new();
                            for p in same {
                                if let Some(h) = full_hash(&p, &progress.hashed, cancel) {
                                    m.entry(h).or_default().push(p);
                                }
                            }
                            m.into_values().filter(|v| v.len() > 1).collect()
                        };
                        for group in fulls {
                            let unique = without_hard_links(group);
                            if unique.len() >= 2 {
                                out.push(Group { size: *size, files: unique.into_iter().map(|p| DupFile { modified: modified(&p), path: p }).collect() });
                            }
                        }
                    }
                }
                progress.groups_done.fetch_add(1, Ordering::Relaxed);
                out
            })
            .collect::<Vec<Group>>()
    };
    let mut groups = match pool {
        Some(p) => p.install(work),
        None => work(),
    };
    groups.sort_by_key(|g| std::cmp::Reverse(g.wasted()));
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_copies_but_not_hard_links() {
        let dir = std::env::temp_dir().join(format!("prostor-dups-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("x")).unwrap();
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.join("a.bin"), &big).unwrap();
        std::fs::write(dir.join("x").join("copy.bin"), &big).unwrap();
        let mut other = big.clone();
        other[150_000] ^= 1; // same size and edges, different middle
        std::fs::write(dir.join("different.bin"), &other).unwrap();
        std::fs::hard_link(dir.join("a.bin"), dir.join("link.bin")).unwrap();

        let tree = crate::scanner::scan(&dir, &crate::scanner::Progress::default()).unwrap();
        let never = AtomicBool::new(false);
        let groups = find(&tree, &dir, 1000, &|_| false, &Progress::default(), &never);
        assert_eq!(groups.len(), 1, "{groups:?}");
        assert_eq!(groups[0].files.len(), 2, "the hard link does not count");
        assert_eq!(groups[0].wasted(), 300_000);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
