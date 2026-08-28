//! Private tmux construction, launch, cleanup, and executable discovery.

use super::validate_executable;
use crate::adapters::config::configured_path;
use crate::adapters::providers::state;
use crate::application::managed_session::{
    ManagedLaunchBroker, ManagedLaunchCleanup, ProviderLaunchCommand,
};
use crate::domain::{ManagedControlChannel, ManagedLaunchEvidence, ManagedLaunchRegistration};
use anyhow::{anyhow, Context, Result};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct TmuxManagedLaunchCleanup {
    tmux_executable: PathBuf,
}

impl TmuxManagedLaunchCleanup {
    pub fn new(tmux_executable: PathBuf) -> Self {
        Self { tmux_executable }
    }
}

impl ManagedLaunchCleanup for TmuxManagedLaunchCleanup {
    fn clean_empty(&self, registration: &ManagedLaunchRegistration) -> Result<()> {
        clean_empty_private_tmux(&self.tmux_executable, &registration.control_channel)
    }
}

pub struct TmuxLaunchBroker {
    tmux_executable: PathBuf,
}

impl TmuxLaunchBroker {
    pub fn new(tmux_executable: PathBuf) -> Self {
        Self { tmux_executable }
    }
}

impl ManagedLaunchBroker for TmuxLaunchBroker {
    fn launch(
        &mut self,
        registration: &ManagedLaunchRegistration,
        provider_command: &ProviderLaunchCommand,
    ) -> Result<ManagedLaunchEvidence> {
        validate_executable(&self.tmux_executable, "tmux")?;
        validate_executable(&provider_command.executable, registration.provider.label())?;
        validate_control_channel(&registration.control_channel)?;
        let terminal_title = escape_tmux_format(&registration.session_alias);
        let window_title = tmux_window_title(
            &registration.session_alias,
            registration.description.as_deref(),
        );
        let output = Command::new(&self.tmux_executable)
            .env_remove("TMUX")
            .args([OsStr::new("-f"), OsStr::new("/dev/null"), OsStr::new("-L")])
            .arg(&registration.control_channel.namespace)
            .args([
                OsStr::new("new-session"),
                OsStr::new("-d"),
                OsStr::new("-P"),
                OsStr::new("-F"),
                OsStr::new("#{pane_id}\t#{pane_pid}"),
                OsStr::new("-s"),
            ])
            .arg(&registration.control_channel.run_target)
            .arg("-n")
            .arg(window_title)
            .arg("-c")
            .arg(&registration.target.working_directory)
            .arg("--")
            .arg(&provider_command.executable)
            .args(&provider_command.arguments)
            .arg(";")
            .args(["set-option", "-w", "automatic-rename", "off"])
            .arg(";")
            .args(["set-option", "-g", "set-titles", "on"])
            .arg(";")
            .args(["set-option", "-g", "set-titles-string"])
            .arg(&terminal_title)
            .arg(";")
            .args(["set-option", "-g", "mouse", "on"])
            .arg(";")
            .args(["set-option", "-s", "copy-command", "/usr/bin/pbcopy"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .context("could not start private tmux")?;
        if !output.status.success() {
            return Err(anyhow!("private tmux launch exited with {}", output.status));
        }
        let output = String::from_utf8(output.stdout)
            .map_err(|_| anyhow!("private tmux launch evidence is not valid UTF-8"))?;
        let (channel_id, pane_pid) = output
            .trim()
            .split_once('\t')
            .ok_or_else(|| anyhow!("private tmux launch evidence is incomplete"))?;
        validate_tmux_pane_id(channel_id)?;
        let pane_pid = pane_pid
            .parse::<u32>()
            .map_err(|_| anyhow!("private tmux Provider PID is invalid"))?;
        let provider_process = state::process_identity_after_spawn(pane_pid)
            .ok_or_else(|| anyhow!("private tmux Provider process identity is unavailable"))?;
        Ok(ManagedLaunchEvidence {
            channel_id: channel_id.to_string(),
            provider_process,
        })
    }

    fn cancel(&mut self, registration: &ManagedLaunchRegistration) -> Result<()> {
        terminate_private_tmux(&self.tmux_executable, &registration.control_channel)
    }
}

pub fn discover_tmux_executable() -> Option<PathBuf> {
    configured_path(["MUDI_TMUX_EXECUTABLE", "SESSIONMESH_TMUX_EXECUTABLE"])
        .filter(|path| validate_executable(path, "tmux").is_ok())
        .or_else(|| {
            std::env::var_os("PATH")
                .as_deref()
                .map(std::env::split_paths)
                .into_iter()
                .flatten()
                .map(|directory| directory.join("tmux"))
                .find(|path| validate_executable(path, "tmux").is_ok())
        })
}

fn terminate_private_tmux(
    tmux_executable: &Path,
    environment: &ManagedControlChannel,
) -> Result<()> {
    validate_executable(tmux_executable, "tmux")?;
    validate_control_channel(environment)?;
    let status = Command::new(tmux_executable)
        .env_remove("TMUX")
        .args([OsStr::new("-f"), OsStr::new("/dev/null"), OsStr::new("-L")])
        .arg(&environment.namespace)
        .arg("kill-server")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("could not clean private tmux")?;
    if status.success() || status.code() == Some(1) {
        Ok(())
    } else {
        Err(anyhow!("private tmux cleanup failed"))
    }
}

fn clean_empty_private_tmux(
    tmux_executable: &Path,
    environment: &ManagedControlChannel,
) -> Result<()> {
    validate_executable(tmux_executable, "tmux")?;
    validate_control_channel(environment)?;
    let probe = Command::new(tmux_executable)
        .env_remove("TMUX")
        .args([OsStr::new("-f"), OsStr::new("/dev/null"), OsStr::new("-L")])
        .arg(&environment.namespace)
        .args([OsStr::new("has-session"), OsStr::new("-t")])
        .arg(&environment.run_target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("could not inspect private tmux")?;
    if probe.code() == Some(1) {
        Ok(())
    } else if probe.success() {
        Err(anyhow!("private tmux environment is not empty"))
    } else {
        Err(anyhow!("private tmux environment could not be inspected"))
    }
}

fn escape_tmux_format(value: &str) -> String {
    value.replace('#', "##")
}

fn tmux_window_title(title: &str, description: Option<&str>) -> String {
    escape_tmux_format(description.unwrap_or(title))
}

pub(crate) fn validate_control_channel(channel: &ManagedControlChannel) -> Result<()> {
    if channel.is_valid() {
        Ok(())
    } else {
        Err(anyhow!("private tmux environment is invalid"))
    }
}

fn validate_tmux_pane_id(value: &str) -> Result<()> {
    if value.strip_prefix('%').is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        Ok(())
    } else {
        Err(anyhow!("private tmux pane identity is invalid"))
    }
}

#[cfg(test)]
mod tests {
    use super::tmux_window_title;

    #[test]
    fn tmux_window_title_falls_back_to_the_session_title() {
        assert_eq!(tmux_window_title("Title #S", None), "Title ##S".to_string());
    }
}
