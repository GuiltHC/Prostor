//! Process helpers for uninstalling: follow an uninstaller with the processes it spawns (NSIS / Inno Setup
//! re-launch themselves from TEMP), bring its windows forward, close processes started from a folder
//! (port of ProcessTracker and LeftoverFinder.RunningFrom).

use std::collections::HashSet;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW,
    TerminateProcess, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetWindow, GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_RESTORE,
    SetForegroundWindow, ShowWindow,
};

pub struct Entry {
    pub pid: u32,
    pub parent: u32,
    pub exe: String,
}

pub fn snapshot() -> Vec<Entry> {
    let mut list = Vec::with_capacity(256);
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap == INVALID_HANDLE_VALUE {
            return list;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                list.push(Entry { pid: e.th32ProcessID, parent: e.th32ParentProcessID, exe: crate::sys::from_wide(&e.szExeFile) });
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    list
}

/// Follows a process tree that starts at one PID.
pub struct Tracker {
    tracked: HashSet<u32>,
    dead: HashSet<u32>,
    raised: HashSet<isize>,
    alive: HashSet<u32>,
    pub active_name: Option<String>,
}

impl Tracker {
    pub fn new(root: u32) -> Self {
        Self { tracked: [root].into(), dead: HashSet::new(), raised: HashSet::new(), alive: HashSet::new(), active_name: None }
    }

    /// Refreshes the tree. True while any process of the uninstaller is running.
    pub fn poll(&mut self) -> bool {
        let entries = snapshot();
        let present: HashSet<u32> = entries.iter().map(|e| e.pid).collect();
        for pid in &self.tracked {
            if !present.contains(pid) {
                self.dead.insert(*pid);
            }
        }
        loop {
            let mut added = false;
            for e in &entries {
                if self.tracked.contains(&e.pid) || e.pid == e.parent {
                    continue;
                }
                // a parent PID that died and is present again was reused by an unrelated process
                if self.tracked.contains(&e.parent) && !(self.dead.contains(&e.parent) && present.contains(&e.parent)) {
                    self.tracked.insert(e.pid);
                    added = true;
                }
            }
            if !added {
                break;
            }
        }
        self.alive.clear();
        self.active_name = None;
        for e in &entries {
            if self.tracked.contains(&e.pid) && !self.dead.contains(&e.pid) {
                self.alive.insert(e.pid);
                if self.active_name.is_none() {
                    self.active_name = Some(e.exe.clone());
                }
            }
        }
        !self.alive.is_empty()
    }

    /// Brings each visible top-level window of the uninstaller forward once.
    pub fn raise_windows(&mut self) {
        if self.alive.is_empty() {
            return;
        }
        struct Ctx<'a> {
            alive: &'a HashSet<u32>,
            raised: &'a mut HashSet<isize>,
        }
        unsafe extern "system" fn cb(hwnd: HWND, l: LPARAM) -> i32 {
            unsafe {
                let ctx = &mut *(l as *mut Ctx);
                let key = hwnd as isize;
                if ctx.raised.contains(&key) || IsWindowVisible(hwnd) == 0 || !GetWindow(hwnd, GW_OWNER).is_null() {
                    return 1;
                }
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, &mut pid);
                if !ctx.alive.contains(&pid) || GetWindowTextLengthW(hwnd) == 0 {
                    return 1;
                }
                ctx.raised.insert(key);
                if IsIconic(hwnd) != 0 {
                    ShowWindow(hwnd, SW_RESTORE);
                }
                SetForegroundWindow(hwnd);
                1
            }
        }
        let mut ctx = Ctx { alive: &self.alive, raised: &mut self.raised };
        unsafe { EnumWindows(Some(cb), &mut ctx as *mut Ctx as LPARAM) };
    }
}

/// Full path of a running process' executable.
pub fn image_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut n);
        CloseHandle(h);
        (ok != 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
    }
}

/// PIDs of processes whose executable lies inside `folder`.
pub fn running_from(folder: &std::path::Path) -> Vec<(u32, String)> {
    let f = format!("{}\\", folder.to_string_lossy().trim_end_matches('\\').to_lowercase());
    let me = std::process::id();
    snapshot()
        .into_iter()
        .filter(|e| e.pid != me && e.pid != 0)
        .filter_map(|e| image_path(e.pid).filter(|p| p.to_lowercase().starts_with(&f)).map(|p| (e.pid, p)))
        .collect()
}

/// Ends a process and waits up to three seconds.
pub fn kill(pid: u32) -> bool {
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE | 0x0010_0000 /* SYNCHRONIZE */, 0, pid);
        if h.is_null() {
            return false;
        }
        let ok = TerminateProcess(h, 1) != 0;
        WaitForSingleObject(h, 3000);
        CloseHandle(h);
        ok
    }
}

/// Lets the next foreground request of any process succeed (the uninstaller's window comes forward).
pub fn allow_any_foreground() {
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(u32::MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_a_child_process() {
        let mut child = std::process::Command::new(crate::sys::cmd::system_tool("cmd.exe"))
            .args(["/c", "ping", "-n", "3", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut t = Tracker::new(child.id());
        assert!(t.poll());
        let _ = child.wait();
        // ping (a grandchild) may still be finishing; poll until everything is gone
        let start = std::time::Instant::now();
        while t.poll() && start.elapsed().as_secs() < 10 {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        assert!(!t.poll());
        assert!(image_path(std::process::id()).unwrap().to_lowercase().ends_with(".exe"));
    }
}
