//! Project Tree interaction state and navigation.

use super::model::{ProjectTreeKey, ProjectTreeRow};
use crate::app::Snapshot;
use crate::domain::{ProjectIdentity, Session, SessionIdentity};
use crate::managed_sessions::ManagedLaunchTarget;
use std::collections::HashSet;
use std::path::PathBuf;

pub(in crate::ui) struct NewTargetSelection {
    pub targets: Vec<ManagedLaunchTarget>,
    pub selected: usize,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::ui) enum VerticalDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::ui) enum HorizontalDirection {
    Left,
    Right,
}

/// Complete interaction state for one Project Tree instance.
#[derive(Default)]
pub(in crate::ui) struct State {
    selection: Option<ProjectTreeKey>,
    collapsed_nodes: HashSet<ProjectTreeKey>,
    expanded_disclosures: HashSet<ProjectTreeKey>,
}

impl State {
    pub(super) fn rows<'a>(&self, snap: &'a Snapshot) -> Vec<ProjectTreeRow<'a>> {
        super::rows(snap, &self.collapsed_nodes, &self.expanded_disclosures)
    }

    /// Resolve the stable selection, falling back deterministically when its node vanished.
    pub(super) fn effective_selection(&self, snap: &Snapshot) -> Option<ProjectTreeKey> {
        let rows = self.rows(snap);
        if let Some(selection) = self
            .selection
            .as_ref()
            .filter(|selection| rows.iter().any(|row| row.key() == *selection))
        {
            return Some(selection.clone());
        }
        rows.iter()
            .find_map(ProjectTreeRow::session_key)
            .or_else(|| rows.first().map(|row| row.key().clone()))
    }

    /// Reconcile stale or hidden selection against the latest visible rows.
    pub(in crate::ui) fn reconcile(&mut self, snap: &Snapshot) {
        self.selection = self.effective_selection(snap);
    }

    pub(in crate::ui) fn move_vertical(&mut self, snap: &Snapshot, direction: VerticalDirection) {
        let rows = self.rows(snap);
        let Some(selection) = self.effective_selection(snap) else {
            return;
        };
        let Some(index) = rows.iter().position(|row| row.key() == &selection) else {
            return;
        };
        let next = match direction {
            VerticalDirection::Down => (index + 1).min(rows.len().saturating_sub(1)),
            VerticalDirection::Up => index.saturating_sub(1),
        };
        self.selection = rows.get(next).map(|row| row.key().clone());
    }

    /// Apply conventional Tree Left/Right behavior without exposing node relationships.
    pub(in crate::ui) fn move_horizontal(
        &mut self,
        snap: &Snapshot,
        direction: HorizontalDirection,
    ) {
        let rows = self.rows(snap);
        let Some(selection) = self.effective_selection(snap) else {
            return;
        };
        let Some(index) = rows.iter().position(|row| row.key() == &selection) else {
            return;
        };
        let row = &rows[index];
        match direction {
            HorizontalDirection::Right if row.is_group() && !row.is_expanded() => {
                self.expand(row.key());
                self.selection = Some(selection);
            }
            HorizontalDirection::Right
                if row.is_group()
                    && rows
                        .get(index + 1)
                        .is_some_and(|child| child.depth() > row.depth()) =>
            {
                self.selection = rows.get(index + 1).map(|child| child.key().clone());
            }
            HorizontalDirection::Left if row.is_group() && row.is_expanded() => {
                self.collapse(&selection);
                self.selection = Some(selection);
            }
            HorizontalDirection::Left => {
                if let Some(parent) = row.parent_key() {
                    self.selection = Some(parent.clone());
                }
            }
            HorizontalDirection::Right => {}
        }
    }

    /// Toggle a group or return the Session selected for opening.
    pub(in crate::ui) fn activate<'a>(&mut self, snap: &'a Snapshot) -> Option<&'a Session> {
        let selection = self.effective_selection(snap)?;
        match selection {
            ProjectTreeKey::Session(identity) => find_session(snap, &identity),
            group => {
                self.toggle_group(group);
                None
            }
        }
    }

    pub(in crate::ui) fn selected_session<'a>(&self, snap: &'a Snapshot) -> Option<&'a Session> {
        let ProjectTreeKey::Session(identity) = self.effective_selection(snap)? else {
            return None;
        };
        find_session(snap, &identity)
    }

    pub(in crate::ui) fn selected_project(&self, snap: &Snapshot) -> Option<ProjectIdentity> {
        let ProjectTreeKey::Project(identity) = self.effective_selection(snap)? else {
            return None;
        };
        Some(identity)
    }

    pub(in crate::ui) fn open_sessions<'a>(&self, snap: &'a Snapshot) -> Vec<&'a Session> {
        self.rows(snap)
            .into_iter()
            .filter_map(|row| match row {
                ProjectTreeRow::Session { session, .. } => Some(session),
                _ => None,
            })
            .collect()
    }

    pub(in crate::ui) fn new_target_selection(
        &self,
        snap: &Snapshot,
    ) -> Option<NewTargetSelection> {
        let (project_identity, selected_worktree) = match self.effective_selection(snap)? {
            ProjectTreeKey::Project(project) => (project, None),
            ProjectTreeKey::Worktree { project, root } => (project, Some(root)),
            ProjectTreeKey::Disclosure {
                project: Some(project),
                worktree_root,
                ..
            } => (project, worktree_root),
            ProjectTreeKey::Session(identity) => {
                let session = find_session(snap, &identity)?;
                (
                    session.project.as_ref()?.identity.clone(),
                    session
                        .worktree
                        .as_ref()
                        .map(|worktree| worktree.root.clone()),
                )
            }
            ProjectTreeKey::Disclosure { project: None, .. } => return None,
        };
        let project = snap
            .sessions
            .iter()
            .filter_map(|session| session.project.as_ref())
            .find(|project| project.identity == project_identity)?;
        let mut worktree_roots = snap
            .sessions
            .iter()
            .filter(|session| session.project.as_ref() == Some(project))
            .filter_map(|session| {
                session
                    .worktree
                    .as_ref()
                    .map(|worktree| worktree.root.clone())
            })
            .collect::<Vec<_>>();
        worktree_roots.sort();
        worktree_roots.dedup();

        let targets = match &project.identity {
            ProjectIdentity::CanonicalCwd(root) => vec![ManagedLaunchTarget {
                project: project.identity.clone(),
                project_label: project.label.clone(),
                worktree_root: None,
                working_directory: root.clone(),
            }],
            ProjectIdentity::GitCommonDir(common_dir) => {
                let main_root =
                    git_main_worktree(common_dir).or_else(|| worktree_roots.first().cloned())?;
                if !worktree_roots.contains(&main_root) {
                    worktree_roots.push(main_root.clone());
                }
                worktree_roots.sort_by_key(|root| (root != &main_root, root.clone()));
                worktree_roots
                    .into_iter()
                    .map(|root| ManagedLaunchTarget {
                        project: project.identity.clone(),
                        project_label: project.label.clone(),
                        worktree_root: Some(root.clone()),
                        working_directory: root,
                    })
                    .collect()
            }
        };
        let selected = selected_worktree
            .as_ref()
            .and_then(|selected| {
                targets
                    .iter()
                    .position(|target| target.worktree_root.as_ref() == Some(selected))
            })
            .unwrap_or(0);
        Some(NewTargetSelection { targets, selected })
    }

    pub(in crate::ui) fn select_session(&mut self, identity: SessionIdentity) {
        self.selection = Some(ProjectTreeKey::Session(identity));
    }

    fn expand(&mut self, key: &ProjectTreeKey) {
        if key.uses_disclosure_state() {
            self.expanded_disclosures.insert(key.clone());
        } else {
            self.collapsed_nodes.remove(key);
        }
    }

    fn collapse(&mut self, key: &ProjectTreeKey) {
        if key.uses_disclosure_state() {
            self.expanded_disclosures.remove(key);
        } else {
            self.collapsed_nodes.insert(key.clone());
        }
    }

    fn toggle_group(&mut self, key: ProjectTreeKey) {
        if key.uses_disclosure_state() {
            if !self.expanded_disclosures.remove(&key) {
                self.expanded_disclosures.insert(key.clone());
            }
        } else if !self.collapsed_nodes.remove(&key) {
            self.collapsed_nodes.insert(key.clone());
        }
        self.selection = Some(key);
    }
}

fn git_main_worktree(common_dir: &std::path::Path) -> Option<PathBuf> {
    (common_dir.file_name() == Some(std::ffi::OsStr::new(".git")))
        .then(|| common_dir.parent().map(PathBuf::from))
        .flatten()
}

fn find_session<'a>(snap: &'a Snapshot, identity: &SessionIdentity) -> Option<&'a Session> {
    snap.sessions
        .iter()
        .find(|session| session.provider == identity.provider && session.id == identity.native_id)
}
