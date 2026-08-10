//! Validated Successor Launch Plans and safe command rendering.

use crate::domain::Provider;
use crate::tasks::{OperationTask, OperationTaskState, SuccessorLaunchPlan};
use anyhow::{anyhow, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub(crate) fn build_launch_plan(
    task: &OperationTask,
    artifact_path: PathBuf,
) -> Result<SuccessorLaunchPlan> {
    Ok(SuccessorLaunchPlan {
        successor_provider: task.successor_provider,
        working_directory: task.source_working_directory.clone(),
        executable: task.successor_executable.clone(),
        arguments: expected_arguments(task.successor_provider, &artifact_path)?,
        artifact_path,
    })
}

pub fn validate_successor_launch_plan(task: &OperationTask) -> Result<&SuccessorLaunchPlan> {
    if task.is_being_deleted() {
        return Err(anyhow!("Operation Task is being deleted"));
    }
    if task.state != OperationTaskState::Ready {
        return Err(anyhow!("Operation Task is not Ready"));
    }
    let artifact_path = task
        .artifact_path
        .as_ref()
        .ok_or_else(|| anyhow!("Ready Task has no Handoff Artifact"))?;
    let plan = task
        .successor_launch_plan
        .as_ref()
        .ok_or_else(|| anyhow!("Ready Task has no Successor Launch Plan"))?;
    if plan.successor_provider != task.successor_provider
        || plan.working_directory != task.source_working_directory
        || plan.executable != task.successor_executable
        || &plan.artifact_path != artifact_path
    {
        return Err(anyhow!(
            "Successor Launch Plan does not match its Operation Task"
        ));
    }
    if !plan.working_directory.is_absolute() || !plan.working_directory.is_dir() {
        return Err(anyhow!("Successor working directory is invalid"));
    }
    if !plan.executable.is_absolute() || !is_executable(&plan.executable) {
        return Err(anyhow!("Successor executable is invalid"));
    }
    if !plan.artifact_path.is_absolute() || !plan.artifact_path.is_file() {
        return Err(anyhow!("Handoff Artifact reference is invalid"));
    }
    let artifact = std::fs::read_to_string(&plan.artifact_path)
        .map_err(|_| anyhow!("Handoff Artifact is unreadable"))?;
    crate::handoff_artifact::validate(&artifact)?;
    if plan.arguments != expected_arguments(plan.successor_provider, &plan.artifact_path)? {
        return Err(anyhow!(
            "Successor arguments do not match the Handoff Artifact"
        ));
    }
    if plan
        .arguments
        .iter()
        .any(|argument| argument.contains('\0'))
    {
        return Err(anyhow!("Successor argument contains NUL"));
    }
    Ok(plan)
}

pub fn render_successor_command(task: &OperationTask) -> Result<String> {
    let plan = validate_successor_launch_plan(task)?;
    render_process_command(
        &plan.working_directory,
        &plan.executable,
        plan.arguments.iter().map(String::as_str),
    )
}

/// Run the foreground Successor helper when present in process arguments.
///
/// Returns false for a normal TUI or detached Handoff Worker invocation.
pub fn run_successor_from_args<I, S>(args: I) -> Result<bool>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    run_successor_from_args_with_config(args, &crate::config::Config::load())
}

pub fn run_successor_from_args_with_config<I, S>(
    args: I,
    config: &crate::config::Config,
) -> Result<bool>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let _program = args.next();
    if args.next().as_deref() != Some(OsStr::new("--launch-successor")) {
        return Ok(false);
    }
    let task_store_path = required_arg(&mut args, "--task-store")?;
    let task_id = required_arg(&mut args, "--task-id")?
        .into_string()
        .map_err(|_| anyhow!("Task ID is not valid UTF-8"))?;
    if args.next().is_some() {
        return Err(anyhow!("unexpected Successor launch helper arguments"));
    }
    let store = crate::tasks::TaskStore::open(Path::new(&task_store_path))?;
    let task = store
        .get(&task_id)?
        .ok_or_else(|| anyhow!("Operation Task is unavailable"))?;
    let plan = validate_successor_launch_plan(&task)?;
    let child = Command::new(&plan.executable)
        .args(&plan.arguments)
        .current_dir(&plan.working_directory)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(_) => {
            store.record_terminal_launch_failed(
                &task.id,
                chrono::Utc::now().timestamp_millis(),
                "successor_provider_start_failed",
            )?;
            return Err(anyhow!("Successor Provider launch failed"));
        }
    };
    let Some(process) = crate::providers::state::process_identity_after_spawn(child.id()) else {
        store.record_terminal_launch_failed(
            &task.id,
            chrono::Utc::now().timestamp_millis(),
            "successor_process_identity_unavailable",
        )?;
        let _ = child.wait();
        return Err(anyhow!("Successor Provider launch evidence is unavailable"));
    };
    store.record_successor_started(&task.id, chrono::Utc::now().timestamp_millis(), process)?;
    let mut linked = false;
    let status = loop {
        if !linked {
            if let Some(session_id) = crate::providers::registry()
                .runtime_control(task.successor_provider)?
                .resolve_successor_session(config, process)
            {
                linked = store
                    .record_successor_linked(
                        &task.id,
                        chrono::Utc::now().timestamp_millis(),
                        process,
                        &session_id,
                    )
                    .is_ok();
            }
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    store.record_successor_exited(
        &task.id,
        chrono::Utc::now().timestamp_millis(),
        status.code(),
    )?;
    if !status.success() {
        return Err(anyhow!("Successor Provider exited with status {status}"));
    }
    Ok(true)
}

pub trait TerminalLauncher {
    fn open(&mut self, task_id: &str, now_ms: i64) -> Result<()>;
}

pub struct MacosTerminalLauncher {
    helper_executable: PathBuf,
    task_store_path: PathBuf,
    osascript_executable: PathBuf,
}

impl MacosTerminalLauncher {
    pub fn new(helper_executable: PathBuf, task_store_path: PathBuf) -> Self {
        Self::with_osascript(
            helper_executable,
            task_store_path,
            PathBuf::from("/usr/bin/osascript"),
        )
    }

    pub fn with_osascript(
        helper_executable: PathBuf,
        task_store_path: PathBuf,
        osascript_executable: PathBuf,
    ) -> Self {
        Self {
            helper_executable,
            task_store_path,
            osascript_executable,
        }
    }
}

impl TerminalLauncher for MacosTerminalLauncher {
    fn open(&mut self, task_id: &str, now_ms: i64) -> Result<()> {
        let store = crate::tasks::TaskStore::open(&self.task_store_path)?;
        let task = store
            .get(task_id)?
            .ok_or_else(|| anyhow!("Operation Task is unavailable"))?;
        validate_successor_launch_plan(&task)?;
        if !self.helper_executable.is_absolute() || !is_executable(&self.helper_executable) {
            return Err(anyhow!("SessionMesh launch helper is invalid"));
        }
        let task_store = path_text(&self.task_store_path, "Task Store path")?;
        let command = render_process_command(
            &task.source_working_directory,
            &self.helper_executable,
            [
                "--launch-successor",
                "--task-store",
                task_store,
                "--task-id",
                &task.id,
            ],
        )?;
        let status = Command::new(&self.osascript_executable)
            .args([
                OsStr::new("-e"),
                OsStr::new("on run argv"),
                OsStr::new("-e"),
                OsStr::new("tell application \"Terminal\""),
                OsStr::new("-e"),
                OsStr::new("do script (item 1 of argv)"),
                OsStr::new("-e"),
                OsStr::new("activate"),
                OsStr::new("-e"),
                OsStr::new("end tell"),
                OsStr::new("-e"),
                OsStr::new("end run"),
                OsStr::new("--"),
                OsStr::new(&command),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match status {
            Ok(status) if status.success() => {
                store.record_terminal_launch_succeeded(task_id, now_ms)?;
                Ok(())
            }
            Ok(_) | Err(_) => {
                store.record_terminal_launch_failed(
                    task_id,
                    now_ms,
                    "terminal_app_launch_failed",
                )?;
                Err(anyhow!("Terminal.app launch failed"))
            }
        }
    }
}

fn expected_arguments(provider: Provider, artifact_path: &Path) -> Result<Vec<String>> {
    Ok(crate::providers::registry()
        .runtime_control(provider)?
        .successor_launch_arguments(artifact_path))
}

fn required_arg(args: &mut impl Iterator<Item = OsString>, expected: &str) -> Result<OsString> {
    let flag = args
        .next()
        .ok_or_else(|| anyhow!("missing Successor launch helper argument {expected}"))?;
    if flag != OsStr::new(expected) {
        return Err(anyhow!(
            "expected Successor launch helper argument {expected}, got {flag:?}"
        ));
    }
    args.next()
        .ok_or_else(|| anyhow!("missing value for Successor launch helper argument {expected}"))
}

fn path_text<'a>(path: &'a Path, field: &str) -> Result<&'a str> {
    path.to_str()
        .ok_or_else(|| anyhow!("Successor {field} is not valid UTF-8"))
}

fn render_process_command<'a>(
    working_directory: &Path,
    executable: &Path,
    arguments: impl IntoIterator<Item = &'a str>,
) -> Result<String> {
    let working_directory = path_text(working_directory, "working directory")?;
    let executable = path_text(executable, "executable")?;
    let mut command = format!(
        "cd -- {} && {}",
        shell_quote(working_directory),
        shell_quote(executable)
    );
    for argument in arguments {
        command.push(' ');
        command.push_str(&shell_quote(argument));
    }
    Ok(command)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
