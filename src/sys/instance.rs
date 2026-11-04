//! One window per user session: a second launch (e.g. "Анализировать в «Простор»" from Explorer)
//! hands its arguments to the running instance and exits (port of Core/SingleInstance.cs).

use std::io::{Read, Write};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT};
use windows_sys::Win32::System::Threading::CreateMutexW;

fn id() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let mut session = 0u32;
    unsafe { windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(std::process::id(), &mut session) };
    format!("Prostor2.{user}.{session}")
}

/// True when this is the first instance. The mutex lives as long as the process.
pub fn acquire() -> bool {
    let name = super::wide(&format!(r"Local\{}", id()));
    unsafe {
        let h: HANDLE = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if h.is_null() {
            return false;
        }
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

fn pipe_name() -> String {
    format!(r"\\.\pipe\{}", id())
}

/// Sends the arguments to the running instance.
pub fn send(args: &[String]) -> bool {
    for _ in 0..15 {
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(pipe_name()) {
            return f.write_all(args.join("\n").as_bytes()).is_ok();
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

/// Receives arguments from later launches on a background thread.
pub fn listen(on_args: impl Fn(Vec<String>) + Send + 'static) {
    std::thread::Builder::new()
        .name("prostor-instance".into())
        .spawn(move || {
            let name = super::wide(&pipe_name());
            loop {
                let pipe = unsafe {
                    CreateNamedPipeW(name.as_ptr(), 0x0000_0001 /* PIPE_ACCESS_INBOUND */, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT, 1, 0, 64 * 1024, 0, std::ptr::null())
                };
                if pipe == INVALID_HANDLE_VALUE {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                }
                let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == 535; // ERROR_PIPE_CONNECTED
                if connected {
                    use std::os::windows::io::FromRawHandle;
                    // the File takes ownership of the handle; read until the client closes
                    let mut f = unsafe { std::fs::File::from_raw_handle(pipe as _) };
                    let mut buf = Vec::new();
                    let _ = f.read_to_end(&mut buf);
                    unsafe { DisconnectNamedPipe(pipe) };
                    drop(f);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    let args: Vec<String> = if text.is_empty() { Vec::new() } else { text.split('\n').map(str::to_string).collect() };
                    on_args(args);
                } else {
                    unsafe { CloseHandle(pipe) };
                }
            }
        })
        .ok();
}
