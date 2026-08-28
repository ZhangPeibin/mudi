//! Terminal.app access to private Managed control channels.

use super::tmux::validate_control_channel;
use super::validate_executable;
use crate::application::managed_session::{
    ManagedCurrentRunTerminalAccess, ManagedTerminalAccess, VerifiedManagedCurrentRun,
};
use crate::domain::{ManagedControlChannel, ManagedLaunchRegistration};
use anyhow::{anyhow, Context, Result};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub struct MacosManagedTerminalAccess {
    tmux_executable: PathBuf,
    osascript_executable: PathBuf,
}

impl MacosManagedTerminalAccess {
    pub fn new(tmux_executable: PathBuf) -> Self {
        Self::with_osascript(tmux_executable, PathBuf::from("/usr/bin/osascript"))
    }

    pub fn with_osascript(tmux_executable: PathBuf, osascript_executable: PathBuf) -> Self {
        Self {
            tmux_executable,
            osascript_executable,
        }
    }

    fn open_private_control_channel(&self, channel: &ManagedControlChannel) -> Result<()> {
        validate_executable(&self.tmux_executable, "tmux")?;
        validate_control_channel(channel)?;
        let command = [
            shell_quote_os(self.tmux_executable.as_os_str())?,
            "-L".to_string(),
            shell_quote(&channel.namespace),
            "attach-session".to_string(),
            "-t".to_string(),
            shell_quote(&channel.run_target),
        ]
        .join(" ");
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
            .status()
            .context("could not open Terminal.app")?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow!("Terminal.app launch failed"))
        }
    }
}

impl ManagedTerminalAccess for MacosManagedTerminalAccess {
    fn open(&mut self, registration: &ManagedLaunchRegistration) -> Result<()> {
        self.open_private_control_channel(&registration.control_channel)
    }
}

impl ManagedCurrentRunTerminalAccess for MacosManagedTerminalAccess {
    fn open_terminal(&mut self, run: &VerifiedManagedCurrentRun) -> Result<()> {
        self.open_private_control_channel(&run.control_channel)
    }

    fn attach_foreground(&mut self, run: &VerifiedManagedCurrentRun) -> Result<()> {
        validate_executable(&self.tmux_executable, "tmux")?;
        validate_control_channel(&run.control_channel)?;
        let status = Command::new(&self.tmux_executable)
            .env_remove("TMUX")
            .args([
                OsStr::new("-L"),
                OsStr::new(&run.control_channel.namespace),
                OsStr::new("attach-session"),
                OsStr::new("-t"),
                OsStr::new(&run.control_channel.run_target),
            ])
            .status()
            .context("could not attach to the Managed Current Run")?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow!("Managed Current Run attach failed"))
        }
    }
}

fn shell_quote_os(value: &OsStr) -> Result<String> {
    value
        .to_str()
        .map(shell_quote)
        .ok_or_else(|| anyhow!("tmux executable path is not valid UTF-8"))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
