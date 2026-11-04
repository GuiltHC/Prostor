//! Russian-friendly formatting of sizes, counts and dates (port of Core/Fmt.cs).

const UNITS: [&str; 5] = ["КБ", "МБ", "ГБ", "ТБ", "ПБ"];

/// "512 Б", "1,5 МБ", "120 ГБ".
pub fn size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} Б");
    }
    let mut v = bytes as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    let num = if v >= 100.0 {
        format!("{v:.0}")
    } else {
        let s = format!("{v:.1}");
        let s = s.strip_suffix(".0").unwrap_or(&s).to_string();
        s.replace('.', ",")
    };
    format!("{num} {}", UNITS[i])
}

/// "1 234 567" with a narrow no-break space as the thousands separator.
pub fn number(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3 * 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('\u{202F}');
        }
        out.push(ch);
    }
    out
}

pub fn plural<'a>(n: u64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let m100 = n % 100;
    if (11..=14).contains(&m100) {
        return many;
    }
    match n % 10 {
        1 => one,
        2..=4 => few,
        _ => many,
    }
}

pub fn count(n: u64, one: &str, few: &str, many: &str) -> String {
    format!("{} {}", number(n), plural(n, one, few, many))
}

pub fn files(n: u64) -> String {
    count(n, "файл", "файла", "файлов")
}

pub fn folders(n: u64) -> String {
    count(n, "папка", "папки", "папок")
}

pub fn percent(fraction: f64) -> String {
    let p = fraction * 100.0;
    if p <= 0.0 {
        "0%".into()
    } else if p < 0.1 {
        "<0,1%".into()
    } else if p >= 10.0 {
        format!("{p:.0}%")
    } else {
        let s = format!("{p:.1}");
        format!("{}%", s.strip_suffix(".0").unwrap_or(&s).replace('.', ","))
    }
}

/// Seconds as "0:42" or "1:02:03".
pub fn clock(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

const MONTHS: [&str; 12] = [
    "янв", "фев", "мар", "апр", "мая", "июн", "июл", "авг", "сен", "окт", "ноя", "дек",
];

/// FILETIME (100 ns ticks since 1601-01-01 UTC) as "4 ноя 2026", or "—".
pub fn filetime_date(ft: u64) -> String {
    match filetime_to_ymd(ft) {
        Some((y, m, d)) => format!("{d} {} {y}", MONTHS[(m - 1) as usize]),
        None => "—".into(),
    }
}

fn filetime_to_ymd(ft: u64) -> Option<(i64, u32, u32)> {
    const TICKS_PER_SEC: u64 = 10_000_000;
    const EPOCH_DIFF: u64 = 11_644_473_600; // seconds between 1601 and 1970
    let secs = (ft / TICKS_PER_SEC).checked_sub(EPOCH_DIFF)?;
    Some(civil_from_days((secs / 86_400) as i64))
}

/// Howard Hinnant's days-from-civil inverse.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0 Б");
        assert_eq!(size(1023), "1023 Б");
        assert_eq!(size(1024), "1 КБ");
        assert_eq!(size(1536), "1,5 КБ");
        assert_eq!(size(150 * 1024 * 1024), "150 МБ");
        assert_eq!(size(5 * 1024 * 1024 * 1024), "5 ГБ");
    }

    #[test]
    fn plurals() {
        assert_eq!(files(1), "1 файл");
        assert_eq!(files(3), "3 файла");
        assert_eq!(files(11), "11 файлов");
        assert_eq!(files(21), "21 файл");
        assert_eq!(files(1000), "1\u{202F}000 файлов");
    }

    #[test]
    fn dates() {
        // 2026-11-04 00:00 UTC
        let secs = 1_793_750_400u64 + 11_644_473_600;
        assert_eq!(filetime_date(secs * 10_000_000), "4 ноя 2026");
        assert_eq!(filetime_date(0), "—");
    }

    #[test]
    fn percents() {
        assert_eq!(percent(0.5), "50%");
        assert_eq!(percent(0.055), "5,5%");
        assert_eq!(percent(0.0001), "<0,1%");
    }
}
