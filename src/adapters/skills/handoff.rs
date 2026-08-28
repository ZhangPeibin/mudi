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

const SKILL_NAME: &str = "mudi-handoff";
const SKILL_FILE: &str = "SKILL.md";
const MANAGED_MARKER: &str = "<!-- mudi-managed-skill: mudi-handoff -->";
const VERSION_MARKER_PREFIX: &str = "<!-- mudi-managed-version: ";
const CURRENT_VERSION: u32 = 1;
const CANONICAL_SKILL: &str = include_str!("../../../skills/mudi-handoff/SKILL.md");

/// Provider-standard roots that receive the same Mudi-managed Skill.
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
        self.invocation_for(provider).map(|_| ())
    }

    pub(crate) fn invocation_for(
        &self,
        provider: Provider,
    ) -> std::result::Result<String, ManagedSkillStatus> {
        let skills_root = match provider {
            Provider::CodexCli => &self.codex_skills_root,
            Provider::ClaudeCode => &self.claude_skills_root,
        };
        let canonical_status = verify_copy(skills_root);
        if canonical_status.is_ok() {
            return Ok(skill_invocation(provider, SKILL_NAME));
        }
        Err(canonical_status.expect_err("canonical Skill status was checked above"))
    }

    pub(crate) fn roots(&self) -> (&Path, &Path) {
        (&self.codex_skills_root, &self.claude_skills_root)
    }
}

fn verify_copy(skills_root: &Path) -> std::result::Result<(), ManagedSkillStatus> {
    verify_named_copy(
        skills_root,
        SKILL_NAME,
        CANONICAL_SKILL,
        MANAGED_MARKER,
        VERSION_MARKER_PREFIX,
    )
}

fn verify_named_copy(
    skills_root: &Path,
    skill_name: &str,
    canonical_skill: &str,
    managed_marker: &str,
    version_marker_prefix: &str,
) -> std::result::Result<(), ManagedSkillStatus> {
    let skill_directory = skills_root.join(skill_name);
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
    let installed_version = managed_version(&installed, managed_marker, version_marker_prefix);
    if installed == canonical_skill {
        Ok(())
    } else if installed_version.is_some() {
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
            let Some(version) = managed_version(&installed, MANAGED_MARKER, VERSION_MARKER_PREFIX)
            else {
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

fn managed_version(
    content: &str,
    managed_marker: &str,
    version_marker_prefix: &str,
) -> Option<u32> {
    content
        .lines()
        .any(|line| line == managed_marker)
        .then_some(())?;
    content.lines().find_map(|line| {
        line.strip_prefix(version_marker_prefix)?
            .strip_suffix(" -->")?
            .parse()
            .ok()
    })
}

fn skill_invocation(provider: Provider, skill_name: &str) -> String {
    match provider {
        Provider::CodexCli => format!("${skill_name}"),
        Provider::ClaudeCode => format!("/{skill_name}"),
    }
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

        let headings = CANONICAL_SKILL
            .lines()
            .filter(|line| line.starts_with("# ") || line.starts_with("## "))
            .collect::<Vec<_>>();

        assert_eq!(headings, expected);
        assert_eq!(
            crate::application::handoff::artifact::required_headings(),
            expected
        );
    }
}
