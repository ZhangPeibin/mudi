//! macOS process-state detection.
//!
//! Claude records an open-session state file per PID; Codex does not, so we detect
//! open Codex sessions by the process holding the rollout file open. All detection is
//! read-only and defends against PID reuse by validating process start time.

use crate::domain::SessionState;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A single scan's worth of live process facts, gathered once per refresh.
pub struct StateDetector {
    /// Claude `sessionId` → live state (Busy/Idle). Absent = exited.
    claude: HashMap<String, SessionState>,
    /// Absolute paths of rollout files currently held open by a Codex process.
    codex_held: HashSet<PathBuf>,
}

impl StateDetector {
    /// Gather live state for both providers.
    pub fn gather(claude_sessions_dir: &Path) -> Self {
        StateDetector {
            claude: detect_claude(claude_sessions_dir),
            codex_held: codex_held_files(),
        }
    }

    /// Live state for a Claude session id. Exited when no valid state file matches.
    pub fn claude_state(&self, session_id: &str) -> SessionState {
        self.claude
            .get(session_id)
            .copied()
            .unwrap_or(SessionState::Exited)
    }

    /// Whether a live Codex process currently holds `rollout_path` open. Only a
    /// held file can be Busy/Idle; everything else is Exited — so the adapter reads
    /// a file's task-boundary tail only when this returns true.
    pub fn holds_codex(&self, rollout_path: &Path) -> bool {
        self.codex_held.contains(rollout_path)
    }

    /// Live state for a Codex session given its rollout path and latest task boundary.
    ///
    /// - held file + unmatched `task_started` → Busy
    /// - held file + completed/aborted turn → Idle
    /// - no holder → Exited
    pub fn codex_state(&self, rollout_path: &Path, open_turn: Option<bool>) -> SessionState {
        if !self.codex_held.contains(rollout_path) {
            return SessionState::Exited;
        }
        match open_turn {
            Some(true) => SessionState::Busy,
            _ => SessionState::Idle,
        }
    }
}

/// Read Claude's per-PID state files, validating each against the live process.
fn detect_claude(sessions_dir: &Path) -> HashMap<String, SessionState> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return out;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };

        let Some(pid) = v.get("pid").and_then(Value::as_i64) else {
            continue;
        };
        let Some(session_id) = v.get("sessionId").and_then(Value::as_str) else {
            continue;
        };
        let started_at_ms = v.get("startedAt").and_then(Value::as_i64);

        // PID-reuse guard: the live process must exist and its start time must match
        // the recorded `startedAt`. We validate via elapsed time (`ps -o etime`), which
        // is locale- and timezone-independent (unlike `lstart`).
        if !process_matches(pid, started_at_ms) {
            continue;
        }

        let status = v.get("status").and_then(Value::as_str).unwrap_or("");
        let state = if status == "busy" {
            SessionState::Busy
        } else {
            // Any other open status (idle, shell, …) = the CLI is open, awaiting input.
            SessionState::Idle
        };
        out.insert(session_id.to_string(), state);
    }
    out
}

/// Elapsed-time tolerance (seconds) when matching a process against `startedAt`.
/// A reused PID would show a wildly different start time, so a small window is safe.
const START_TOLERANCE_S: i64 = 120;

/// True if `pid` is alive and (when known) its start time matches `expected_started_ms`.
fn process_matches(pid: i64, expected_started_ms: Option<i64>) -> bool {
    let output = Command::new("ps")
        .args(["-o", "etime=", "-p", &pid.to_string()])
        .output();
    let Ok(output) = output else { return false };
    if !output.status.success() {
        return false;
    }
    let etime = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if etime.is_empty() {
        return false; // process not alive
    }
    let Some(elapsed_s) = parse_etime(&etime) else {
        return false;
    };

    match expected_started_ms {
        Some(exp_ms) => {
            let now_s = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let actual_start_s = now_s - elapsed_s;
            (actual_start_s - exp_ms / 1000).abs() <= START_TOLERANCE_S
        }
        // No recorded start time: fall back to liveness only.
        None => true,
    }
}

/// Parse `ps -o etime` output (`[[DD-]HH:]MM:SS`) into elapsed seconds.
fn parse_etime(s: &str) -> Option<i64> {
    let (days, hms) = match s.split_once('-') {
        Some((d, rest)) => (d.parse::<i64>().ok()?, rest),
        None => (0, s),
    };
    let parts: Vec<i64> = hms
        .split(':')
        .map(|p| p.parse::<i64>().ok())
        .collect::<Option<_>>()?;
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (*h, *m, *s),
        [m, s] => (0, *m, *s),
        _ => return None,
    };
    Some(days * 86_400 + h * 3_600 + m * 60 + sec)
}

/// Set of rollout files currently held open by a live `codex` process.
fn codex_held_files() -> HashSet<PathBuf> {
    let mut held = HashSet::new();

    // Find codex PIDs. No match → nothing is held.
    let pids = match Command::new("pgrep").arg("codex").output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        _ => return held,
    };
    if pids.is_empty() {
        return held;
    }

    // List open files for those PIDs; keep rollout JSONLs.
    let output = Command::new("lsof")
        .arg("-p")
        .arg(pids.join(","))
        .arg("-Fn")
        .output();
    let Ok(output) = output else { return held };

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // `-Fn` prints file paths on lines prefixed with 'n'.
        if let Some(name) = line.strip_prefix('n') {
            if name.contains("/sessions/") && name.ends_with(".jsonl") {
                held.insert(PathBuf::from(name));
            }
        }
    }
    held
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_state_maps_boundary_when_held() {
        let d = StateDetector {
            claude: HashMap::new(),
            codex_held: HashSet::from([PathBuf::from("/x/sessions/a.jsonl")]),
        };
        let held = Path::new("/x/sessions/a.jsonl");
        assert_eq!(d.codex_state(held, Some(true)), SessionState::Busy);
        assert_eq!(d.codex_state(held, Some(false)), SessionState::Idle);
        assert_eq!(d.codex_state(held, None), SessionState::Idle);
        // Not held → exited regardless of boundary.
        let other = Path::new("/x/sessions/b.jsonl");
        assert_eq!(d.codex_state(other, Some(true)), SessionState::Exited);
    }

    #[test]
    fn claude_state_defaults_to_exited() {
        let d = StateDetector {
            claude: HashMap::new(),
            codex_held: HashSet::new(),
        };
        assert_eq!(d.claude_state("nope"), SessionState::Exited);
    }

    #[test]
    fn parses_etime_formats() {
        assert_eq!(parse_etime("57:43"), Some(57 * 60 + 43));
        assert_eq!(parse_etime("12:34:56"), Some(12 * 3600 + 34 * 60 + 56));
        assert_eq!(
            parse_etime("13-06:15:11"),
            Some(13 * 86400 + 6 * 3600 + 15 * 60 + 11)
        );
        assert_eq!(parse_etime("garbage"), None);
    }
}
