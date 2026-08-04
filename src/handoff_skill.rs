//! Managed Provider Skill used to generate Handoff Artifact candidates.

use crate::domain::Provider;
use anyhow::{anyhow, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

const SKILL_NAME: &str = "sessionmesh-handoff";
const SKILL_FILE: &str = "SKILL.md";
const MANAGED_MARKER: &str = "<!-- sessionmesh-managed-skill: sessionmesh-handoff -->";
const VERSION_MARKER_PREFIX: &str = "<!-- sessionmesh-managed-version: ";
const CURRENT_VERSION: u32 = 1;
const CANONICAL_SKILL: &str = include_str!("../skills/sessionmesh-handoff/SKILL.md");

pub(crate) fn artifact_headings() -> [&'static str; 10] {
    CANONICAL_SKILL
        .lines()
        .filter(|line| line.starts_with("# ") || line.starts_with("## "))
        .collect::<Vec<_>>()
        .try_into()
        .expect("canonical Handoff Skill contains exactly ten Artifact headings")
}

/// Provider-standard roots that receive the same SessionMesh-managed Skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedHandoffSkills {
    codex_skills_root: PathBuf,
    claude_skills_root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedSkillStatus {
    Missing,
    Stale,
    Unreadable,
    Conflict,
}

impl ManagedSkillStatus {
    pub(crate) fn failure_code(self) -> &'static str {
        match self {
            Self::Missing => "handoff_skill_missing",
            Self::Stale => "handoff_skill_stale",
            Self::Unreadable => "handoff_skill_unreadable",
            Self::Conflict => "handoff_skill_conflict",
        }
    }
}

impl ManagedHandoffSkills {
    pub fn for_user_home(home: &Path, claude_dir: &Path) -> Self {
        Self::new(home.join(".agents/skills"), claude_dir.join("skills"))
    }

    pub fn new(codex_skills_root: PathBuf, claude_skills_root: PathBuf) -> Self {
        Self {
            codex_skills_root,
            claude_skills_root,
        }
    }

    /// Install the canonical Skill as the explicit first-use part of authorization.
    pub fn install_for_first_use(&self) -> Result<()> {
        install_copy(&self.codex_skills_root, "Codex CLI")?;
        install_copy(&self.claude_skills_root, "Claude Code")?;
        Ok(())
    }

    pub(crate) fn verify(&self, provider: Provider) -> std::result::Result<(), ManagedSkillStatus> {
        let skills_root = match provider {
            Provider::CodexCli => &self.codex_skills_root,
            Provider::ClaudeCode => &self.claude_skills_root,
        };
        verify_copy(skills_root)
    }

    pub(crate) fn roots(&self) -> (&Path, &Path) {
        (&self.codex_skills_root, &self.claude_skills_root)
    }
}

fn verify_copy(skills_root: &Path) -> std::result::Result<(), ManagedSkillStatus> {
    let skill_directory = skills_root.join(SKILL_NAME);
    let directory_metadata = fs::symlink_metadata(&skill_directory).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ManagedSkillStatus::Missing
        } else {
            ManagedSkillStatus::Unreadable
        }
    })?;
    if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
        return Err(ManagedSkillStatus::Conflict);
    }
    #[cfg(unix)]
    if directory_metadata.permissions().mode() & 0o077 != 0 {
        return Err(ManagedSkillStatus::Stale);
    }

    let skill_path = skill_directory.join(SKILL_FILE);
    let file_metadata = fs::symlink_metadata(&skill_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ManagedSkillStatus::Missing
        } else {
            ManagedSkillStatus::Unreadable
        }
    })?;
    if file_metadata.file_type().is_symlink() || !file_metadata.is_file() {
        return Err(ManagedSkillStatus::Conflict);
    }
    #[cfg(unix)]
    if file_metadata.permissions().mode() & 0o077 != 0 {
        return Err(ManagedSkillStatus::Stale);
    }
    let installed = fs::read_to_string(skill_path).map_err(|_| ManagedSkillStatus::Unreadable)?;
    if installed == CANONICAL_SKILL {
        Ok(())
    } else if managed_version(&installed).is_some() {
        Err(ManagedSkillStatus::Stale)
    } else {
        Err(ManagedSkillStatus::Conflict)
    }
}

fn install_copy(skills_root: &Path, provider: &str) -> Result<()> {
    fs::create_dir_all(skills_root)?;
    let skill_directory = skills_root.join(SKILL_NAME);
    match fs::symlink_metadata(&skill_directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(anyhow!(
                "{provider} managed Skill conflicts with an unrelated same-name Skill"
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            builder.mode(0o700);
            builder.create(&skill_directory)?;
        }
        Err(error) => return Err(error.into()),
    }

    let skill_path = skill_directory.join(SKILL_FILE);
    match fs::symlink_metadata(&skill_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(anyhow!(
                "{provider} managed Skill conflicts with an unrelated same-name Skill"
            ));
        }
        Ok(_) => {
            let installed = fs::read_to_string(&skill_path)
                .map_err(|_| anyhow!("{provider} managed Skill is unreadable"))?;
            if installed == CANONICAL_SKILL {
                #[cfg(unix)]
                {
                    fs::set_permissions(&skill_directory, fs::Permissions::from_mode(0o700))?;
                    fs::set_permissions(&skill_path, fs::Permissions::from_mode(0o600))?;
                }
                return Ok(());
            }
            let Some(version) = managed_version(&installed) else {
                return Err(anyhow!(
                    "{provider} managed Skill conflicts with an unrelated same-name Skill"
                ));
            };
            if version > CURRENT_VERSION {
                return Err(anyhow!(
                    "{provider} managed Skill is out of date (installed {version}, expected {CURRENT_VERSION})"
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(anyhow!("{provider} managed Skill is unreadable")),
    }

    #[cfg(unix)]
    fs::set_permissions(&skill_directory, fs::Permissions::from_mode(0o700))?;
    atomic_publish(&skill_directory, &skill_path)
}

fn managed_version(content: &str) -> Option<u32> {
    content
        .lines()
        .any(|line| line == MANAGED_MARKER)
        .then_some(())?;
    content.lines().find_map(|line| {
        line.strip_prefix(VERSION_MARKER_PREFIX)?
            .strip_suffix(" -->")?
            .parse()
            .ok()
    })
}

fn atomic_publish(skill_directory: &Path, skill_path: &Path) -> Result<()> {
    let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary = skill_directory.join(format!(
        ".{SKILL_FILE}.tmp-{}-{sequence}",
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    file.write_all(CANONICAL_SKILL.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temporary, skill_path)?;
    #[cfg(unix)]
    fs::set_permissions(skill_path, fs::Permissions::from_mode(0o600))?;
    fs::File::open(skill_directory)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_skill_and_validator_share_the_exact_artifact_heading_contract() {
        let expected = [
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

        assert_eq!(artifact_headings(), expected);
        assert_eq!(crate::handoff::required_handoff_headings(), expected);
    }
}
