//! External capability ports used to launch and access Managed runs.

use crate::domain::{
    ManagedControlChannel, ManagedLaunchEvidence, ManagedLaunchRegistration, ManagedLaunchTarget,
    Provider,
};
use anyhow::{anyhow, Result};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderLaunchCommand {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
}

pub trait ManagedProviderRuntime: Send + Sync {
    fn validate_target(&self, target: &ManagedLaunchTarget) -> Result<()>;
    fn new_registration_id(&self, now_ms: i64) -> String;
    fn new_session_command(
        &self,
        provider: Provider,
        binding_initializer: bool,
    ) -> Result<ProviderLaunchCommand>;
    fn resume_session_command(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<ProviderLaunchCommand>;
}

pub trait ManagedLaunchBroker: Send {
    fn control_channel(&self, registration_id: &str) -> Result<ManagedControlChannel> {
        Ok(ManagedControlChannel {
            namespace: format!("mudi-{registration_id}"),
            run_target: "managed".to_string(),
        })
    }

    fn launch(
        &mut self,
        registration: &ManagedLaunchRegistration,
        command: &ProviderLaunchCommand,
    ) -> Result<ManagedLaunchEvidence>;

    fn cancel(&mut self, _registration: &ManagedLaunchRegistration) -> Result<()> {
        Err(anyhow!("Managed launch cancellation is unavailable"))
    }
}

pub trait ManagedTerminalAccess: Send {
    fn open(&mut self, registration: &ManagedLaunchRegistration) -> Result<()>;
}
