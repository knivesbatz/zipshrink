//! Formatting shared by the screens.

use std::path::Path;
use std::time::Duration;

/// Sizes the way File Explorer shows them (1024-based, "KB/MB/GB").
pub fn size(n: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return if n == 1 { "1 byte".into() } else { format!("{n} bytes") };
    }
    let mut v = n as f64 / 1024.0;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[u])
    } else {
        format!("{v:.2} {}", UNITS[u])
    }
}

/// Parses what people type into size fields: "650 MB", "4.7 GB", "25mb".
pub fn parse_size(s: &str) -> Result<u64, String> {
    let t = s.trim();
    if t.is_empty() {
        return Err("Enter a size, such as 650 MB.".into());
    }
    zipshrink_core::util::parse_size(t).map_err(|_| "Use a number and a unit, such as 650 MB or 4 GB.".into())
}

pub fn count(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{} {many}", group(n))
    }
}

/// 12345 -> "12,345".
pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{} hr {} min", s / 3600, (s % 3600) / 60)
    } else if s >= 90 {
        format!("{} min", (s + 30) / 60)
    } else if s >= 50 {
        "About a minute".into()
    } else {
        format!("{} sec", s.max(1))
    }
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

pub fn parent(p: &Path) -> String {
    p.parent().map(|d| d.display().to_string()).unwrap_or_default()
}

pub fn percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "—".into();
    }
    format!("{:.0}%", 100.0 * part as f64 / whole as f64)
}
