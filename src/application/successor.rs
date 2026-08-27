//! Validated Successor Launch Plans and safe command rendering.

use crate::application::tasks::{OperationTask, OperationTaskState, SuccessorLaunchPlan};
use crate::domain::Provider;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

/// Provider-native Successor argument construction.
pub trait SuccessorRuntime: Send + Sync {
    fn launch_arguments(&self, provider: Provider, artifact_path: &Path) -> Result<Vec<String>>;
    fn validate_environment(&self, plan: &SuccessorLaunchPlan) -> Result<()>;
}

pub(crate) fn build_launch_plan(
    task: &OperationTask,
    artifact_path: PathBuf,
    runtime: &dyn SuccessorRuntime,
) -> Result<SuccessorLaunchPlan> {
    Ok(SuccessorLaunchPlan {
        successor_provider: task.successor_provider,
        working_directory: task.source_working_directory.clone(),
        executable: task.successor_executable.clone(),
        arguments: runtime.launch_arguments(task.successor_provider, &artifact_path)?,
        artifact_path,
    })
}

pub fn validate_successor_launch_plan<'a>(
    task: &'a OperationTask,
    runtime: &dyn SuccessorRuntime,
) -> Result<&'a SuccessorLaunchPlan> {
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
    runtime.validate_environment(plan)?;
    if plan.arguments != runtime.launch_arguments(plan.successor_provider, &plan.artifact_path)? {
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

pub fn render_successor_command(
    task: &OperationTask,
    runtime: &dyn SuccessorRuntime,
) -> Result<String> {
    let plan = validate_successor_launch_plan(task, runtime)?;
    render_process_command(
        &plan.working_directory,
        &plan.executable,
        plan.arguments.iter().map(String::as_str),
    )
}

pub trait TerminalLauncher {
    fn open(&mut self, task_id: &str, now_ms: i64) -> Result<()>;
}

pub(crate) fn render_process_command<'a>(
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

pub(crate) fn path_text<'a>(path: &'a Path, field: &str) -> Result<&'a str> {
    path.to_str()
        .ok_or_else(|| anyhow!("Successor {field} is not valid UTF-8"))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
