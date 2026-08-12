//! Generic boundary for SessionMesh-managed Skills.

pub mod handoff;

use crate::domain::Provider;
use anyhow::Result;
use handoff::ManagedHandoffSkills;
use std::path::PathBuf;

/// A declared, versioned Skill that SessionMesh may manage locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillDefinition {
    pub id: &'static str,
    pub version: u32,
}

pub const HANDOFF_SKILL: SkillDefinition = SkillDefinition {
    id: "sessionmesh-handoff",
    version: 1,
};

/// Resolves a locally declared managed Skill without inspecting provider content.
pub trait SkillCatalog {
    fn declared(&self, id: &str) -> Option<SkillDefinition>;
}

/// Installs or verifies owner-only managed copies and renders worker configuration.
pub trait SkillInstaller: Send + Sync + std::fmt::Debug {
    fn install_for_first_use(&self) -> Result<()>;
    fn verify(&self, provider: Provider) -> std::result::Result<(), String>;
    fn worker_roots(&self) -> Option<(PathBuf, PathBuf)>;
}

/// The catalog and installer for the current Handoff Skill declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSkillCatalog {
    handoff: ManagedHandoffSkills,
}

impl ManagedSkillCatalog {
    pub fn handoff(handoff: ManagedHandoffSkills) -> Self {
        Self { handoff }
    }
}

impl SkillCatalog for ManagedSkillCatalog {
    fn declared(&self, id: &str) -> Option<SkillDefinition> {
        (id == HANDOFF_SKILL.id).then_some(HANDOFF_SKILL)
    }
}

impl SkillInstaller for ManagedSkillCatalog {
    fn install_for_first_use(&self) -> Result<()> {
        self.handoff.install_for_first_use()
    }

    fn verify(&self, provider: Provider) -> std::result::Result<(), String> {
        self.handoff
            .verify(provider)
            .map_err(|status| status.failure_code().to_string())
    }

    fn worker_roots(&self) -> Option<(PathBuf, PathBuf)> {
        let (codex, claude) = self.handoff.roots();
        Some((codex.to_path_buf(), claude.to_path_buf()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_only_resolves_the_declared_skill() {
        let catalog = ManagedSkillCatalog::handoff(ManagedHandoffSkills::new(
            PathBuf::from("/codex"),
            PathBuf::from("/claude"),
        ));

        assert_eq!(catalog.declared("sessionmesh-handoff"), Some(HANDOFF_SKILL));
        assert_eq!(catalog.declared("not-managed"), None);
    }
}
