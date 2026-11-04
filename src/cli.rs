//! Command-line options, the same as the C# version:
//!
//! | Option | Effect |
//! |---|---|
//! | `--page <name>` | open a page: dashboard, programs, analyzer, cleanup, startup, journal, settings |
//! | `--scan <path>` | start analysing a folder or drive |
//! | `--theme light\|dark` | theme for this run |
//! | `--new-window` | open another window instead of switching to the running one |
//! | `--auto-clean` | scheduled cleanup without a window |
//! | `--go <command>` | an in-app command, e.g. `program:Steam`, `tab:types`, `programs:updates` |

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Args {
    pub page: Option<String>,
    pub scan: Option<String>,
    pub theme: Option<String>,
    /// An in-app navigation command (see `ProstorApp::navigate`).
    pub go: Option<String>,
    pub new_window: bool,
    pub auto_clean: bool,
    /// Sent by a second launch that only wants to bring the window forward.
    pub activate: bool,
}

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Args {
    let mut a = Args::default();
    let mut it = args.into_iter().peekable();
    while let Some(arg) = it.next() {
        match arg.to_ascii_lowercase().as_str() {
            "--page" => a.page = it.next(),
            "--scan" => a.scan = it.next(),
            "--theme" => a.theme = it.next().map(|t| t.to_ascii_lowercase()),
            "--go" => a.go = it.next(),
            "--new-window" => a.new_window = true,
            "--auto-clean" => a.auto_clean = true,
            "--activate" => a.activate = true,
            // a bare path (dropped on the exe or sent by Explorer) means "analyse it"
            _ if !arg.starts_with("--") && a.scan.is_none() && std::path::Path::new(&arg).exists() => a.scan = Some(arg),
            _ => {}
        }
    }
    a
}

impl Args {
    /// Back to a command line (to hand over to the running instance).
    pub fn to_vec(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(p) = &self.page {
            v.extend(["--page".to_string(), p.clone()]);
        }
        if let Some(s) = &self.scan {
            v.extend(["--scan".to_string(), s.clone()]);
        }
        if let Some(t) = &self.theme {
            v.extend(["--theme".to_string(), t.clone()]);
        }
        if let Some(g) = &self.go {
            v.extend(["--go".to_string(), g.clone()]);
        }
        if v.is_empty() {
            v.push("--activate".into());
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &[&str]) -> Args {
        parse(s.iter().map(|x| x.to_string()))
    }

    #[test]
    fn parses_options() {
        let a = p(&["--page", "startup", "--theme", "Dark", "--new-window"]);
        assert_eq!(a.page.as_deref(), Some("startup"));
        assert_eq!(a.theme.as_deref(), Some("dark"));
        assert!(a.new_window);
        assert!(p(&["--auto-clean"]).auto_clean);
        let w = std::env::var("SystemRoot").unwrap();
        assert_eq!(p(&[&w]).scan.as_deref(), Some(w.as_str()));
        assert_eq!(p(&["--scan", r"D:\x"]).to_vec(), vec!["--scan", r"D:\x"]);
        assert_eq!(p(&[]).to_vec(), vec!["--activate"]);
    }
}
