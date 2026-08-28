//! Atomic SQLite publication of Binding Receipts and Managed authority records.

use crate::domain::{BindingReceipt, ManagedLaunchRegistration};
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use std::io::Read;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

pub(super) fn establish_management(
    connection: &mut Connection,
    registration: &ManagedLaunchRegistration,
    receipt: &BindingReceipt,
    prior_receipt_id: Option<String>,
) -> Result<()> {
    let opaque_ref = new_opaque_run_ref()?;
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO binding_receipts (receipt_id, registration_id, provider, native_session_id, source_path, source_format, provider_pid, provider_started_at_ms, issued_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, 'jsonl', ?6, ?7, ?8)",
        params![
            receipt.id,
            receipt.registration_id,
            receipt.provider.as_str(),
            receipt.native_session_id,
            path_bytes(&receipt.source.path),
            receipt.provider_process.pid,
            receipt.provider_process.started_at_ms,
            receipt.issued_at_ms,
        ],
    )?;
    if let Some(prior_receipt_id) = prior_receipt_id {
        update_one(
            transaction.execute(
                "UPDATE managed_sessions SET binding_receipt_id = ?3 WHERE provider = ?1 AND native_session_id = ?2 AND binding_receipt_id = ?4",
                params![
                    receipt.provider.as_str(),
                    receipt.native_session_id,
                    receipt.id,
                    prior_receipt_id,
                ],
            )?,
            &registration.id,
        )?;
    } else {
        transaction.execute(
            "INSERT INTO managed_sessions (provider, native_session_id, working_directory, session_alias, description, binding_receipt_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                receipt.provider.as_str(),
                receipt.native_session_id,
                path_bytes(&registration.target.working_directory),
                registration.session_alias,
                registration.description,
                receipt.id,
            ],
        )?;
        transaction.execute(
            r#"
            INSERT INTO session_metadata (
                provider, native_session_id, alias, description
            ) VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(provider, native_session_id) DO UPDATE SET
                alias = COALESCE(session_metadata.alias, excluded.alias),
                description = COALESCE(session_metadata.description, excluded.description)
            "#,
            params![
                receipt.provider.as_str(),
                receipt.native_session_id,
                registration.session_alias,
                registration.description,
            ],
        )?;
    }
    transaction.execute(
        "INSERT INTO managed_current_runs (run_id, opaque_ref, provider, native_session_id, provider_pid, provider_started_at_ms, tmux_socket_name, tmux_session_name, binding_receipt_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            format!("run-{}", registration.id),
            opaque_ref,
            receipt.provider.as_str(),
            receipt.native_session_id,
            receipt.provider_process.pid,
            receipt.provider_process.started_at_ms,
            registration.control_channel.namespace,
            registration.control_channel.run_target,
            receipt.id,
        ],
    )?;
    transaction.execute(
        "DELETE FROM managed_resume_registrations WHERE registration_id = ?1",
        [registration.id.as_str()],
    )?;
    transaction.execute(
        "DELETE FROM managed_launch_title_associations WHERE registration_id = ?1",
        [registration.id.as_str()],
    )?;
    update_one(
        transaction.execute(
            "DELETE FROM managed_launch_registrations WHERE registration_id = ?1 AND status = 'binding'",
            [registration.id.as_str()],
        )?,
        &registration.id,
    )?;
    transaction.commit()?;
    Ok(())
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

fn new_opaque_run_ref() -> Result<String> {
    let mut random = [0_u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let encoded = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("run_{encoded}"))
}
