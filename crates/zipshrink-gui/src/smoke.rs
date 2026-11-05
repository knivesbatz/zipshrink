//! Smoke-test mode for CI: with `ZIPSHRINK_SMOKE=<seconds>` set, the app
//! reports on stderr, one line each, `ZIPSHRINK_SMOKE ready` once the first
//! frame is drawn and `ZIPSHRINK_SMOKE open <path>` for every path it is
//! handed (command line, another launch, Finder, Services or a drop), then
//! closes itself after that many seconds. Other `ZIPSHRINK_SMOKE ...` lines
//! are diagnostics.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Used when the variable is set but is not a number of seconds.
const DEFAULT: Duration = Duration::from_secs(10);

struct Smoke {
    start: Instant,
    run_for: Duration,
}

static SMOKE: OnceLock<Option<Smoke>> = OnceLock::new();

fn parse(v: &str) -> Duration {
    match v.trim().parse::<f64>() {
        Ok(s) if s.is_finite() && s > 0.0 => Duration::from_secs_f64(s.min(3600.0)),
        _ => DEFAULT,
    }
}

fn state() -> Option<&'static Smoke> {
    SMOKE
        .get_or_init(|| {
            let v = std::env::var("ZIPSHRINK_SMOKE").ok()?;
            Some(Smoke { start: Instant::now(), run_for: parse(&v) })
        })
        .as_ref()
}

/// Starts the clock; call first thing in `main`.
pub fn init() {
    let _ = state();
}

pub fn active() -> bool {
    state().is_some()
}

pub fn log(line: &str) {
    if active() {
        eprintln!("ZIPSHRINK_SMOKE {line}");
    }
}

/// One line per path of a batch the app was handed.
pub fn opened(paths: &[PathBuf]) {
    if active() {
        for p in paths {
            eprintln!("ZIPSHRINK_SMOKE open {}", p.display());
        }
    }
}

/// Time left before the app should close itself (`None` outside smoke mode).
pub fn remaining() -> Option<Duration> {
    state().map(|s| s.run_for.saturating_sub(s.start.elapsed()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds() {
        assert_eq!(parse("3"), Duration::from_secs(3));
        assert_eq!(parse(" 1.5 "), Duration::from_millis(1500));
        assert_eq!(parse(""), DEFAULT);
        assert_eq!(parse("yes"), DEFAULT);
        assert_eq!(parse("-2"), DEFAULT);
        assert_eq!(parse("NaN"), DEFAULT);
        assert_eq!(parse("1e9"), Duration::from_secs(3600));
    }
}
