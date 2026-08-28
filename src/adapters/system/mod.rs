//! Process, tmux, and Terminal.app system integrations.

pub mod process;
pub mod terminal_macos;
pub mod tmux;

use anyhow::{anyhow, Context, Result};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub(crate) fn validate_executable(path: &Path, name: &str) -> Result<()> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("{name} executable is unavailable"))?;
    if !path.is_absolute() || !metadata.is_file() {
        return Err(anyhow!("{name} executable is unavailable"));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(anyhow!("{name} executable is unavailable"));
    }
    Ok(())
}
