//! Explicitly authorized Managed New workflow.

use super::{
    launch::launch_and_open, ManagedLaunchBroker, ManagedProviderRuntime, ManagedSessionRepository,
    ManagedTerminalAccess,
};
use crate::application::session_labels::{validate_alias, validate_description};
use crate::domain::{
    ManagedLaunchRegistration, ManagedLaunchStatus, ManagedLaunchTarget, Provider,
};
use anyhow::{anyhow, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedNewRequest {
    pub provider: Provider,
    pub target: ManagedLaunchTarget,
    pub title: String,
    pub description: Option<String>,
    pub binding_initializer: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedNewOutcome {
    pub registration: ManagedLaunchRegistration,
    pub terminal_opened: bool,
}

pub struct ManagedNewService {
    store: Box<dyn ManagedSessionRepository>,
    provider_runtime: Box<dyn ManagedProviderRuntime>,
    launch_broker: Box<dyn ManagedLaunchBroker>,
    terminal_access: Box<dyn ManagedTerminalAccess>,
}

impl ManagedNewService {
    pub fn new(
        store: impl ManagedSessionRepository + 'static,
        provider_runtime: impl ManagedProviderRuntime + 'static,
        launch_broker: Box<dyn ManagedLaunchBroker>,
        terminal_access: Box<dyn ManagedTerminalAccess>,
    ) -> Self {
        Self {
            store: Box::new(store),
            provider_runtime: Box::new(provider_runtime),
            launch_broker,
            terminal_access,
        }
    }

    pub fn start(
        &mut self,
        mut request: ManagedNewRequest,
        now_ms: i64,
    ) -> Result<ManagedNewOutcome> {
        request.binding_initializer = true;
        if request.title.trim().is_empty() {
            return Err(anyhow!("New requires a Title"));
        }
        request.title = validate_alias(&request.title)?;
        request.description =
            validate_description(request.description.as_deref().unwrap_or_default())?;
        self.provider_runtime.validate_target(&request.target)?;
        let command = self
            .provider_runtime
            .new_session_command(request.provider, request.binding_initializer)?;
        let id = self.provider_runtime.new_registration_id(now_ms);
        let control_channel = self.launch_broker.control_channel(&id)?;
        if !control_channel.is_valid() {
            return Err(anyhow!("private Managed control channel is invalid"));
        }
        let registration = ManagedLaunchRegistration {
            id,
            status: ManagedLaunchStatus::Binding,
            provider: request.provider,
            target: request.target,
            session_alias: request.title,
            description: request.description,
            binding_initializer: request.binding_initializer,
            control_channel,
            created_at_ms: now_ms,
            launched_at_ms: None,
            launch_evidence: None,
            terminal_opened_at_ms: None,
            terminal_error: None,
            binding_error: None,
        };
        self.store.insert(&registration)?;
        let (registration, terminal_opened) = launch_and_open(
            self.store.as_ref(),
            self.launch_broker.as_mut(),
            self.terminal_access.as_mut(),
            &registration,
            &command,
            now_ms,
            "Managed Provider launch failed",
        )?;
        Ok(ManagedNewOutcome {
            registration,
            terminal_opened,
        })
    }
}
