//! Task Scheduler through schtasks.exe and task XML (port of Services/TaskSchedulerHelper.cs).

use crate::sys::cmd::{self, Encoding};
use std::time::Duration;

fn schtasks(args: &[&str]) -> std::io::Result<cmd::CmdResult> {
    cmd::run(cmd::system_tool("schtasks.exe"), args, Encoding::Oem, Duration::from_secs(60))
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// DOMAIN\user of the current user.
pub fn current_user() -> String {
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    let user = std::env::var("USERNAME").unwrap_or_default();
    if domain.is_empty() { user } else { format!("{domain}\\{user}") }
}

/// Creates or replaces a task from XML. `path` is like `\Prostor\AutoClean`.
pub fn register(path: &str, xml: &str) -> Result<(), String> {
    let file = std::env::temp_dir().join(format!("prostor-task-{}.xml", std::process::id()));
    // schtasks wants UTF-16 with a BOM
    let mut bytes = vec![0xFF, 0xFE];
    for u in xml.encode_utf16() {
        bytes.extend(u.to_le_bytes());
    }
    std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
    let r = schtasks(&["/Create", "/TN", path, "/XML", &file.to_string_lossy(), "/F"]);
    let _ = std::fs::remove_file(&file);
    match r {
        Ok(r) if r.ok() => Ok(()),
        Ok(r) => Err(r.output.trim().to_string()),
        Err(e) => Err(e.to_string()),
    }
}

pub fn exists(path: &str) -> bool {
    schtasks(&["/Query", "/TN", path]).is_ok_and(|r| r.ok())
}

pub fn delete(path: &str) {
    let _ = schtasks(&["/Delete", "/TN", path, "/F"]);
}

pub fn set_enabled(path: &str, enabled: bool) -> Result<(), String> {
    match schtasks(&["/Change", "/TN", path, if enabled { "/ENABLE" } else { "/DISABLE" }]) {
        Ok(r) if r.ok() => Ok(()),
        Ok(r) => Err(r.output.trim().to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// The task's XML (used as a backup before deleting it).
///
/// schtasks writes the XML in the console code page, which loses characters outside it,
/// so the export goes through PowerShell with UTF-8 output.
pub fn export_xml(path: &str) -> Option<String> {
    let (folder, name) = match path.rfind('\\') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("\\", path),
    };
    let q = |s: &str| s.replace('\'', "''");
    let script = format!("Export-ScheduledTask -TaskPath '{}' -TaskName '{}' -ErrorAction Stop", q(folder), q(name));
    let r = cmd::powershell(&script, Duration::from_secs(60)).ok()?;
    (r.ok() && r.output.contains("<Task")).then(|| r.output.trim().to_string())
}

const SETTINGS_XML: &str = "  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <ExecutionTimeLimit>PT2H</ExecutionTimeLimit>
  </Settings>";

fn wrap(description: &str, trigger: &str, command: &str, arguments: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>
<Task version=\"1.3\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">
  <RegistrationInfo><Author>Простор</Author><Description>{}</Description></RegistrationInfo>
  <Triggers>{trigger}</Triggers>
  <Principals>
    <Principal id=\"Author\"><UserId>{}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal>
  </Principals>
{SETTINGS_XML}
  <Actions Context=\"Author\"><Exec><Command>{}</Command><Arguments>{}</Arguments></Exec></Actions>
</Task>",
        escape(description),
        escape(&current_user()),
        escape(command),
        escape(arguments)
    )
}

/// Weekly (Sunday) or monthly (1st day) at 12:00; missed runs start as soon as the PC is on.
pub fn calendar_task_xml(description: &str, monthly: bool, command: &str, arguments: &str) -> String {
    let start = (chrono::Local::now().date_naive() + chrono::Days::new(1)).format("%Y-%m-%dT12:00:00");
    let schedule = if monthly {
        "<ScheduleByMonth><DaysOfMonth><Day>1</Day></DaysOfMonth><Months><January/><February/><March/><April/><May/><June/><July/><August/><September/><October/><November/><December/></Months></ScheduleByMonth>"
    } else {
        "<ScheduleByWeek><WeeksInterval>1</WeeksInterval><DaysOfWeek><Sunday/></DaysOfWeek></ScheduleByWeek>"
    };
    wrap(
        description,
        &format!("<CalendarTrigger><StartBoundary>{start}</StartBoundary><Enabled>true</Enabled>{schedule}</CalendarTrigger>"),
        command,
        arguments,
    )
}

/// Runs at the user's logon after a delay like "PT1M".
pub fn logon_task_xml(description: &str, delay: &str, command: &str, arguments: &str) -> String {
    wrap(
        description,
        &format!(
            "<LogonTrigger><Enabled>true</Enabled><UserId>{}</UserId><Delay>{delay}</Delay></LogonTrigger>",
            escape(&current_user())
        ),
        command,
        arguments,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_queries_and_deletes() {
        let path = format!(r"\Prostor\Test-{}", std::process::id());
        let xml = calendar_task_xml("тест <&>", false, r"C:\Windows\System32\cmd.exe", "/c exit");
        register(&path, &xml).expect("register");
        assert!(exists(&path));
        let back = export_xml(&path).unwrap();
        assert!(back.contains("тест &lt;&amp;&gt;"), "{back}");
        set_enabled(&path, false).unwrap();
        delete(&path);
        assert!(!exists(&path));
    }
}
