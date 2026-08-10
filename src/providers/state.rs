//! macOS process-state detection.
//!
//! Claude records an open-session state file per PID; Codex does not, so we detect
//! open Codex sessions by the process holding the rollout file open. All detection is
//! read-only and defends against PID reuse by validating process start time.

use crate::domain::SessionState;
use chrono::{Local, NaiveDateTime, TimeZone};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A single scan's worth of live process facts, gathered once per refresh.
pub struct StateDetector {
    /// Claude `sessionId` → validated live state. Absent = exited.
    claude: HashMap<String, LiveState>,
    /// Absolute rollout paths currently held by Codex, with the holder's
    /// validated process start when it is unambiguous.
    codex_held: HashMap<PathBuf, Option<ProcessIdentity>>,
}

/// Exact Provider CLI process evidence used to defend control actions from PID reuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub started_at_ms: i64,
}

pub(crate) enum ProcessInspection {
    Present(ProcessIdentity, bool),
    Absent,
    Unknown,
}

/// Live lifecycle evidence and the validated start of its Current Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveState {
    pub state: SessionState,
    pub current_run_started_at_ms: Option<i64>,
    pub process_identity: Option<ProcessIdentity>,
}

impl LiveState {
    fn exited() -> Self {
        LiveState {
            state: SessionState::Exited,
            current_run_started_at_ms: None,
            process_identity: None,
        }
    }
}

impl StateDetector {
    /// Gather live state for both providers.
    pub fn gather(claude_sessions_dir: &Path) -> Self {
        StateDetector {
            claude: detect_claude(claude_sessions_dir),
            codex_held: codex_live_files(),
        }
    }

    /// Live state for a Claude session id. Exited when no valid state file matches.
    pub fn claude_state(&self, session_id: &str) -> LiveState {
        self.claude
            .get(session_id)
            .copied()
            .unwrap_or_else(LiveState::exited)
    }

    /// Whether a live Codex process currently holds `rollout_path` open. Only a
    /// held file can be Busy/Idle; everything else is Exited — so the adapter reads
    /// a file's task-boundary tail only when this returns true.
    pub fn holds_codex(&self, rollout_path: &Path) -> bool {
        self.codex_held.contains_key(rollout_path)
    }

    /// Live state for a Codex session given its rollout path and latest task boundary.
    ///
    /// - held file + unmatched `task_started` → Busy
    /// - held file + completed/aborted turn → Idle
    /// - no holder → Exited
    pub fn codex_state(&self, rollout_path: &Path, open_turn: Option<bool>) -> LiveState {
        let Some(started_at_ms) = self.codex_held.get(rollout_path) else {
            return LiveState::exited();
        };
        let state = match open_turn {
            Some(true) => SessionState::Busy,
            _ => SessionState::Idle,
        };
        LiveState {
            state,
            current_run_started_at_ms: started_at_ms.map(|identity| identity.started_at_ms),
            process_identity: *started_at_ms,
        }
    }
}

/// Read Claude's per-PID state files, validating each against the live process.
fn detect_claude(sessions_dir: &Path) -> HashMap<String, LiveState> {
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
        out.insert(
            session_id.to_string(),
            LiveState {
                state,
                current_run_started_at_ms: started_at_ms,
                process_identity: u32::try_from(pid)
                    .ok()
                    .zip(started_at_ms)
                    .map(|(pid, started_at_ms)| ProcessIdentity { pid, started_at_ms }),
            },
        );
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

pub(crate) fn process_identity_is_live(identity: ProcessIdentity) -> bool {
    process_identity(identity.pid)
        .is_some_and(|live| (live.started_at_ms - identity.started_at_ms).abs() <= 2_000)
}

fn observe_process_identity(pid: u32) -> Option<(ProcessIdentity, bool)> {
    match inspect_process_identity(pid) {
        ProcessInspection::Present(identity, is_zombie) => Some((identity, is_zombie)),
        ProcessInspection::Absent | ProcessInspection::Unknown => None,
    }
}

pub(crate) fn inspect_process_identity(pid: u32) -> ProcessInspection {
    let output = match Command::new("ps")
        .args(["-o", "stat=", "-o", "lstart=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .output()
    {
        Ok(output) => output,
        Err(_) => return ProcessInspection::Unknown,
    };
    if !output.status.success() {
        return if output.status.code() == Some(1) {
            ProcessInspection::Absent
        } else {
            ProcessInspection::Unknown
        };
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut fields = stdout.trim().splitn(2, char::is_whitespace);
    let Some(state) = fields.next().filter(|state| !state.is_empty()) else {
        return ProcessInspection::Absent;
    };
    let Some(started_at_ms) = fields.next().and_then(|value| parse_lstart(value.trim())) else {
        return ProcessInspection::Unknown;
    };
    ProcessInspection::Present(
        ProcessIdentity { pid, started_at_ms },
        state.starts_with('Z'),
    )
}

pub(crate) fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    observe_process_identity(pid).and_then(|(identity, is_zombie)| (!is_zombie).then_some(identity))
}

pub(crate) fn process_identity_after_spawn(pid: u32) -> Option<ProcessIdentity> {
    for attempt in 0..5 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // Child::id remains reserved until its parent reaps it, so even a
        // zombie's start timestamp is conclusive evidence for this spawn.
        if let Some((identity, _)) = observe_process_identity(pid) {
            return Some(identity);
        }
    }
    None
}

pub(crate) fn files_held_by_process(process: ProcessIdentity) -> Option<Vec<PathBuf>> {
    if !process_identity_is_live(process) {
        return None;
    }
    let output = Command::new("lsof")
        .arg("-p")
        .arg(process.pid.to_string())
        .arg("-Fn")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let files = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix('n'))
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    process_identity_is_live(process).then_some(files)
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

/// Rollout files currently held by Codex, joined to each holder's process start.
fn codex_live_files() -> HashMap<PathBuf, Option<ProcessIdentity>> {
    // Find codex PIDs. No match → nothing is held.
    let pids = match Command::new("pgrep").arg("codex").output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        _ => return HashMap::new(),
    };
    if pids.is_empty() {
        return HashMap::new();
    }

    let ps_output = Command::new("ps")
        .args(["-o", "pid=,lstart=", "-p", &pids.join(",")])
        .env("LC_ALL", "C")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();

    // Keep the PID field so each open rollout can be joined to its holder.
    let output = Command::new("lsof")
        .arg("-p")
        .arg(pids.join(","))
        .arg("-Fpn")
        .output();
    let Ok(output) = output else {
        return HashMap::new();
    };

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    parse_codex_live_files(&String::from_utf8_lossy(&output.stdout), &ps_output, now_ms)
}

fn parse_codex_live_files(
    lsof_output: &str,
    ps_output: &str,
    _now_ms: i64,
) -> HashMap<PathBuf, Option<ProcessIdentity>> {
    let starts: HashMap<&str, i64> = ps_output
        .lines()
        .filter_map(|line| {
            let mut fields = line.trim().splitn(2, char::is_whitespace);
            let pid = fields.next()?;
            let started_at_ms = parse_lstart(fields.next()?.trim())?;
            Some((pid, started_at_ms))
        })
        .collect();

    let mut held = HashMap::new();
    let mut current_pid: Option<&str> = None;
    for line in lsof_output.lines() {
        if let Some(pid) = line.strip_prefix('p') {
            current_pid = Some(pid);
            continue;
        }
        let Some(name) = line.strip_prefix('n') else {
            continue;
        };
        if !name.contains("/sessions/") || !name.ends_with(".jsonl") {
            continue;
        }

        let process_identity = current_pid.and_then(|pid| {
            Some(ProcessIdentity {
                pid: pid.parse().ok()?,
                started_at_ms: starts.get(pid).copied()?,
            })
        });
        held.entry(PathBuf::from(name))
            .and_modify(|existing| {
                if *existing != process_identity {
                    *existing = None;
                }
            })
            .or_insert(process_identity);
    }
    held
}

fn parse_lstart(value: &str) -> Option<i64> {
    let local = NaiveDateTime::parse_from_str(value, "%a %b %e %H:%M:%S %Y").ok()?;
    Local
        .from_local_datetime(&local)
        .single()
        .map(|started| started.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_state_maps_boundary_when_held() {
        let d = StateDetector {
            claude: HashMap::new(),
            codex_held: HashMap::from([(
                PathBuf::from("/x/sessions/a.jsonl"),
                Some(ProcessIdentity {
                    pid: 42,
                    started_at_ms: 1_234,
                }),
            )]),
        };
        let held = Path::new("/x/sessions/a.jsonl");
        assert_eq!(d.codex_state(held, Some(true)).state, SessionState::Busy);
        assert_eq!(d.codex_state(held, Some(false)).state, SessionState::Idle);
        assert_eq!(d.codex_state(held, None).state, SessionState::Idle);
        // Not held → exited regardless of boundary.
        let other = Path::new("/x/sessions/b.jsonl");
        assert_eq!(d.codex_state(other, Some(true)).state, SessionState::Exited);
    }

    #[test]
    fn missing_codex_rollout_holder_has_no_controllable_current_run() {
        let detector = StateDetector {
            claude: HashMap::new(),
            codex_held: HashMap::new(),
        };

        let live = detector.codex_state(Path::new("/x/sessions/not-held.jsonl"), Some(true));

        assert_eq!(live.state, SessionState::Exited);
        assert_eq!(live.process_identity, None);
    }

    #[test]
    fn claude_state_defaults_to_exited() {
        let d = StateDetector {
            claude: HashMap::new(),
            codex_held: HashMap::new(),
        };
        assert_eq!(d.claude_state("nope").state, SessionState::Exited);
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

    #[test]
    fn claude_live_state_keeps_the_validated_process_start_as_run_boundary() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let dir = std::env::temp_dir().join(format!(
            "sessionmesh-claude-live-boundary-{}-{now_ms}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("state.json"),
            format!(
                "{{\"pid\":{},\"sessionId\":\"live-session\",\"status\":\"busy\",\"startedAt\":{now_ms}}}",
                std::process::id()
            ),
        )
        .unwrap();

        let detector = StateDetector::gather(&dir);
        let live = detector.claude_state("live-session");

        assert_eq!(live.state, SessionState::Busy);
        assert_eq!(live.current_run_started_at_ms, Some(now_ms));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn claude_live_state_without_started_at_has_no_run_boundary() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let dir = std::env::temp_dir().join(format!(
            "sessionmesh-claude-missing-boundary-{}-{now_ms}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("state.json"),
            format!(
                "{{\"pid\":{},\"sessionId\":\"live-session\",\"status\":\"busy\"}}",
                std::process::id()
            ),
        )
        .unwrap();

        let detector = StateDetector::gather(&dir);
        let live = detector.claude_state("live-session");

        assert_eq!(live.state, SessionState::Busy);
        assert_eq!(live.current_run_started_at_ms, None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_claude_pid_state_is_not_treated_as_an_open_current_run() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let dir = std::env::temp_dir().join(format!(
            "sessionmesh-claude-stale-state-{}-{now_ms}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("state.json"),
            format!(
                "{{\"pid\":{},\"sessionId\":\"stale-session\",\"status\":\"idle\",\"startedAt\":0}}",
                std::process::id()
            ),
        )
        .unwrap();

        let live = StateDetector::gather(&dir).claude_state("stale-session");

        assert_eq!(live.state, SessionState::Exited);
        assert_eq!(live.process_identity, None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_live_file_uses_the_holding_process_start_as_run_boundary() {
        let now_ms = 1_800_000;
        let ps = "42 Mon Aug  3 12:34:56 2026\n";
        let lsof = "p42\nn/tmp/codex/sessions/rollout-live.jsonl\n";

        let live = parse_codex_live_files(lsof, ps, now_ms);

        assert_eq!(
            live.get(Path::new("/tmp/codex/sessions/rollout-live.jsonl")),
            Some(&Some(ProcessIdentity {
                pid: 42,
                started_at_ms: Local
                    .with_ymd_and_hms(2026, 8, 3, 12, 34, 56)
                    .single()
                    .unwrap()
                    .timestamp_millis(),
            }))
        );
    }

    #[test]
    fn codex_process_identity_is_stable_across_rechecks() {
        let ps = "42 Mon Aug  3 12:34:56 2026\n";
        let lsof = "p42\nn/tmp/codex/sessions/rollout-live.jsonl\n";

        let first = parse_codex_live_files(lsof, ps, 1_800_123);
        let second = parse_codex_live_files(lsof, ps, 9_999_987);

        assert_eq!(
            first.get(Path::new("/tmp/codex/sessions/rollout-live.jsonl")),
            second.get(Path::new("/tmp/codex/sessions/rollout-live.jsonl"))
        );
        assert!(matches!(
            first.get(Path::new("/tmp/codex/sessions/rollout-live.jsonl")),
            Some(Some(ProcessIdentity { pid: 42, .. }))
        ));
    }

    #[test]
    fn codex_task_boundary_does_not_substitute_for_a_missing_process_start() {
        let path = PathBuf::from("/tmp/codex/sessions/rollout-live.jsonl");
        let held = parse_codex_live_files(
            "p42\nn/tmp/codex/sessions/rollout-live.jsonl\n",
            "",
            1_800_000,
        );
        let detector = StateDetector {
            claude: HashMap::new(),
            codex_held: held,
        };

        let live = detector.codex_state(&path, Some(true));

        assert_eq!(live.state, SessionState::Busy);
        assert_eq!(live.current_run_started_at_ms, None);
    }
}
