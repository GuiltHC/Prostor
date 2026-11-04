//! State of physical disks from the Windows storage subsystem: SMART-based health, wear and temperature
//! (port of Services/DiskHealthService.cs).

use crate::sys::cmd;
use serde::Deserialize;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Danger,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct DiskHealth {
    pub model: String,
    /// "SSD", "HDD", "NVMe SSD"…
    pub media: String,
    pub level: Level,
    pub status: String,
    pub temperature: Option<i64>,
    /// Percent of rated endurance used (SSD).
    pub wear: Option<i64>,
    pub power_on_hours: Option<i64>,
    pub letters: Vec<char>,
}

impl DiskHealth {
    pub fn summary(&self) -> String {
        let mut parts = vec![self.status.clone()];
        if let Some(w) = self.wear {
            parts.push(format!("износ {w}%"));
        }
        if let Some(t) = self.temperature.filter(|t| *t > 0) {
            parts.push(format!("{t} °C"));
        }
        if let Some(h) = self.power_on_hours.filter(|h| *h > 0) {
            parts.push(format!("{} дн. в работе", crate::fmt::number((h / 24) as u64)));
        }
        parts.join(" · ")
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Raw {
    device_id: Option<String>,
    friendly_name: Option<String>,
    media_type: Option<i64>,
    bus_type: Option<i64>,
    health_status: Option<i64>,
    temperature: Option<i64>,
    wear: Option<i64>,
    power_on_hours: Option<i64>,
    letters: Option<String>,
}

pub fn load() -> Vec<DiskHealth> {
    let script = r#"
$parts = @(Get-CimInstance -Namespace root/Microsoft/Windows/Storage -ClassName MSFT_Partition -ErrorAction SilentlyContinue)
$list = @(Get-CimInstance -Namespace root/Microsoft/Windows/Storage -ClassName MSFT_PhysicalDisk -ErrorAction SilentlyContinue | ForEach-Object {
  $d = $_
  $r = $null
  try { $r = Get-CimAssociatedInstance -InputObject $d -ResultClassName MSFT_StorageReliabilityCounter -ErrorAction Stop } catch {}
  $letters = ($parts | Where-Object { "$($_.DiskNumber)" -eq "$($d.DeviceId)" -and $_.DriveLetter -match '[A-Za-z]' } | ForEach-Object { [string]$_.DriveLetter }) -join ''
  [pscustomobject]@{ DeviceId = "$($d.DeviceId)"; FriendlyName = $d.FriendlyName; MediaType = [int]$d.MediaType; BusType = [int]$d.BusType;
    HealthStatus = [int]$d.HealthStatus; Temperature = $r.Temperature; Wear = $r.Wear; PowerOnHours = $r.PowerOnHours; Letters = $letters }
})
ConvertTo-Json -InputObject $list -Compress
"#;
    let Ok(r) = cmd::powershell(script, Duration::from_secs(60)) else { return Vec::new() };
    let text = r.output.trim();
    let start = text.find('[').unwrap_or(0);
    let raw: Vec<Raw> = serde_json::from_str(&text[start..]).unwrap_or_default();
    raw.into_iter().map(convert).collect()
}

fn convert(d: Raw) -> DiskHealth {
    let (media, bus) = (d.media_type.unwrap_or(0), d.bus_type.unwrap_or(0));
    let media_text = match media {
        4 => if bus == 17 { "NVMe SSD" } else { "SSD" },
        3 => "HDD",
        _ => match bus {
            17 => "NVMe",
            7 => "USB",
            _ => "Диск",
        },
    };
    let mut level = match d.health_status.unwrap_or(5) {
        0 => Level::Ok,
        1 => Level::Warn,
        2 => Level::Danger,
        _ => Level::Unknown,
    };
    let worn = d.wear.is_some_and(|w| w >= 90);
    let hot = d.temperature.is_some_and(|t| t >= 70);
    if level == Level::Ok && (worn || hot) {
        level = Level::Warn;
    }
    let status = match level {
        Level::Ok => "Исправен",
        Level::Warn if worn => "Сильно изношен",
        Level::Warn if hot => "Перегрев",
        Level::Warn => "Есть предупреждения",
        Level::Danger => "Неисправен — сделайте копию данных",
        Level::Unknown => "Состояние неизвестно",
    };
    let _ = d.device_id;
    DiskHealth {
        model: d.friendly_name.unwrap_or_default().trim().to_string(),
        media: media_text.into(),
        level,
        status: status.into(),
        temperature: d.temperature,
        wear: d.wear,
        power_on_hours: d.power_on_hours,
        letters: d.letters.unwrap_or_default().chars().map(|c| c.to_ascii_uppercase()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts() {
        let d = convert(Raw {
            device_id: Some("0".into()),
            friendly_name: Some(" Samsung 980 ".into()),
            media_type: Some(4),
            bus_type: Some(17),
            health_status: Some(0),
            temperature: Some(75),
            wear: Some(3),
            power_on_hours: Some(4800),
            letters: Some("cd".into()),
        });
        assert_eq!(d.media, "NVMe SSD");
        assert_eq!(d.level, Level::Warn);
        assert_eq!(d.status, "Перегрев");
        assert_eq!(d.letters, vec!['C', 'D']);
        assert!(d.summary().contains("200 дн."));
    }

    #[test]
    fn loads_disks() {
        let disks = load();
        assert!(!disks.is_empty(), "at least the system disk is reported");
    }
}
