//! UI-facing application state: the shared snapshot and small formatting helpers.
//! Rendering lives in `ui/`.

use crate::domain::Session;
use chrono::{Local, TimeZone};

/// Compatibility name for the structured read model consumed by the UI.
pub use crate::read_model::RefreshSnapshot as Snapshot;

// ---- formatting helpers ----

/// Compact token counts: 1234 → "1.2k", 1_500_000 → "1.5M".
pub fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Bare dollar amount; `None` → `N/A`.
pub fn fmt_cost(cost: Option<f64>) -> String {
    match cost {
        Some(c) if c >= 100.0 => format!("${c:.0}"),
        Some(c) if c >= 0.01 => format!("${c:.2}"),
        Some(c) if c > 0.0 => format!("${c:.4}"),
        Some(_) => "$0.00".to_string(),
        None => "N/A".to_string(),
    }
}

/// Context as `tokens (pct%)`, or `tokens (N/A)` when the window is unknown.
pub fn fmt_context(s: &Session) -> String {
    match s.context.percent() {
        Some(pct) => format!("{} ({:.0}%)", fmt_tokens(s.context.tokens), pct),
        None => format!("{} (N/A)", fmt_tokens(s.context.tokens)),
    }
}

/// Relative "last update" like `5s`, `12m`, `3h`, `2d`.
pub fn fmt_relative(ms: i64, now_ms: i64) -> String {
    let d = (now_ms - ms).max(0) / 1000; // seconds
    if d < 60 {
        format!("{d}s")
    } else if d < 3600 {
        format!("{}m", d / 60)
    } else if d < 86_400 {
        format!("{}h", d / 3600)
    } else {
        format!("{}d", d / 86_400)
    }
}

/// Format an epoch-ms instant as a local `MM-DD HH:MM` timestamp.
pub fn fmt_datetime(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%m-%d %H:%M").to_string(),
        None => "—".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_formatting() {
        assert_eq!(fmt_tokens(500), "500");
        assert_eq!(fmt_tokens(1_500), "1.5k");
        assert_eq!(fmt_tokens(2_000_000), "2.0M");
    }

    #[test]
    fn cost_formatting_na_for_unknown() {
        assert_eq!(fmt_cost(None), "N/A");
        assert_eq!(fmt_cost(Some(0.0)), "$0.00");
        assert_eq!(fmt_cost(Some(12.5)), "$12.50");
        assert_eq!(fmt_cost(Some(250.0)), "$250");
    }
}
