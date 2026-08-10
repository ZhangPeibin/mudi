//! Handoff Artifact validation and owner-only publication.

use anyhow::{anyhow, Result};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

static NEXT_ARTIFACT_TEMP: AtomicU64 = AtomicU64::new(0);

const REQUIRED_HEADINGS: [&str; 10] = [
    "# Handoff",
    "## Objective",
    "## Current State",
    "## Decisions",
    "## Completed",
    "## Remaining",
    "## Files Changed",
    "## Verification",
    "## Risks And Blockers",
    "## Next Action",
];

pub(crate) fn validate(candidate: &str) -> Result<()> {
    if candidate.trim().is_empty() {
        return Err(anyhow!("Handoff Artifact is empty"));
    }
    if candidate.contains("-----BEGIN PRIVATE KEY-----")
        || candidate.contains("-----BEGIN RSA PRIVATE KEY-----")
        || candidate.contains("-----BEGIN OPENSSH PRIVATE KEY-----")
    {
        return Err(anyhow!("Handoff Artifact contains secret material"));
    }
    let lines = candidate.lines().collect::<Vec<_>>();
    if contains_heading(&lines, "transcript") {
        return Err(anyhow!("Handoff Artifact contains a Transcript section"));
    }
    if contains_heading(&lines, "tool output") {
        return Err(anyhow!("Handoff Artifact contains a Tool Output section"));
    }
    let required_headings = required_headings();
    let contract_headings = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("# ") || line.starts_with("## "))
        .collect::<Vec<_>>();
    if contract_headings.as_slice() != required_headings {
        return Err(anyhow!(
            "Handoff Artifact headings do not match the contract"
        ));
    }
    let mut positions = Vec::with_capacity(required_headings.len());
    let mut search_from = 0;
    for heading in required_headings {
        let relative = lines[search_from..]
            .iter()
            .position(|line| *line == heading)
            .ok_or_else(|| anyhow!("Handoff Artifact is missing {heading:?}"))?;
        let position = search_from + relative;
        positions.push(position);
        search_from = position + 1;
    }
    for heading_index in [1, 2, 5, 9] {
        let start = positions[heading_index] + 1;
        let end = positions
            .get(heading_index + 1)
            .copied()
            .unwrap_or(lines.len());
        if !lines[start..end].iter().any(|line| !line.trim().is_empty()) {
            return Err(anyhow!(
                "Handoff Artifact section {:?} is empty",
                required_headings[heading_index]
            ));
        }
    }
    Ok(())
}

pub(crate) fn required_headings() -> [&'static str; 10] {
    REQUIRED_HEADINGS
}

pub(crate) fn publish(root: &Path, task_id: &str, contents: &[u8]) -> Result<PathBuf> {
    create_private_dir(root)?;
    let task_dir = root.join(task_id);
    create_private_dir(&task_dir)?;
    let final_path = task_dir.join("handoff.md");
    let sequence = NEXT_ARTIFACT_TEMP.fetch_add(1, Ordering::Relaxed);
    let temporary_path =
        task_dir.join(format!(".handoff.md.tmp-{}-{sequence}", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut temporary = options.open(&temporary_path)?;
    temporary.write_all(contents)?;
    temporary.sync_all()?;
    fs::rename(&temporary_path, &final_path)?;
    #[cfg(unix)]
    fs::set_permissions(&final_path, fs::Permissions::from_mode(0o600))?;
    fs::File::open(&task_dir)?.sync_all()?;
    Ok(final_path)
}

fn contains_heading(lines: &[&str], needle: &str) -> bool {
    lines.iter().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with('#')
            && trimmed
                .trim_start_matches('#')
                .trim()
                .to_ascii_lowercase()
                .contains(needle)
    })
}

fn create_private_dir(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "private directory is not a real directory: {}",
                    path.display()
                ),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            builder.mode(0o700);
            builder.create(path)?;
        }
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
