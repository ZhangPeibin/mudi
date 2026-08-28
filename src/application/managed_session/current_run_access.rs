//! Verification and explicitly authorized terminal access for Managed Current Runs.

use super::ManagedSessionRepository;
use crate::domain::{
    BindingReceipt, ManagedControlChannel, ManagedCurrentRunRecord, ManagedSessionRecord, Provider,
};
use anyhow::{anyhow, Result};
use std::ffi::{OsStr, OsString};

pub trait ManagedCurrentRunEvidence: Send + Sync {
    fn revalidate(
        &self,
        run: &ManagedCurrentRunRecord,
        session: &ManagedSessionRecord,
        receipt: &BindingReceipt,
    ) -> Result<()>;
}

/// A revalidated target. Private control addressing has no public accessor.
#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedManagedCurrentRun {
    pub opaque_ref: String,
    pub provider: Provider,
    pub native_session_id: String,
    pub(crate) control_channel: ManagedControlChannel,
}

pub struct ManagedCurrentRunVerifier {
    evidence: Box<dyn ManagedCurrentRunEvidence>,
}

impl ManagedCurrentRunVerifier {
    pub fn new(evidence: Box<dyn ManagedCurrentRunEvidence>) -> Self {
        Self { evidence }
    }

    pub fn verify_ref(
        &self,
        store: &dyn ManagedSessionRepository,
        opaque_ref: &str,
    ) -> Result<VerifiedManagedCurrentRun> {
        if !valid_opaque_run_ref(opaque_ref) {
            return Err(anyhow!("Managed Current Run reference is invalid"));
        }
        let candidates = store
            .managed_current_runs()?
            .into_iter()
            .filter(|run| run.opaque_ref == opaque_ref)
            .collect::<Vec<_>>();
        let [run] = candidates.as_slice() else {
            return Err(if candidates.is_empty() {
                anyhow!("Managed Current Run reference is stale or unavailable")
            } else {
                anyhow!("Managed Current Run reference is ambiguous")
            });
        };
        let session = store
            .managed_session(run.provider, &run.native_session_id)?
            .ok_or_else(|| anyhow!("Managed Current Run has no Managed Session authority"))?;
        let receipt = store
            .binding_receipt_by_id(&run.binding_receipt_id)?
            .ok_or_else(|| anyhow!("Managed Current Run Binding Receipt is unavailable"))?;
        if session.provider != run.provider
            || session.native_session_id != run.native_session_id
            || session.binding_receipt_id != run.binding_receipt_id
            || receipt.provider != run.provider
            || receipt.native_session_id != run.native_session_id
            || receipt.provider_process != run.provider_process
            || receipt.id != run.binding_receipt_id
        {
            return Err(anyhow!(
                "Managed Current Run authority metadata is contradictory"
            ));
        }
        if !run.control_channel.is_valid() {
            return Err(anyhow!("private Managed control channel is invalid"));
        }
        self.evidence.revalidate(run, &session, &receipt)?;
        Ok(VerifiedManagedCurrentRun {
            opaque_ref: run.opaque_ref.clone(),
            provider: run.provider,
            native_session_id: run.native_session_id.clone(),
            control_channel: run.control_channel.clone(),
        })
    }

    pub fn verified_runs(
        &self,
        store: &dyn ManagedSessionRepository,
    ) -> Result<Vec<VerifiedManagedCurrentRun>> {
        let references = store
            .managed_current_runs()?
            .into_iter()
            .map(|run| run.opaque_ref)
            .collect::<Vec<_>>();
        Ok(references
            .iter()
            .filter_map(|opaque_ref| self.verify_ref(store, opaque_ref).ok())
            .collect())
    }
}

pub trait ManagedCurrentRunTerminalAccess: Send {
    fn open_terminal(&mut self, run: &VerifiedManagedCurrentRun) -> Result<()>;
    fn attach_foreground(&mut self, run: &VerifiedManagedCurrentRun) -> Result<()>;
}

pub struct ManagedCurrentRunAccessService {
    store: Box<dyn ManagedSessionRepository>,
    verifier: ManagedCurrentRunVerifier,
    terminal_access: Box<dyn ManagedCurrentRunTerminalAccess>,
}

impl ManagedCurrentRunAccessService {
    pub fn new(
        store: impl ManagedSessionRepository + 'static,
        verifier: ManagedCurrentRunVerifier,
        terminal_access: Box<dyn ManagedCurrentRunTerminalAccess>,
    ) -> Self {
        Self {
            store: Box::new(store),
            verifier,
            terminal_access,
        }
    }

    pub fn open_terminal(
        &mut self,
        provider: Provider,
        native_session_id: &str,
        opaque_ref: &str,
    ) -> Result<()> {
        let run = self.verify_identity(provider, native_session_id, opaque_ref)?;
        self.terminal_access.open_terminal(&run)
    }

    pub fn attach_command(
        &self,
        provider: Provider,
        native_session_id: &str,
        opaque_ref: &str,
    ) -> Result<String> {
        let run = self.verify_identity(provider, native_session_id, opaque_ref)?;
        Ok(format!("mudi attach {}", run.opaque_ref))
    }

    pub fn attach_foreground(&mut self, opaque_ref: &str) -> Result<()> {
        let run = self.verifier.verify_ref(self.store.as_ref(), opaque_ref)?;
        self.terminal_access.attach_foreground(&run)
    }

    fn verify_identity(
        &self,
        provider: Provider,
        native_session_id: &str,
        opaque_ref: &str,
    ) -> Result<VerifiedManagedCurrentRun> {
        let run = self.verifier.verify_ref(self.store.as_ref(), opaque_ref)?;
        if run.provider != provider || run.native_session_id != native_session_id {
            return Err(anyhow!(
                "Managed Current Run reference does not match the selected Session"
            ));
        }
        Ok(run)
    }
}

pub fn run_attach_from_args_with_access<I, S>(
    args: I,
    access: &mut ManagedCurrentRunAccessService,
) -> Result<bool>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let _program = args.next();
    if args.next().as_deref() != Some(OsStr::new("attach")) {
        return Ok(false);
    }
    let opaque_ref = args
        .next()
        .ok_or_else(|| anyhow!("missing opaque Managed Current Run reference"))?
        .into_string()
        .map_err(|_| anyhow!("Managed Current Run reference is not valid UTF-8"))?;
    if args.next().is_some() {
        return Err(anyhow!("unexpected attach command arguments"));
    }
    access.attach_foreground(&opaque_ref)?;
    Ok(true)
}

fn valid_opaque_run_ref(value: &str) -> bool {
    value.len() == 36
        && value.starts_with("run_")
        && value[4..].bytes().all(|byte| byte.is_ascii_hexdigit())
}
