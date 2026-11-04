//! Free space of each drive, one point per day, recorded whenever the app runs
//! (port of Services/DiskHistory.cs, same disk-history.json format).

use crate::data;
use crate::sys::drives;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;

const KEEP_DAYS: i64 = 400;
static LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub struct Point {
    pub date: DateTime<Local>,
    pub free: i64,
    pub total: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Forecast {
    /// Change of free space per day (negative — the disk fills up).
    pub bytes_per_day: f64,
    pub days_until_full: Option<i64>,
    pub days: i64,
    pub points: usize,
}

pub type History = BTreeMap<String, Vec<Point>>;

fn path() -> std::path::PathBuf {
    data::data_dir().join("disk-history.json")
}

pub fn load() -> History {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    data::read_json(&path()).unwrap_or_default()
}

/// Replaces today's point of every fixed drive.
pub fn record() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h: History = data::read_json(&path()).unwrap_or_default();
    let now = Local::now();
    let today = now.date_naive();
    for d in drives::list().into_iter().filter(|d| !d.removable) {
        let list = h.entry(d.letter().to_string()).or_default();
        list.retain(|p| p.date.date_naive() != today && (today - p.date.date_naive()).num_days() <= KEEP_DAYS);
        list.push(Point { date: now, free: d.free as i64, total: d.total as i64 });
        list.sort_by_key(|p| p.date);
    }
    if let Err(e) = data::write_json(&path(), &h) {
        data::log_error("disk-history", &e);
    }
}

/// Records on a background thread (after cleaning, deleting, uninstalling).
pub fn record_async() {
    std::thread::spawn(record);
}

/// Linear trend of free space over the last `window_days` days.
pub fn forecast(points: &[Point], window_days: i64) -> Option<Forecast> {
    let last = points.last()?.date;
    let window: Vec<&Point> = points.iter().filter(|p| (last - p.date).num_days() <= window_days).collect();
    if window.len() < 3 {
        return None;
    }
    let t0 = window[0].date;
    let span = (window[window.len() - 1].date - t0).num_seconds() as f64 / 86_400.0;
    if span < 3.0 {
        return None;
    }
    let xs: Vec<f64> = window.iter().map(|p| (p.date - t0).num_seconds() as f64 / 86_400.0).collect();
    let ys: Vec<f64> = window.iter().map(|p| p.free as f64).collect();
    let n = xs.len() as f64;
    let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
    let sxx: f64 = xs.iter().map(|x| (x - mx) * (x - mx)).sum();
    if sxx <= 0.0 {
        return None;
    }
    let slope = xs.iter().zip(&ys).map(|(x, y)| (x - mx) * (y - my)).sum::<f64>() / sxx;
    let days_until_full = (slope < -1.0)
        .then(|| window[window.len() - 1].free as f64 / -slope)
        .filter(|d| *d < 3650.0)
        .map(|d| d.round() as i64);
    Some(Forecast { bytes_per_day: slope, days_until_full, days: span.round() as i64, points: window.len() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn forecasts_a_filling_disk() {
        let start = Local::now() - Duration::days(30);
        let pts: Vec<Point> = (0..=30)
            .map(|d| Point { date: start + Duration::days(d), free: 100_000_000_000 - d * 1_000_000_000, total: 500_000_000_000 })
            .collect();
        let f = forecast(&pts, 90).unwrap();
        assert!((f.bytes_per_day + 1e9).abs() < 1e6);
        assert_eq!(f.days_until_full, Some(70));
        assert_eq!(f.points, 31);
        assert!(forecast(&pts[..2], 90).is_none());
    }

    #[test]
    fn reads_csharp_file() {
        let json = r#"{"C:":[{"Date":"2026-10-01T10:00:00.123+03:00","Free":1000,"Total":5000}]}"#;
        let h: History = serde_json::from_str(json).unwrap();
        assert_eq!(h["C:"][0].free, 1000);
    }
}
