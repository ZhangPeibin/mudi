//! SQLite persistence for Managed Session registrations and authority records.

use super::StateDatabase;
use crate::application::managed_session::{
    BindingReceipt, ManagedCurrentRunRecord, ManagedLaunchEvidence, ManagedLaunchRegistration,
    ManagedLaunchStatus, ManagedLaunchTarget, ManagedResumeTarget, ManagedSessionRecord,
    ManagedSessionRepository,
};
use crate::application::session_labels::validate_alias;
use crate::domain::{
    ProcessIdentity, Provider, Session, SessionIdentity, SessionProvenance, SourceFormat,
    SourceMetadata,
};
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

pub struct StateManagedSessionRepository {
    conn: Connection,
}

fn insert_registration(conn: &Connection, registration: &ManagedLaunchRegistration) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO managed_launch_registrations (
            registration_id, status, provider, working_directory, session_alias,
            description, tmux_socket_name, tmux_session_name, created_at_ms,
            launched_at_ms, tmux_pane_id, provider_pid, provider_started_at_ms,
            terminal_opened_at_ms, terminal_error, binding_error, binding_initializer
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
            ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17
        )
        "#,
        params![
            registration.id,
            registration.status.as_str(),
            registration.provider.as_str(),
            path_bytes(&registration.target.working_directory),
            registration.session_alias,
            registration.description,
            registration.control_channel.namespace,
            registration.control_channel.run_target,
            registration.created_at_ms,
            registration.launched_at_ms,
            registration
                .launch_evidence
                .as_ref()
                .map(|evidence| &evidence.channel_id),
            registration
                .launch_evidence
                .as_ref()
                .map(|evidence| evidence.provider_process.pid),
            registration
                .launch_evidence
                .as_ref()
                .map(|evidence| evidence.provider_process.started_at_ms),
            registration.terminal_opened_at_ms,
            registration.terminal_error,
            registration.binding_error,
            registration.binding_initializer,
        ],
    )?;
    Ok(())
}

impl StateManagedSessionRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let database = StateDatabase::open(path)?;
        let conn = database.connect()?;
        Ok(Self { conn })
    }

    pub fn insert(&self, registration: &ManagedLaunchRegistration) -> Result<()> {
        insert_registration(&self.conn, registration)
    }

    pub fn insert_resume(
        &mut self,
        registration: &ManagedLaunchRegistration,
        target: &ManagedResumeTarget,
    ) -> Result<()> {
        if registration.provider != target.provider {
            return Err(anyhow!("Resume Provider disagrees with its Registration"));
        }
        let transaction = self.conn.transaction()?;
        insert_registration(&transaction, registration)?;
        transaction.execute(
            "INSERT INTO managed_resume_registrations (registration_id, provider, native_session_id, prior_run_id) VALUES (?1, ?2, ?3, ?4)",
            params![
                registration.id,
                target.provider.as_str(),
                target.native_session_id,
                target.prior_run_id,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn managed_resume_target(
        &self,
        registration_id: &str,
    ) -> Result<Option<ManagedResumeTarget>> {
        self.conn
            .query_row(
                "SELECT provider, native_session_id, prior_run_id FROM managed_resume_registrations WHERE registration_id = ?1",
                [registration_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
            .map(|(provider, native_session_id, prior_run_id)| {
                Ok(ManagedResumeTarget {
                    provider: Provider::parse(&provider)
                        .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                    native_session_id,
                    prior_run_id,
                })
            })
            .transpose()
    }

    pub(crate) fn launch_title_association(
        &self,
        registration_id: &str,
    ) -> Result<Option<SessionIdentity>> {
        self.conn
            .query_row(
                "SELECT provider, native_session_id FROM managed_launch_title_associations WHERE registration_id = ?1",
                [registration_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .map(|(provider, native_id)| {
                Ok::<SessionIdentity, anyhow::Error>(SessionIdentity {
                    provider: Provider::parse(&provider)
                        .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                    native_id,
                })
            })
            .transpose()
    }

    pub(crate) fn record_launch_title_association(
        &self,
        registration_id: &str,
        session: &SessionIdentity,
    ) -> Result<()> {
        let inserted = self.conn.execute(
            r#"
            INSERT OR IGNORE INTO managed_launch_title_associations (
                registration_id, provider, native_session_id
            )
            SELECT registration_id, ?2, ?3
            FROM managed_launch_registrations
            WHERE registration_id = ?1 AND status = 'binding'
            "#,
            params![
                registration_id,
                session.provider.as_str(),
                session.native_id
            ],
        )?;
        let associated = self.launch_title_association(registration_id)?;
        if inserted == 1 || associated.as_ref() == Some(session) {
            Ok(())
        } else {
            Err(anyhow!(
                "Managed Launch Registration {registration_id} cannot record its Title association"
            ))
        }
    }

    fn associate_launch_title(
        &mut self,
        registration_id: &str,
        session: &SessionIdentity,
        alias: &str,
    ) -> Result<()> {
        let alias = validate_alias(alias)?;
        let transaction = self.conn.transaction()?;
        let inserted = transaction.execute(
            r#"
            INSERT OR IGNORE INTO managed_launch_title_associations (
                registration_id, provider, native_session_id
            )
            SELECT registration_id, ?2, ?3
            FROM managed_launch_registrations
            WHERE registration_id = ?1 AND status = 'binding'
            "#,
            params![
                registration_id,
                session.provider.as_str(),
                session.native_id
            ],
        )?;
        let associated = transaction
            .query_row(
                "SELECT provider, native_session_id FROM managed_launch_title_associations WHERE registration_id = ?1",
                [registration_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .map(|(provider, native_id)| {
                Ok::<SessionIdentity, anyhow::Error>(SessionIdentity {
                    provider: Provider::parse(&provider)
                        .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                    native_id,
                })
            })
            .transpose()?;
        if inserted != 1 && associated.as_ref() != Some(session) {
            return Err(anyhow!(
                "Managed Launch Registration {registration_id} cannot record its Title association"
            ));
        }
        let disposition = transaction
            .query_row(
                "SELECT disposition FROM session_metadata WHERE provider = ?1 AND native_session_id = ?2",
                params![session.provider.as_str(), session.native_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        if disposition.as_deref() == Some("permanently_excluded") {
            return Err(anyhow!(
                "Permanently excluded Session metadata cannot be edited"
            ));
        }
        transaction.execute(
            r#"
            INSERT INTO session_metadata (provider, native_session_id, alias)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(provider, native_session_id) DO UPDATE
            SET alias = COALESCE(session_metadata.alias, excluded.alias)
            "#,
            params![session.provider.as_str(), session.native_id, alias],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn has_pending_resume(&self, provider: Provider, native_session_id: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM managed_resume_registrations WHERE provider = ?1 AND native_session_id = ?2",
                params![provider.as_str(), native_session_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn get(&self, registration_id: &str) -> Result<Option<ManagedLaunchRegistration>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT status, provider, working_directory, session_alias, description,
                   tmux_socket_name, tmux_session_name, created_at_ms, launched_at_ms,
                   tmux_pane_id, provider_pid, provider_started_at_ms,
                   terminal_opened_at_ms, terminal_error, binding_error,
                   binding_initializer
            FROM managed_launch_registrations
            WHERE registration_id = ?1
            "#,
        )?;
        statement
            .query_row([registration_id], |row| {
                let status: String = row.get(0)?;
                let provider: String = row.get(1)?;
                let working_directory = path_from_bytes(row.get(2)?);
                let tmux_pane_id = row.get::<_, Option<String>>(9)?;
                let provider_pid = row.get::<_, Option<u32>>(10)?;
                let provider_started_at_ms = row.get::<_, Option<i64>>(11)?;
                Ok((
                    status,
                    provider,
                    working_directory,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    tmux_pane_id,
                    provider_pid,
                    provider_started_at_ms,
                    row.get::<_, Option<i64>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, bool>(15)?,
                ))
            })
            .optional()?
            .map(
                |(
                    status,
                    provider,
                    working_directory,
                    session_alias,
                    description,
                    tmux_socket_name,
                    tmux_session_name,
                    created_at_ms,
                    launched_at_ms,
                    tmux_pane_id,
                    provider_pid,
                    provider_started_at_ms,
                    terminal_opened_at_ms,
                    terminal_error,
                    binding_error,
                    binding_initializer,
                )| {
                    Ok(ManagedLaunchRegistration {
                        id: registration_id.to_string(),
                        status: ManagedLaunchStatus::parse(&status).ok_or_else(|| {
                            anyhow!("unknown Managed Launch Registration status {status:?}")
                        })?,
                        provider: Provider::parse(&provider)
                            .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                        target: ManagedLaunchTarget { working_directory },
                        session_alias,
                        description,
                        binding_initializer,
                        control_channel: crate::domain::ManagedControlChannel {
                            namespace: tmux_socket_name,
                            run_target: tmux_session_name,
                        },
                        created_at_ms,
                        launched_at_ms,
                        launch_evidence: persisted_launch_evidence(
                            tmux_pane_id,
                            provider_pid,
                            provider_started_at_ms,
                        )?,
                        terminal_opened_at_ms,
                        terminal_error,
                        binding_error,
                    })
                },
            )
            .transpose()
    }

    pub fn binding_registrations(&self) -> Result<Vec<ManagedLaunchRegistration>> {
        let mut statement = self.conn.prepare(
            "SELECT registration_id FROM managed_launch_registrations WHERE status = 'binding' ORDER BY created_at_ms, registration_id",
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        ids.into_iter()
            .map(|id| {
                self.get(&id)?
                    .ok_or_else(|| anyhow!("Binding Managed Launch Registration {id} disappeared"))
            })
            .collect()
    }

    pub fn registrations(&self) -> Result<Vec<ManagedLaunchRegistration>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT registration_id
            FROM managed_launch_registrations
            ORDER BY CASE status WHEN 'failed' THEN 1 ELSE 0 END,
                     created_at_ms DESC,
                     registration_id DESC
            "#,
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        ids.into_iter()
            .map(|id| {
                self.get(&id)?
                    .ok_or_else(|| anyhow!("Managed Launch Registration {id} disappeared"))
            })
            .collect()
    }

    pub fn binding_receipt(&self, registration_id: &str) -> Result<Option<BindingReceipt>> {
        let mut statement = self.conn.prepare(
            "SELECT receipt_id, provider, native_session_id, source_path, source_format, provider_pid, provider_started_at_ms, issued_at_ms FROM binding_receipts WHERE registration_id = ?1",
        )?;
        statement
            .query_row([registration_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, u32>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })
            .optional()?
            .map(
                |(
                    id,
                    provider,
                    native_session_id,
                    source_path,
                    source_format,
                    pid,
                    started_at_ms,
                    issued_at_ms,
                )| {
                    Ok(BindingReceipt {
                        id,
                        registration_id: registration_id.to_string(),
                        provider: Provider::parse(&provider)
                            .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                        native_session_id,
                        source: SourceMetadata {
                            path: path_from_bytes(source_path),
                            format: match source_format.as_str() {
                                "jsonl" => SourceFormat::JsonLines,
                                _ => {
                                    return Err(anyhow!("unknown source format {source_format:?}"))
                                }
                            },
                        },
                        provider_process: ProcessIdentity { pid, started_at_ms },
                        issued_at_ms,
                    })
                },
            )
            .transpose()
    }

    fn binding_receipt_by_id(&self, receipt_id: &str) -> Result<Option<BindingReceipt>> {
        let registration_id = self
            .conn
            .query_row(
                "SELECT registration_id FROM binding_receipts WHERE receipt_id = ?1",
                [receipt_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        registration_id
            .map(|registration_id| self.binding_receipt(&registration_id))
            .transpose()
            .map(Option::flatten)
    }

    pub fn managed_session(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<Option<ManagedSessionRecord>> {
        Ok(self.managed_sessions()?.into_iter().find(|session| {
            session.provider == provider && session.native_session_id == native_session_id
        }))
    }

    pub fn managed_sessions(&self) -> Result<Vec<ManagedSessionRecord>> {
        let mut statement = self.conn.prepare(
            "SELECT provider, native_session_id, working_directory, session_alias, description, binding_receipt_id FROM managed_sessions ORDER BY provider, native_session_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        rows.map(|row| {
            let (
                provider,
                native_session_id,
                working_directory,
                session_alias,
                description,
                binding_receipt_id,
            ) = row?;
            Ok(ManagedSessionRecord {
                provider: Provider::parse(&provider)
                    .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                native_session_id,
                target: ManagedLaunchTarget {
                    working_directory: path_from_bytes(working_directory),
                },
                session_alias,
                description,
                binding_receipt_id,
            })
        })
        .collect()
    }

    pub fn managed_current_run(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<Option<ManagedCurrentRunRecord>> {
        let authority_receipt = self
            .managed_session(provider, native_session_id)?
            .map(|session| session.binding_receipt_id);
        Ok(self.managed_current_runs()?.into_iter().find(|run| {
            run.provider == provider
                && run.native_session_id == native_session_id
                && authority_receipt.as_deref() == Some(run.binding_receipt_id.as_str())
        }))
    }

    pub fn managed_current_runs(&self) -> Result<Vec<ManagedCurrentRunRecord>> {
        let mut statement = self.conn.prepare(
            "SELECT run_id, opaque_ref, provider, native_session_id, provider_pid, provider_started_at_ms, tmux_socket_name, tmux_session_name, binding_receipt_id FROM managed_current_runs ORDER BY run_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, u32>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })?;
        rows.map(|row| {
            let (
                id,
                opaque_ref,
                provider,
                native_session_id,
                pid,
                started_at_ms,
                socket_name,
                session_name,
                binding_receipt_id,
            ) = row?;
            Ok(ManagedCurrentRunRecord {
                id,
                opaque_ref,
                provider: Provider::parse(&provider)
                    .ok_or_else(|| anyhow!("unknown Session Provider {provider:?}"))?,
                native_session_id,
                provider_process: ProcessIdentity { pid, started_at_ms },
                control_channel: crate::domain::ManagedControlChannel {
                    namespace: socket_name,
                    run_target: session_name,
                },
                binding_receipt_id,
            })
        })
        .collect()
    }

    pub fn apply_provenance(&self, sessions: &mut [Session]) -> Result<()> {
        let managed = self
            .managed_sessions()?
            .into_iter()
            .map(|session| SessionIdentity {
                provider: session.provider,
                native_id: session.native_session_id,
            })
            .collect::<HashSet<_>>();
        for session in sessions {
            session.provenance = if managed.contains(&SessionIdentity::from(&*session)) {
                SessionProvenance::Managed
            } else {
                SessionProvenance::Observed
            };
        }
        Ok(())
    }

    fn establish_management(
        &mut self,
        registration: &ManagedLaunchRegistration,
        receipt: &BindingReceipt,
    ) -> Result<()> {
        let resume = self.managed_resume_target(&registration.id)?;
        let resume_authority = resume
            .as_ref()
            .map(|target| {
                let run = self
                    .managed_current_run(target.provider, &target.native_session_id)?
                    .ok_or_else(|| anyhow!("Resume prior Managed Current Run is unavailable"))?;
                let session = self
                    .managed_session(target.provider, &target.native_session_id)?
                    .ok_or_else(|| anyhow!("Resume Managed Session is unavailable"))?;
                if run.id != target.prior_run_id
                    || session.binding_receipt_id != run.binding_receipt_id
                    || registration.provider != target.provider
                    || receipt.provider != target.provider
                    || receipt.native_session_id != target.native_session_id
                    || registration.target != session.target
                    || registration.session_alias != session.session_alias
                    || registration.description != session.description
                {
                    return Err(anyhow!("Resume authority changed before Receipt issuance"));
                }
                Ok(run.binding_receipt_id)
            })
            .transpose()?;
        super::bindings::establish_management(
            &mut self.conn,
            registration,
            receipt,
            resume_authority,
        )
    }

    fn record_launched(
        &self,
        registration_id: &str,
        now_ms: i64,
        evidence: ManagedLaunchEvidence,
    ) -> Result<()> {
        update_one(
            self.conn.execute(
                r#"
                UPDATE managed_launch_registrations
                SET launched_at_ms = ?2,
                    tmux_pane_id = ?3,
                    provider_pid = ?4,
                    provider_started_at_ms = ?5
                WHERE registration_id = ?1
                  AND launched_at_ms IS NULL
                  AND tmux_pane_id IS NULL
                  AND provider_pid IS NULL
                  AND provider_started_at_ms IS NULL
                "#,
                params![
                    registration_id,
                    now_ms,
                    evidence.channel_id,
                    evidence.provider_process.pid,
                    evidence.provider_process.started_at_ms,
                ],
            )?,
            registration_id,
        )
    }

    fn record_terminal_opened(&self, registration_id: &str, now_ms: i64) -> Result<()> {
        update_one(
            self.conn.execute(
                "UPDATE managed_launch_registrations SET terminal_opened_at_ms = ?2, terminal_error = NULL WHERE registration_id = ?1 AND launched_at_ms IS NOT NULL",
                params![registration_id, now_ms],
            )?,
            registration_id,
        )
    }

    fn record_terminal_error(&self, registration_id: &str, error: &str) -> Result<()> {
        update_one(
            self.conn.execute(
                "UPDATE managed_launch_registrations SET terminal_error = ?2 WHERE registration_id = ?1 AND launched_at_ms IS NOT NULL",
                params![registration_id, error],
            )?,
            registration_id,
        )
    }

    fn remove_binding(&mut self, registration_id: &str) -> Result<()> {
        let transaction = self.conn.transaction()?;
        transaction.execute(
            "DELETE FROM managed_resume_registrations WHERE registration_id = ?1",
            [registration_id],
        )?;
        transaction.execute(
            "DELETE FROM managed_launch_title_associations WHERE registration_id = ?1",
            [registration_id],
        )?;
        update_one(
            transaction.execute(
                "DELETE FROM managed_launch_registrations WHERE registration_id = ?1 AND status = 'binding'",
                [registration_id],
            )?,
            registration_id,
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn mark_needs_review(&self, registration_id: &str, error: &str) -> Result<()> {
        update_one(
            self.conn.execute(
                "UPDATE managed_launch_registrations SET status = 'needs_review', binding_error = ?2 WHERE registration_id = ?1 AND status = 'binding'",
                params![registration_id, error],
            )?,
            registration_id,
        )
    }

    fn mark_failed(&mut self, registration_id: &str, error: &str) -> Result<()> {
        let transaction = self.conn.transaction()?;
        update_one(
            transaction.execute(
                "UPDATE managed_launch_registrations SET status = 'failed', binding_error = ?2 WHERE registration_id = ?1 AND status = 'binding'",
                params![registration_id, error],
            )?,
            registration_id,
        )?;
        transaction.execute(
            "DELETE FROM managed_resume_registrations WHERE registration_id = ?1",
            [registration_id],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

impl ManagedSessionRepository for StateManagedSessionRepository {
    fn insert(&self, registration: &ManagedLaunchRegistration) -> Result<()> {
        StateManagedSessionRepository::insert(self, registration)
    }

    fn insert_resume(
        &mut self,
        registration: &ManagedLaunchRegistration,
        target: &ManagedResumeTarget,
    ) -> Result<()> {
        StateManagedSessionRepository::insert_resume(self, registration, target)
    }

    fn managed_resume_target(&self, registration_id: &str) -> Result<Option<ManagedResumeTarget>> {
        StateManagedSessionRepository::managed_resume_target(self, registration_id)
    }

    fn launch_title_association(&self, registration_id: &str) -> Result<Option<SessionIdentity>> {
        StateManagedSessionRepository::launch_title_association(self, registration_id)
    }

    fn record_launch_title_association(
        &self,
        registration_id: &str,
        session: &SessionIdentity,
    ) -> Result<()> {
        StateManagedSessionRepository::record_launch_title_association(
            self,
            registration_id,
            session,
        )
    }

    fn associate_launch_title(
        &mut self,
        registration_id: &str,
        session: &SessionIdentity,
        alias: &str,
    ) -> Result<()> {
        StateManagedSessionRepository::associate_launch_title(self, registration_id, session, alias)
    }

    fn has_pending_resume(&self, provider: Provider, native_session_id: &str) -> Result<bool> {
        StateManagedSessionRepository::has_pending_resume(self, provider, native_session_id)
    }

    fn get(&self, registration_id: &str) -> Result<Option<ManagedLaunchRegistration>> {
        StateManagedSessionRepository::get(self, registration_id)
    }

    fn binding_registrations(&self) -> Result<Vec<ManagedLaunchRegistration>> {
        StateManagedSessionRepository::binding_registrations(self)
    }

    fn registrations(&self) -> Result<Vec<ManagedLaunchRegistration>> {
        StateManagedSessionRepository::registrations(self)
    }

    fn binding_receipt(&self, registration_id: &str) -> Result<Option<BindingReceipt>> {
        StateManagedSessionRepository::binding_receipt(self, registration_id)
    }

    fn binding_receipt_by_id(&self, receipt_id: &str) -> Result<Option<BindingReceipt>> {
        StateManagedSessionRepository::binding_receipt_by_id(self, receipt_id)
    }

    fn managed_session(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<Option<ManagedSessionRecord>> {
        StateManagedSessionRepository::managed_session(self, provider, native_session_id)
    }

    fn managed_sessions(&self) -> Result<Vec<ManagedSessionRecord>> {
        StateManagedSessionRepository::managed_sessions(self)
    }

    fn managed_current_run(
        &self,
        provider: Provider,
        native_session_id: &str,
    ) -> Result<Option<ManagedCurrentRunRecord>> {
        StateManagedSessionRepository::managed_current_run(self, provider, native_session_id)
    }

    fn managed_current_runs(&self) -> Result<Vec<ManagedCurrentRunRecord>> {
        StateManagedSessionRepository::managed_current_runs(self)
    }

    fn apply_provenance(&self, sessions: &mut [Session]) -> Result<()> {
        StateManagedSessionRepository::apply_provenance(self, sessions)
    }

    fn establish_management(
        &mut self,
        registration: &ManagedLaunchRegistration,
        receipt: &BindingReceipt,
    ) -> Result<()> {
        StateManagedSessionRepository::establish_management(self, registration, receipt)
    }

    fn record_launched(
        &self,
        registration_id: &str,
        now_ms: i64,
        evidence: ManagedLaunchEvidence,
    ) -> Result<()> {
        StateManagedSessionRepository::record_launched(self, registration_id, now_ms, evidence)
    }

    fn record_terminal_opened(&self, registration_id: &str, now_ms: i64) -> Result<()> {
        StateManagedSessionRepository::record_terminal_opened(self, registration_id, now_ms)
    }

    fn record_terminal_error(&self, registration_id: &str, error: &str) -> Result<()> {
        StateManagedSessionRepository::record_terminal_error(self, registration_id, error)
    }

    fn remove_binding(&mut self, registration_id: &str) -> Result<()> {
        StateManagedSessionRepository::remove_binding(self, registration_id)
    }

    fn mark_needs_review(&self, registration_id: &str, error: &str) -> Result<()> {
        StateManagedSessionRepository::mark_needs_review(self, registration_id, error)
    }

    fn mark_failed(&mut self, registration_id: &str, error: &str) -> Result<()> {
        StateManagedSessionRepository::mark_failed(self, registration_id, error)
    }
}

fn persisted_launch_evidence(
    channel_id: Option<String>,
    provider_pid: Option<u32>,
    provider_started_at_ms: Option<i64>,
) -> Result<Option<ManagedLaunchEvidence>> {
    match (channel_id, provider_pid, provider_started_at_ms) {
        (Some(channel_id), Some(pid), Some(started_at_ms)) => Ok(Some(ManagedLaunchEvidence {
            channel_id,
            provider_process: ProcessIdentity { pid, started_at_ms },
        })),
        (None, None, None) => Ok(None),
        _ => Err(anyhow!("incomplete persisted Managed launch evidence")),
    }
}

fn update_one(changed: usize, registration_id: &str) -> Result<()> {
    if changed == 1 {
        Ok(())
    } else {
        Err(anyhow!(
            "Managed Launch Registration {registration_id} is unavailable or in the wrong state"
        ))
    }
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}

#[cfg(unix)]
fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
}
