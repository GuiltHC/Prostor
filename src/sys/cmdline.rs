//! Splits command lines like those in UninstallString or Run keys (port of Core/CommandLine.cs).

use std::path::Path;

/// Expands %VARIABLES% the way ExpandEnvironmentStrings does.
pub fn expand_env(s: &str) -> String {
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
    let w = super::wide(s);
    unsafe {
        let n = ExpandEnvironmentStringsW(w.as_ptr(), std::ptr::null_mut(), 0);
        if n == 0 {
            return s.to_string();
        }
        let mut buf = vec![0u16; n as usize];
        ExpandEnvironmentStringsW(w.as_ptr(), buf.as_mut_ptr(), n);
        super::from_wide(&buf)
    }
}

/// (file, arguments)
pub fn split(command: &str) -> (String, String) {
    let cmd = expand_env(command.trim());
    if cmd.is_empty() {
        return (String::new(), String::new());
    }
    if let Some(rest) = cmd.strip_prefix('"') {
        return match rest.find('"') {
            Some(end) => (rest[..end].to_string(), rest[end + 1..].trim().to_string()),
            None => (rest.trim_matches('"').to_string(), String::new()),
        };
    }
    // unquoted path that may contain spaces: shortest prefix that resolves to an existing file
    let mut idx = 0;
    loop {
        let sp = cmd[idx..].find(' ').map(|i| i + idx);
        let candidate = match sp {
            Some(s) => &cmd[..s],
            None => &cmd[..],
        };
        if let Some(resolved) = resolve(candidate) {
            let args = sp.map(|s| cmd[s + 1..].trim().to_string()).unwrap_or_default();
            return (resolved, args);
        }
        match sp {
            Some(s) => idx = s + 1,
            None => break,
        }
    }
    match cmd.find(' ') {
        Some(f) => (cmd[..f].to_string(), cmd[f + 1..].trim().to_string()),
        None => (cmd, String::new()),
    }
}

/// The executable referenced by a command line, if it exists on disk.
pub fn extract_executable(command: &str) -> Option<String> {
    let (file, _) = split(command);
    if file.is_empty() { None } else { resolve(&file) }
}

fn resolve(candidate: &str) -> Option<String> {
    if candidate.is_empty() || candidate.contains(['<', '>', '|', '"']) {
        return None;
    }
    let p = Path::new(candidate);
    if p.is_file() {
        return Some(candidate.to_string());
    }
    if !candidate.to_ascii_lowercase().ends_with(".exe") && Path::new(&format!("{candidate}.exe")).is_file() {
        return Some(format!("{candidate}.exe"));
    }
    if !candidate.contains('\\') {
        let sys = super::cmd::system_tool("");
        let win = sys.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        for dir in [sys, win] {
            let full = dir.join(candidate);
            if full.is_file() {
                return Some(full.to_string_lossy().into_owned());
            }
            let exe = dir.join(format!("{candidate}.exe"));
            if exe.is_file() {
                return Some(exe.to_string_lossy().into_owned());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits() {
        assert_eq!(split(r#""C:\Program Files\x\u.exe" /S"#), (r"C:\Program Files\x\u.exe".into(), "/S".into()));
        let (f, a) = split(r"C:\Windows\System32\cmd.exe /c echo");
        assert!(f.to_lowercase().ends_with("cmd.exe"));
        assert_eq!(a, "/c echo");
        let (f, a) = split("msiexec /x {GUID}");
        assert!(f.to_lowercase().ends_with("msiexec.exe"), "{f}");
        assert_eq!(a, "/x {GUID}");
        assert!(extract_executable(r"%SystemRoot%\System32\cmd.exe /c").is_some());
    }
}
