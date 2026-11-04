//! Builds a size tree of a folder or drive (port of Services/DiskScanner.cs and Services/MftScanner.cs).
//!
//! Two ways to scan:
//! * walking folders — `std::fs::read_dir` is a thin wrapper over FindFirstFileExW and `DirEntry::metadata`
//!   reuses the find data, so there is no extra system call per file; folders are walked in parallel;
//! * reading the NTFS Master File Table directly (administrator, local NTFS) — the whole volume in seconds,
//!   hard links counted once, files of other users and system folders included.
//!
//! Junctions and symbolic links are never followed; cloud-only (OneDrive) files count as zero bytes.

use crate::categories::{self, COUNT};
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
/// Folders up to this depth scan their children in parallel.
const PARALLEL_DEPTH: usize = 6;

pub struct FileEntry {
    pub name: Box<str>,
    pub size: u64,
    /// FILETIME of the last write.
    pub modified: u64,
    /// FILETIME: latest of last access and last write (when the file was last used).
    pub used: u64,
    /// FILETIME of creation (for downloads: when the file arrived).
    pub created: u64,
    pub category: u8,
}

#[derive(Default)]
pub struct DirNode {
    pub name: Box<str>,
    pub size: u64,
    pub file_count: u64,
    pub dir_count: u64,
    /// Sorted by size, largest first.
    pub dirs: Vec<DirNode>,
    /// Sorted by size, largest first.
    pub files: Vec<FileEntry>,
    /// Category taking the most space inside (colours collapsed folders in the treemap).
    pub dominant: u8,
    pub access_denied: bool,
    /// Virtual node for space the scan could not attribute (system files, no access).
    pub is_virtual: bool,
}

impl DirNode {
    /// Follows `path` (indices into `dirs`) from this node.
    pub fn at(&self, path: &[usize]) -> &DirNode {
        path.iter().fold(self, |n, &i| &n.dirs[i])
    }

    pub fn at_mut(&mut self, path: &[usize]) -> &mut DirNode {
        path.iter().fold(self, |n, &i| &mut n.dirs[i])
    }

    /// Full file-system path of the node reached by `path`.
    pub fn full_path(&self, path: &[usize]) -> PathBuf {
        let mut p = PathBuf::from(&*self.name);
        let mut n = self;
        for &i in path {
            n = &n.dirs[i];
            p.push(&*n.name);
        }
        p
    }

    /// Indices of the folder `target` (a full path) below this root.
    pub fn find(&self, target: &Path) -> Option<Vec<usize>> {
        let root = self.name.trim_end_matches('\\').to_lowercase();
        let t = crate::sys::long_path(target).to_string_lossy().trim_end_matches('\\').to_lowercase();
        if t == root {
            return Some(Vec::new());
        }
        let rest = t.strip_prefix(&format!("{root}\\"))?;
        let mut node = self;
        let mut path = Vec::new();
        for part in rest.split('\\').filter(|p| !p.is_empty()) {
            let i = node.dirs.iter().position(|d| d.name.to_lowercase() == part)?;
            path.push(i);
            node = &node.dirs[i];
        }
        Some(path)
    }

    /// Removes a sub-folder or file from the tree after it was deleted, updating the totals of every parent.
    pub fn remove(&mut self, parent: &[usize], dir: Option<usize>, file: Option<&str>) {
        let (size, files, dirs) = {
            let p = self.at_mut(parent);
            if let Some(i) = dir {
                let d = p.dirs.remove(i);
                (d.size, d.file_count, d.dir_count + 1)
            } else if let Some(name) = file {
                match p.files.iter().position(|f| f.name.eq_ignore_ascii_case(name)) {
                    Some(i) => (p.files.remove(i).size, 1, 0),
                    None => return,
                }
            } else {
                return;
            }
        };
        let mut node: &mut DirNode = self;
        for depth in 0..=parent.len() {
            node.size = node.size.saturating_sub(size);
            node.file_count = node.file_count.saturating_sub(files);
            node.dir_count = node.dir_count.saturating_sub(dirs);
            if depth < parent.len() {
                node = &mut node.dirs[parent[depth]];
            }
        }
    }
}

#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    /// MFT: records read / total.
    pub records: AtomicU64,
    pub records_total: AtomicU64,
    pub cancel: AtomicBool,
    pub current: Mutex<String>,
    mft_failed: AtomicBool,
}

impl Progress {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    /// Reading the MFT failed and the scan fell back to walking folders.
    pub fn set_mft_failed(&self) {
        self.mft_failed.store(true, Ordering::Relaxed);
    }
    pub fn mft_failed(&self) -> bool {
        self.mft_failed.load(Ordering::Relaxed)
    }
    fn set_current(&self, s: &str) {
        if let Ok(mut cur) = self.current.try_lock() {
            cur.clear();
            cur.push_str(s);
        }
    }
}

fn normalize_root(root: &Path) -> String {
    // the MFT and the snapshots know folders by their long names only
    let mut s = crate::sys::long_path(root).to_string_lossy().into_owned();
    if s.len() == 2 && s.ends_with(':') {
        s.push('\\');
    }
    if s.len() > 3 { s.trim_end_matches('\\').to_string() } else { s }
}

/// On a whole drive, shows the space the scan could not see as its own block.
fn add_unknown(node: &mut DirNode) {
    if node.name.len() > 3 {
        return;
    }
    if let Some(space) = crate::sys::drives::space(&node.name) {
        let used = space.total.saturating_sub(space.free);
        let unknown = used.saturating_sub(node.size);
        if unknown > used / 200 {
            node.dirs.push(DirNode {
                name: "Системное и недоступное".into(),
                size: unknown,
                dominant: categories::Category::System as u8,
                is_virtual: true,
                ..Default::default()
            });
            node.size += unknown;
            node.dirs.sort_unstable_by_key(|d| Reverse(d.size));
        }
    }
}

/// Scans by walking folders. Returns `None` when cancelled.
pub fn scan(root: &Path, progress: &Progress) -> Option<DirNode> {
    let root_str = normalize_root(root);
    let (mut node, _) = scan_dir(root_str.trim_end_matches('\\').into(), &PathBuf::from(&root_str), 0, progress);
    if progress.cancelled() {
        return None;
    }
    node.name = root_str.into();
    add_unknown(&mut node);
    Some(node)
}

fn dominant(cats: &[u64; COUNT]) -> u8 {
    let best = (0..COUNT).max_by_key(|&i| (cats[i], i == categories::Category::Other as usize)).unwrap_or(COUNT - 1);
    if cats[best] == 0 { categories::Category::Other as u8 } else { best as u8 }
}

fn scan_dir(name: Box<str>, path: &Path, depth: usize, progress: &Progress) -> (DirNode, [u64; COUNT]) {
    let mut node = DirNode { name, dominant: categories::Category::Other as u8, ..Default::default() };
    let mut cats = [0u64; COUNT];
    if progress.cancelled() {
        return (node, cats);
    }
    let entries = match std::fs::read_dir(path) {
        Ok(e) => e,
        Err(e) => {
            node.access_denied = e.kind() == std::io::ErrorKind::PermissionDenied;
            progress.dirs.fetch_add(1, Ordering::Relaxed);
            return (node, cats);
        }
    };
    let mut subdirs: Vec<(Box<str>, PathBuf)> = Vec::new();
    let mut local_bytes = 0u64;
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if ft.is_dir() {
            subdirs.push((name.as_ref().into(), entry.path()));
            continue;
        }
        if ft.is_symlink() {
            continue; // junctions and symlinks are never followed
        }
        let Ok(md) = entry.metadata() else { continue };
        let size = if md.file_attributes() & FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS != 0 { 0 } else { md.file_size() };
        let category = categories::of(&name) as u8;
        cats[category as usize] += size;
        local_bytes += size;
        let modified = md.last_write_time();
        node.files.push(FileEntry {
            name: name.as_ref().into(),
            size,
            modified,
            used: modified.max(md.last_access_time()),
            created: md.creation_time(),
            category,
        });
    }
    progress.files.fetch_add(node.files.len() as u64, Ordering::Relaxed);
    progress.bytes.fetch_add(local_bytes, Ordering::Relaxed);
    progress.dirs.fetch_add(1, Ordering::Relaxed);
    progress.set_current(&path.to_string_lossy());
    node.files.sort_unstable_by_key(|f| Reverse(f.size));

    let children: Vec<(DirNode, [u64; COUNT])> = if depth < PARALLEL_DEPTH && subdirs.len() > 1 {
        subdirs.into_par_iter().map(|(n, p)| scan_dir(n, &p, depth + 1, progress)).collect()
    } else {
        subdirs.into_iter().map(|(n, p)| scan_dir(n, &p, depth + 1, progress)).collect()
    };
    let (mut size, mut fc, mut dc) = (local_bytes, node.files.len() as u64, 0u64);
    node.dirs.reserve_exact(children.len());
    for (child, child_cats) in children {
        size += child.size;
        fc += child.file_count;
        dc += child.dir_count + 1;
        for (a, b) in cats.iter_mut().zip(child_cats) {
            *a += b;
        }
        node.dirs.push(child);
    }
    node.dirs.sort_unstable_by_key(|d| Reverse(d.size));
    node.size = size;
    node.file_count = fc;
    node.dir_count = dc;
    node.dominant = dominant(&cats);
    (node, cats)
}

// ------------------------------------------------------------------ MFT

/// MFT reading is possible: administrator, local fixed NTFS volume.
pub fn can_use_mft(path: &Path) -> bool {
    if !crate::sys::shell::is_admin() {
        return false;
    }
    let s = path.to_string_lossy();
    let Some(root) = s.get(..3).filter(|r| r.as_bytes()[1] == b':') else { return false };
    crate::sys::drives::list().iter().any(|d| !d.removable && d.is_ntfs() && d.root.eq_ignore_ascii_case(root))
}

const ROOT_FRN: usize = 5;
// per-record flags
const F_USED: u8 = 1;
const F_DIR: u8 = 2;
const F_NAME: u8 = 4;
const F_LINK: u8 = 8;
const F_CLOUD: u8 = 16;

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn i64le(b: &[u8], o: usize) -> i64 {
    i64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Restores the last two bytes of every sector from the update sequence array.
pub fn apply_fixups(rec: &mut [u8]) -> bool {
    if rec.len() < 48 || &rec[0..4] != b"FILE" {
        return false;
    }
    let usa_off = u16le(rec, 4) as usize;
    let usa_count = u16le(rec, 6) as usize;
    if usa_count < 2 || usa_off + usa_count * 2 > rec.len() {
        return false;
    }
    let stride = rec.len() / (usa_count - 1);
    let usn = u16le(rec, usa_off);
    for i in 1..usa_count {
        let end = i * stride - 2;
        if end + 2 > rec.len() || u16le(rec, end) != usn {
            return false; // torn write
        }
        rec[end] = rec[usa_off + i * 2];
        rec[end + 1] = rec[usa_off + i * 2 + 1];
    }
    true
}

/// Data runs → (LCN, clusters); sparse runs are skipped.
pub fn parse_runs(data: &[u8]) -> Vec<(i64, i64)> {
    let mut runs = Vec::new();
    let (mut lcn, mut i) = (0i64, 0usize);
    while i < data.len() && data[i] != 0 {
        let header = data[i];
        i += 1;
        let (len_size, off_size) = ((header & 0x0F) as usize, (header >> 4) as usize);
        if len_size == 0 || i + len_size + off_size > data.len() {
            break;
        }
        let mut length = 0i64;
        for k in 0..len_size {
            length |= (data[i + k] as i64) << (8 * k);
        }
        i += len_size;
        if off_size == 0 {
            continue; // sparse
        }
        let mut delta = 0i64;
        for k in 0..off_size {
            delta |= (data[i + k] as i64) << (8 * k);
        }
        if data[i + off_size - 1] & 0x80 != 0 {
            delta |= -1i64 << (8 * off_size);
        }
        i += off_size;
        lcn += delta;
        runs.push((lcn, length));
    }
    runs
}

struct Mft {
    parent: Vec<u32>,
    name: Vec<Option<Box<str>>>,
    size: Vec<u64>,
    write: Vec<u64>,
    used: Vec<u64>,
    created: Vec<u64>,
    flags: Vec<u8>,
}

impl Mft {
    fn new(n: usize) -> Self {
        Self {
            parent: vec![0; n],
            name: vec![None; n],
            size: vec![0; n],
            write: vec![0; n],
            used: vec![0; n],
            created: vec![0; n],
            flags: vec![0; n],
        }
    }

    fn parse(&mut self, rec: &mut [u8], frn: usize) {
        if !apply_fixups(rec) {
            return;
        }
        let flags = u16le(rec, 0x16);
        if flags & 1 == 0 {
            return; // not in use
        }
        let base = (i64le(rec, 0x20) & 0xFFFF_FFFF_FFFF) as usize;
        let owner = if base != 0 { base } else { frn };
        if owner >= self.flags.len() {
            return;
        }
        if base == 0 {
            self.flags[owner] |= F_USED;
            if flags & 2 != 0 {
                self.flags[owner] |= F_DIR;
            }
        }
        let mut off = u16le(rec, 0x14) as usize;
        while off + 16 <= rec.len() {
            let ty = u32le(rec, off);
            if ty == 0xFFFF_FFFF {
                break;
            }
            let len = u32le(rec, off + 4) as usize;
            if len == 0 || off + len > rec.len() {
                break;
            }
            let non_resident = rec[off + 8] != 0;
            let name_len = rec[off + 9];
            match ty {
                0x10 if !non_resident => {
                    // $STANDARD_INFORMATION
                    let v = u16le(rec, off + 0x14) as usize + off;
                    if v + 0x24 <= rec.len() {
                        let created = i64le(rec, v) as u64;
                        let modified = i64le(rec, v + 8) as u64;
                        let accessed = i64le(rec, v + 24) as u64;
                        let attrs = u32le(rec, v + 32);
                        self.created[owner] = created;
                        self.write[owner] = modified;
                        self.used[owner] = modified.max(accessed);
                        if attrs & FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS != 0 {
                            self.flags[owner] |= F_CLOUD;
                        }
                    }
                }
                0x30 if !non_resident => {
                    // $FILE_NAME
                    let v = u16le(rec, off + 0x14) as usize + off;
                    if v + 0x42 <= rec.len() && rec[v + 0x41] != 2 && self.flags[owner] & F_NAME == 0 {
                        let chars = rec[v + 0x40] as usize;
                        if v + 0x42 + chars * 2 <= rec.len() {
                            let parent = (i64le(rec, v) & 0xFFFF_FFFF_FFFF) as u32;
                            let units: Vec<u16> = rec[v + 0x42..v + 0x42 + chars * 2].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                            self.parent[owner] = parent;
                            self.name[owner] = Some(String::from_utf16_lossy(&units).into());
                            self.flags[owner] |= F_NAME; // the first name of a hard-linked file wins
                        }
                    }
                }
                0x80 if name_len == 0 => {
                    // unnamed $DATA
                    if non_resident {
                        if i64le(rec, off + 0x10) == 0 {
                            self.size[owner] = i64le(rec, off + 0x30) as u64;
                        }
                    } else {
                        self.size[owner] = u32le(rec, off + 0x10) as u64;
                    }
                }
                0xC0 if !non_resident => {
                    // $REPARSE_POINT
                    let v = u16le(rec, off + 0x14) as usize + off;
                    if v + 4 <= rec.len() {
                        let tag = u32le(rec, v);
                        if tag == 0xA000_0003 || tag == 0xA000_000C {
                            self.flags[owner] |= F_LINK;
                        }
                    }
                }
                _ => {}
            }
            off += len;
        }
    }
}

/// Reads the whole Master File Table. Returns Err with a message, or Ok(None) when cancelled.
pub fn scan_mft(path: &Path, progress: &Progress) -> Result<Option<DirNode>, String> {
    use std::os::windows::fs::{FileExt, OpenOptionsExt};
    let requested = normalize_root(path);
    let root = requested.get(..3).ok_or("Неверный путь")?.to_string();
    let volume = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(7)
        .open(format!(r"\\.\{}", root.trim_end_matches('\\')))
        .map_err(|e| format!("Нет доступа к тому {root}: {e}"))?;
    let read_exact = |buf: &mut [u8], offset: u64| -> Result<(), String> {
        let mut done = 0;
        while done < buf.len() {
            let n = volume.seek_read(&mut buf[done..], offset + done as u64).map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("Неожиданный конец тома".into());
            }
            done += n;
        }
        Ok(())
    };

    // boot sector
    let mut boot = vec![0u8; 4096];
    read_exact(&mut boot, 0)?;
    if &boot[3..7] != b"NTFS" {
        return Err("Это не NTFS".into());
    }
    let bytes_per_sector = u16le(&boot, 0x0B) as u64;
    let spc_raw = boot[0x0D] as u64;
    let sectors_per_cluster = if spc_raw <= 0x80 { spc_raw } else { 1 << (256 - spc_raw) };
    let cluster = bytes_per_sector * sectors_per_cluster;
    let mft_lcn = i64le(&boot, 0x30) as u64;
    let cpr = boot[0x40] as i8;
    let record_size = if cpr > 0 { cpr as u64 * cluster } else { 1u64 << (-(cpr as i32)) } as usize;

    // $MFT record 0 → where the MFT lives
    let mut rec0 = vec![0u8; record_size.div_ceil(4096) * 4096];
    read_exact(&mut rec0, mft_lcn * cluster)?;
    let rec0 = &mut rec0[..record_size];
    if !apply_fixups(rec0) {
        return Err("Повреждена запись $MFT".into());
    }
    let (runs, mft_size) = {
        let mut off = u16le(rec0, 0x14) as usize;
        let mut found = (Vec::new(), 0u64);
        while off + 16 <= rec0.len() {
            let ty = u32le(rec0, off);
            if ty == 0xFFFF_FFFF {
                break;
            }
            let len = u32le(rec0, off + 4) as usize;
            if len == 0 || off + len > rec0.len() {
                break;
            }
            if ty == 0x80 && rec0[off + 8] == 1 && rec0[off + 9] == 0 {
                let size = i64le(rec0, off + 0x30) as u64;
                let run_off = u16le(rec0, off + 0x20) as usize;
                found = (parse_runs(&rec0[off + run_off..off + len]), size);
                break;
            }
            off += len;
        }
        found
    };
    if runs.is_empty() || mft_size == 0 {
        return Err("Не найдено расположение MFT".into());
    }
    let count = (mft_size / record_size as u64) as usize;
    progress.records_total.store(count as u64, Ordering::Relaxed);
    let mut mft = Mft::new(count);

    const CHUNK: usize = 4 << 20;
    let mut buf = vec![0u8; CHUNK];
    let mut frn = 0usize;
    for (lcn, clusters) in runs {
        let mut offset = lcn as u64 * cluster;
        let mut remaining = clusters as u64 * cluster;
        while remaining > 0 && frn < count {
            if progress.cancelled() {
                return Ok(None);
            }
            let len = (CHUNK as u64).min(remaining) as usize;
            let len = len.div_ceil(bytes_per_sector as usize) * bytes_per_sector as usize;
            read_exact(&mut buf[..len], offset)?;
            let mut pos = 0;
            while pos + record_size <= len && frn < count {
                mft.parse(&mut buf[pos..pos + record_size], frn);
                pos += record_size;
                frn += 1;
            }
            progress.records.store(frn as u64, Ordering::Relaxed);
            offset += len as u64;
            remaining = remaining.saturating_sub(len as u64);
        }
    }
    progress.set_current("Строим дерево папок…");
    let mut tree = build_tree(&mft, &root, progress);
    if progress.cancelled() {
        return Ok(None);
    }
    // a sub-folder was requested: walk down to it
    let rel = requested[root.len().min(requested.len())..].trim_matches('\\').to_string();
    if rel.is_empty() {
        add_unknown(&mut tree);
        return Ok(Some(tree));
    }
    let Some(path) = tree.find(Path::new(&requested)) else { return Err(format!("Папка не найдена: {requested}")) };
    let mut node = std::mem::take(tree.at_mut(&path));
    node.name = requested.into();
    Ok(Some(node))
}

fn build_tree(m: &Mft, root: &str, progress: &Progress) -> DirNode {
    let n = m.flags.len();
    // children as linked lists: head[parent] → first child, next[child] → sibling
    let mut head_dir = vec![u32::MAX; n];
    let mut head_file = vec![u32::MAX; n];
    let mut next = vec![u32::MAX; n];
    #[allow(clippy::needless_range_loop)] // i indexes several parallel arrays
    for i in 0..n {
        if m.flags[i] & (F_USED | F_NAME) != (F_USED | F_NAME) || i == ROOT_FRN {
            continue;
        }
        let p = m.parent[i] as usize;
        if p >= n || p == i {
            continue;
        }
        if m.flags[i] & F_DIR != 0 {
            next[i] = head_dir[p];
            head_dir[p] = i as u32;
        } else {
            next[i] = head_file[p];
            head_file[p] = i as u32;
        }
    }
    let mut root_node = DirNode { name: root.into(), ..Default::default() };
    build(m, &mut root_node, ROOT_FRN, &head_dir, &head_file, &next, progress, 0);
    root_node
}

#[allow(clippy::too_many_arguments)]
fn build(m: &Mft, node: &mut DirNode, frn: usize, head_dir: &[u32], head_file: &[u32], next: &[u32], progress: &Progress, depth: usize) -> [u64; COUNT] {
    let mut cats = [0u64; COUNT];
    if progress.cancelled() || depth > 512 {
        return cats;
    }
    let (mut size, mut files, mut dirs) = (0u64, 0u64, 0u64);
    let mut f = head_file[frn];
    while f != u32::MAX {
        let fi = f as usize;
        f = next[fi];
        let Some(name) = m.name[fi].as_ref() else { continue };
        if frn == ROOT_FRN && name.starts_with('$') && fi < 24 && &**name != "$MFT" && &**name != "$LogFile" {
            continue; // tiny metadata files
        }
        let s = if m.flags[fi] & F_CLOUD != 0 { 0 } else { m.size[fi] };
        let cat = categories::of(name) as u8;
        node.files.push(FileEntry { name: name.clone(), size: s, modified: m.write[fi], used: m.used[fi], created: m.created[fi], category: cat });
        cats[cat as usize] += s;
        size += s;
    }
    files += node.files.len() as u64;
    progress.files.fetch_add(node.files.len() as u64, Ordering::Relaxed);
    progress.bytes.fetch_add(size, Ordering::Relaxed);
    node.files.sort_unstable_by_key(|f| Reverse(f.size));

    let mut d = head_dir[frn];
    while d != u32::MAX {
        let di = d as usize;
        d = next[di];
        if m.flags[di] & F_LINK != 0 {
            continue; // junctions and symlinks are not followed
        }
        let Some(name) = m.name[di].as_ref() else { continue };
        if frn == ROOT_FRN && &**name == "$Extend" {
            continue;
        }
        let mut child = DirNode { name: name.clone(), dominant: categories::Category::Other as u8, ..Default::default() };
        let child_cats = build(m, &mut child, di, head_dir, head_file, next, progress, depth + 1);
        size += child.size;
        files += child.file_count;
        dirs += child.dir_count + 1;
        for (a, b) in cats.iter_mut().zip(child_cats) {
            *a += b;
        }
        node.dirs.push(child);
    }
    node.dirs.sort_unstable_by_key(|d| Reverse(d.size));
    node.size = size;
    node.file_count = files;
    node.dir_count = dirs;
    node.dominant = dominant(&cats);
    cats
}

// ------------------------------------------------------------------ statistics

#[derive(Clone)]
pub struct LargeFile {
    pub name: String,
    pub folder: PathBuf,
    pub size: u64,
    pub modified: u64,
    pub used: u64,
    pub created: u64,
    pub category: u8,
}

impl LargeFile {
    pub fn full_path(&self) -> PathBuf {
        self.folder.join(&self.name)
    }
}

pub struct CategoryStat {
    pub category: u8,
    pub size: u64,
    pub count: u64,
}

pub struct ExtStat {
    pub ext: String,
    pub size: u64,
    pub count: u64,
    pub category: u8,
}

#[derive(Default)]
pub struct Stats {
    pub largest: Vec<LargeFile>,
    /// Big files not opened or changed for more than a year (outside Windows and program folders).
    pub old_files: Vec<LargeFile>,
    pub old_files_size: u64,
    /// Big files lying in Downloads for more than three months.
    pub old_downloads: Vec<LargeFile>,
    pub old_downloads_size: u64,
    pub categories: Vec<CategoryStat>,
    pub extensions: Vec<ExtStat>,
}

struct TopN {
    heap: BinaryHeap<Reverse<(u64, usize)>>,
    store: Vec<Option<LargeFile>>,
    top: usize,
}

impl TopN {
    fn new(top: usize) -> Self {
        Self { heap: BinaryHeap::with_capacity(top + 1), store: Vec::new(), top }
    }
    fn push(&mut self, size: u64, make: impl FnOnce() -> LargeFile) {
        let min = self.heap.peek().map(|r| r.0.0);
        if self.heap.len() < self.top || min.is_some_and(|m| size > m) {
            let seq = self.store.len();
            self.store.push(Some(make()));
            self.heap.push(Reverse((size, seq)));
            if self.heap.len() > self.top
                && let Some(Reverse((_, s))) = self.heap.pop() {
                    self.store[s] = None;
                }
        }
    }
    fn into_sorted(mut self) -> Vec<LargeFile> {
        let mut v: Vec<LargeFile> = self.heap.into_vec().into_iter().filter_map(|Reverse((_, s))| self.store[s].take()).collect();
        v.sort_unstable_by_key(|f| Reverse(f.size));
        v
    }
}

fn filetime_now() -> u64 {
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    (unix + 11_644_473_600) * 10_000_000
}

/// Statistics of `node` (whose full path is `node_path`).
pub fn stats(node: &DirNode, node_path: &Path, top: usize) -> Stats {
    use crate::sys::shell;
    const DAY: u64 = 86_400 * 10_000_000;
    const MIN_OLD: u64 = 10 << 20;
    let now = filetime_now();
    let (year_ago, quarter_ago) = (now - 365 * DAY, now - 90 * DAY);
    let downloads = shell::downloads().to_string_lossy().to_lowercase();
    let sys_root = crate::sys::drives::system_root();
    // system and program folders: their old files are still needed
    let system_dirs: Vec<String> = [
        shell::windows_dir(),
        shell::program_files(),
        shell::program_files_x86(),
        shell::program_data(),
        PathBuf::from(&sys_root).join("System Volume Information"),
        PathBuf::from(&sys_root).join("$Recycle.Bin"),
        PathBuf::from(&sys_root).join("Recovery"),
    ]
    .iter()
    .map(|p| p.to_string_lossy().to_lowercase())
    .collect();

    let mut largest = TopN::new(top);
    let mut old = TopN::new(top);
    let mut dl = TopN::new(top);
    let (mut old_size, mut dl_size) = (0u64, 0u64);
    let mut cat_size = [0u64; COUNT];
    let mut cat_count = [0u64; COUNT];
    let mut exts: HashMap<String, (u64, u64)> = HashMap::new();

    let root_lower = node_path.to_string_lossy().to_lowercase();
    let is_dl = |p: &str| crate::sys::path_within(p, &downloads);
    let is_sys = |p: &str| system_dirs.iter().any(|s| crate::sys::path_within(p, s));
    let mut stack: Vec<(&DirNode, PathBuf, bool, bool)> = vec![(node, node_path.to_path_buf(), is_dl(&root_lower), is_sys(&root_lower))];
    while let Some((dir, path, in_dl, sys)) = stack.pop() {
        let is_root = path.to_string_lossy().len() <= 3;
        for f in &dir.files {
            cat_size[f.category as usize] += f.size;
            cat_count[f.category as usize] += 1;
            if let Some(dot) = f.name.rfind('.').filter(|&d| d > 0 && d + 1 < f.name.len() && f.name.len() - d <= 16) {
                let e = exts.entry(f.name[dot + 1..].to_lowercase()).or_default();
                e.0 += f.size;
                e.1 += 1;
            }
            let make = || LargeFile {
                name: f.name.to_string(),
                folder: path.clone(),
                size: f.size,
                modified: f.modified,
                used: f.used,
                created: f.created,
                category: f.category,
            };
            largest.push(f.size, make);
            let special_root_file = is_root && (f.name.starts_with('$') || f.name.to_lowercase().ends_with(".sys"));
            if f.size >= MIN_OLD && !sys && !special_root_file {
                let used = if f.used > 0 { f.used } else { f.modified };
                if used > 0 && used < year_ago {
                    old.push(f.size, make);
                    old_size += f.size;
                }
                let arrived = if f.created > 0 { f.created.min(f.modified.max(1)) } else { f.modified };
                if in_dl && arrived > 0 && arrived < quarter_ago {
                    dl.push(f.size, make);
                    dl_size += f.size;
                }
            }
        }
        for d in dir.dirs.iter().filter(|d| !d.is_virtual) {
            let p = path.join(&*d.name);
            let l = p.to_string_lossy().to_lowercase();
            stack.push((d, p, in_dl || l == downloads, sys || system_dirs.contains(&l)));
        }
    }

    let mut categories: Vec<CategoryStat> =
        (0..COUNT).filter(|&i| cat_count[i] > 0).map(|i| CategoryStat { category: i as u8, size: cat_size[i], count: cat_count[i] }).collect();
    categories.sort_unstable_by_key(|c| Reverse(c.size));
    let mut ext_list: Vec<ExtStat> =
        exts.into_iter().map(|(e, (s, c))| ExtStat { category: categories::of(&format!("x.{e}")) as u8, ext: format!(".{e}"), size: s, count: c }).collect();
    ext_list.sort_unstable_by_key(|e| Reverse(e.size));
    ext_list.truncate(40);
    Stats {
        largest: largest.into_sorted(),
        old_files: old.into_sorted(),
        old_files_size: old_size,
        old_downloads: dl.into_sorted(),
        old_downloads_size: dl_size,
        categories,
        extensions: ext_list,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn scans_a_temp_tree() {
        let root = std::env::temp_dir().join(format!("prostor-scan-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("a").join("b")).unwrap();
        fs::write(root.join("big.mp4"), vec![0u8; 5000]).unwrap();
        fs::write(root.join("a").join("doc.pdf"), vec![0u8; 3000]).unwrap();
        fs::write(root.join("a").join("b").join("x.txt"), vec![0u8; 100]).unwrap();

        let progress = Progress::default();
        let mut tree = scan(&root, &progress).unwrap();
        assert_eq!(tree.size, 8100);
        assert_eq!(tree.file_count, 3);
        assert_eq!(tree.dir_count, 2);
        assert_eq!(&*tree.dirs[0].name, "a");
        assert_eq!(tree.dirs[0].size, 3100);
        assert_eq!(tree.dominant, categories::Category::Video as u8);
        assert_eq!(tree.dirs[0].dominant, categories::Category::Documents as u8);
        assert!(tree.files[0].created > 0 && tree.files[0].used >= tree.files[0].modified);

        // the tree knows the folder by its long name even when asked with an 8.3 one (as on CI runners)
        let short = root.clone();
        let root = crate::sys::long_path(&root);
        let st = stats(&tree, &root, 2);
        assert_eq!(st.largest.len(), 2);
        assert_eq!(st.largest[0].name, "big.mp4");
        assert_eq!(st.largest[1].full_path(), root.join("a").join("doc.pdf"));
        assert_eq!(st.categories[0].category, categories::Category::Video as u8);
        assert_eq!(st.extensions[0].ext, ".mp4");

        assert_eq!(tree.full_path(&[0, 0]), root.join("a").join("b"));
        assert_eq!(tree.find(&root.join("A").join("b")), Some(vec![0, 0]));
        assert_eq!(tree.find(&short.join("a").join("b")), Some(vec![0, 0]));
        tree.remove(&[0], Some(0), None);
        assert_eq!(tree.size, 8000);
        assert_eq!(tree.dirs[0].size, 3000);
        assert_eq!(tree.dir_count, 1);
        tree.remove(&[], None, Some("BIG.mp4"));
        assert_eq!(tree.size, 3000);
        assert_eq!(tree.file_count, 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn data_runs() {
        // 0x21: 1-byte length, 2-byte offset; second run goes back (negative offset)
        let runs = parse_runs(&[0x21, 0x10, 0x00, 0x01, 0x21, 0x08, 0x00, 0xFF, 0x00]);
        assert_eq!(runs, vec![(0x100, 0x10), (0x100 - 0x100, 8)]);
    }

    #[test]
    fn mft_matches_folder_walk_on_the_system_drive() {
        if !crate::sys::shell::is_admin() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("prostor-mft-{}", std::process::id()));
        if !can_use_mft(&dir) {
            return;
        }
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("one.bin"), vec![1u8; 70_000]).unwrap();
        fs::write(dir.join("sub").join("two.txt"), b"hello").unwrap();
        let p = Progress::default();
        let tree = scan_mft(&dir, &p).unwrap().unwrap();
        assert_eq!(tree.size, 70_005);
        assert_eq!(tree.file_count, 2);
        fs::remove_dir_all(&dir).unwrap();
    }
}
