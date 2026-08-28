use crate::adapters::providers::{registry, ProviderExecutables};
use crate::adapters::system::validate_executable;
use crate::application::managed_session::{ManagedProviderRuntime, ProviderLaunchCommand};
use crate::domain::{ManagedLaunchTarget, Provider};
use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static REGISTRATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl ManagedProviderRuntime for ProviderExecutables {
    fn validate_target(&self, target: &ManagedLaunchTarget) -> Result<()> {
        if !target.working_directory.is_absolute() || !target.working_directory.is_dir() {
            return Err(anyhow!("New target working directory is unavailable"));
        }
        std::fs::read_dir(&target.working_directory)
            .context("New target working directory is unreadable")?;
        Ok(())
    }

    fn new_registration_id(&self, _now_ms: i64) -> String {
        let sequence = REGISTRATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        format!("launch-{:x}-{:x}-{:x}", nonce, std::process::id(), sequence)
    }

    fn new_session_command(
        &self,
        provider: Provider,
        binding_initializer: bool,
    ) -> Result<ProviderLaunchCommand> {
        let executable = executable_for(self, provider)?;
        Ok(ProviderLaunchCommand {
            executable,
            arguments: registry()
                .runtime_control(provider)?
                .new_session_args(binding_initializer),
        })
    }

    fn resume_session_command(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<ProviderLaunchCommand> {
        let executable = executable_for(self, provider)?;
        Ok(ProviderLaunchCommand {
            executable,
            arguments: registry()
                .runtime_control(provider)?
                .resume_session_args(native_session_id),
        })
    }
}

fn executable_for(executables: &ProviderExecutables, provider: Provider) -> Result<PathBuf> {
    let executable = executables
        .executable_for(provider)
        .ok_or_else(|| anyhow!("{} executable is unavailable", provider.label()))?
        .to_path_buf();
    validate_executable(&executable, provider.label())?;
    Ok(executable)
}
