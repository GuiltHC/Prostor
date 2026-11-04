//! Runs console tools (reg, powercfg, compact, vssadmin, pnputil, dism, winget, schtasks) hidden,
//! capturing their output (port of Core/Cmd.cs).

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone)]
pub struct CmdResult {
    pub code: i32,
    pub output: String,
}

impl CmdResult {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// Classic console tools (CP866 on Russian Windows).
    Oem,
    Utf8,
    /// PowerShell / tools writing UTF-16.
    Utf16,
}

pub fn system_tool(exe: &str) -> PathBuf {
    let mut buf = [0u16; 260];
    let n = unsafe { windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(buf.as_mut_ptr(), 260) } as usize;
    let dir = if n > 0 && n < 260 { String::from_utf16_lossy(&buf[..n]) } else { r"C:\Windows\System32".into() };
    PathBuf::from(dir).join(exe)
}

pub fn decode(bytes: &[u8], enc: Encoding) -> String {
    match enc {
        Encoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        Encoding::Utf16 => {
            let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&units).trim_start_matches('\u{feff}').to_string()
        }
        Encoding::Oem => {
            if let Ok(s) = std::str::from_utf8(bytes) {
                return s.to_string();
            }
            oem_to_string(bytes)
        }
    }
}

fn oem_to_string(bytes: &[u8]) -> String {
    use windows_sys::Win32::Globalization::{GetOEMCP, MultiByteToWideChar};
    if bytes.is_empty() {
        return String::new();
    }
    unsafe {
        let cp = GetOEMCP();
        let n = MultiByteToWideChar(cp, 0, bytes.as_ptr(), bytes.len() as i32, std::ptr::null_mut(), 0);
        if n <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut out = vec![0u16; n as usize];
        MultiByteToWideChar(cp, 0, bytes.as_ptr(), bytes.len() as i32, out.as_mut_ptr(), n);
        String::from_utf16_lossy(&out)
    }
}

/// Runs `file` with `args` (each passed as its own argument) without a window.
pub fn run(file: impl Into<PathBuf>, args: &[&str], enc: Encoding, timeout: Duration) -> std::io::Result<CmdResult> {
    let mut c = Command::new(file.into());
    c.args(args);
    run_command(c, enc, timeout, None)
}

/// Like [`run`] but with a raw, already quoted argument string (needed by tools with odd parsing).
pub fn run_raw(file: impl Into<PathBuf>, raw_args: &str, enc: Encoding, timeout: Duration) -> std::io::Result<CmdResult> {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new(file.into());
    c.raw_arg(raw_args);
    run_command(c, enc, timeout, None)
}

pub fn run_command(mut c: Command, enc: Encoding, timeout: Duration, cancel: Option<&AtomicBool>) -> std::io::Result<CmdResult> {
    use std::os::windows::process::CommandExt;
    c.creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn()?;
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let t_out = std::thread::spawn(move || {
        let mut v = Vec::new();
        if let Some(o) = out.as_mut() {
            let _ = o.read_to_end(&mut v);
        }
        v
    });
    let t_err = std::thread::spawn(move || {
        let mut v = Vec::new();
        if let Some(e) = err.as_mut() {
            let _ = e.read_to_end(&mut v);
        }
        v
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        let cancelled = cancel.is_some_and(|c| c.load(Ordering::Relaxed));
        if cancelled || start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                if cancelled { "отменено".to_string() } else { "программа не ответила вовремя".to_string() },
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut bytes = t_out.join().unwrap_or_default();
    let eb = t_err.join().unwrap_or_default();
    if !eb.is_empty() {
        if !bytes.is_empty() && enc != Encoding::Utf16 {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(&eb);
    }
    Ok(CmdResult { code: status.code().unwrap_or(-1), output: decode(&bytes, enc) })
}

/// PowerShell one-liner with UTF-8 output.
pub fn powershell(script: &str, timeout: Duration) -> std::io::Result<CmdResult> {
    let full = format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; $ProgressPreference='SilentlyContinue'; {script}");
    run(
        system_tool(r"WindowsPowerShell\v1.0\powershell.exe"),
        &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &full],
        Encoding::Utf8,
        timeout,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_cmd() {
        let r = run(system_tool("cmd.exe"), &["/c", "echo", "hello"], Encoding::Oem, Duration::from_secs(10)).unwrap();
        assert!(r.ok());
        assert!(r.output.contains("hello"), "{:?}", r.output);
        // Cyrillic survives the console code page only where that code page has it (866 on Russian Windows)
        if unsafe { windows_sys::Win32::Globalization::GetOEMCP() } == 866 {
            let r = run(system_tool("cmd.exe"), &["/c", "echo", "привет"], Encoding::Oem, Duration::from_secs(10)).unwrap();
            assert!(r.output.contains("привет"), "{:?}", r.output);
        }
    }

    #[test]
    fn timeout_kills() {
        let r = run(system_tool("ping.exe"), &["-n", "10", "127.0.0.1"], Encoding::Oem, Duration::from_millis(300));
        assert!(r.is_err());
    }
}
