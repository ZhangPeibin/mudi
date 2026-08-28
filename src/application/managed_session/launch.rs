use super::{
    ManagedLaunchBroker, ManagedSessionRepository, ManagedTerminalAccess, ProviderLaunchCommand,
};
use crate::domain::ManagedLaunchRegistration;
use anyhow::{anyhow, Context, Result};

pub(super) fn launch_and_open(
    store: &dyn ManagedSessionRepository,
    broker: &mut dyn ManagedLaunchBroker,
    terminal: &mut dyn ManagedTerminalAccess,
    registration: &ManagedLaunchRegistration,
    command: &ProviderLaunchCommand,
    now_ms: i64,
    launch_context: &'static str,
) -> Result<(ManagedLaunchRegistration, bool)> {
    let launch_evidence = broker
        .launch(registration, command)
        .with_context(|| launch_context)?;
    store.record_launched(&registration.id, now_ms, launch_evidence)?;
    let launched = store
        .get(&registration.id)?
        .ok_or_else(|| anyhow!("Managed Launch Registration disappeared after launch"))?;
    let terminal_opened = match terminal.open(&launched) {
        Ok(()) => {
            store.record_terminal_opened(&registration.id, now_ms)?;
            true
        }
        Err(error) => {
            store.record_terminal_error(&registration.id, &error.to_string())?;
            false
        }
    };
    let registration = store
        .get(&registration.id)?
        .ok_or_else(|| anyhow!("Managed Launch Registration disappeared after Terminal access"))?;
    Ok((registration, terminal_opened))
}
