//! UI-facing application state: tabs, time filters, the shared snapshot, and
//! small formatting helpers. Rendering lives in `ui/`.

use crate::alerts::AlertCounts;
use crate::collector::ScanStats;
use crate::domain::{Provider, Session};
use chrono::{Duration, Local, TimeZone};

/// The three top-level tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    All,
    Claude,
    Codex,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::All, Tab::Claude, Tab::Codex];

    pub fn title(self) -> &'static str {
        match self {
            Tab::All => "All",
            Tab::Claude => "Claude Code",
            Tab::Codex => "Codex CLI",
        }
    }

    pub fn provider(self) -> Option<Provider> {
        match self {
            Tab::All => None,
            Tab::Claude => Some(Provider::ClaudeCode),
            Tab::Codex => Some(Provider::CodexCli),
        }
    }

    pub fn index(self) -> usize {
        match self {
            Tab::All => 0,
            Tab::Claude => 1,
            Tab::Codex => 2,
        }
    }
}

/// Activity-time filters. A matched session always shows its full lifetime usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeFilter {
    Today,
    Last7,
    Last30,
    All,
}

impl TimeFilter {
    pub const ALL: [TimeFilter; 4] = [TimeFilter::Today, TimeFilter::Last7, TimeFilter::Last30, TimeFilter::All];

    pub fn title(self) -> &'static str {
        match self {
            TimeFilter::Today => "Active Today",
            TimeFilter::Last7 => "Last 7 Days",
            TimeFilter::Last30 => "Last 30 Days",
            TimeFilter::All => "All Sessions",
        }
    }

    pub fn next(self) -> TimeFilter {
        let all = TimeFilter::ALL;
        let i = all.iter().position(|&f| f == self).unwrap_or(0);
        all[(i + 1) % all.len()]
    }

    /// Epoch-ms cutoff; sessions with `last_activity_ms >= cutoff` match. 0 = all time.
    pub fn cutoff_ms(self) -> i64 {
        let now = Local::now();
        let start = match self {
            TimeFilter::Today => now
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .and_then(|d| Local.from_local_datetime(&d).single())
                .unwrap_or(now),
            TimeFilter::Last7 => now - Duration::days(7),
            TimeFilter::Last30 => now - Duration::days(30),
            TimeFilter::All => return 0,
        };
        start.timestamp_millis()
    }
}

/// Immutable snapshot produced by the background collector, consumed by the UI.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub sessions: Vec<Session>,
    pub stats: ScanStats,
    pub alerts: AlertCounts,
    pub status: String,
    pub generated_ms: i64,
    /// True once the first full scan has completed.
    pub ready: bool,
}

/// Provider totals for the All dashboard.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderTotals {
    pub sessions: usize,
    pub open: usize,
    pub cost: f64,
    pub cost_complete: bool, // false if some sessions had unknown pricing
}

impl Snapshot {
    /// Totals for one provider within the given time cutoff.
    pub fn totals(&self, provider: Provider, cutoff_ms: i64) -> ProviderTotals {
        let mut t = ProviderTotals { cost_complete: true, ..Default::default() };
        for s in &self.sessions {
            if s.provider != provider || s.last_activity_ms < cutoff_ms {
                continue;
            }
            t.sessions += 1;
            if s.state.is_open() {
                t.open += 1;
            }
            match s.cost {
                Some(c) => t.cost += c,
                None => t.cost_complete = false,
            }
        }
        t
    }
}

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

    #[test]
    fn filter_cycles() {
        assert_eq!(TimeFilter::Today.next(), TimeFilter::Last7);
        assert_eq!(TimeFilter::All.next(), TimeFilter::Today);
    }
}
