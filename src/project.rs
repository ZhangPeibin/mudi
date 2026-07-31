//! Resolve a Session's working directory into canonical Project and Worktree identity.

use crate::domain::{Project, ProjectIdentity, Worktree};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectResolution {
    pub project: Option<Project>,
    pub worktree: Option<Worktree>,
}

/// Resolve `cwd` without reading repository contents. Unresolvable paths return
/// unavailable identity rather than guessing.
pub fn resolve(cwd: Option<&Path>) -> ProjectResolution {
    let Some(cwd) = cwd.and_then(|path| std::fs::canonicalize(path).ok()) else {
        return ProjectResolution::default();
    };
    let Some(common_dir) = git_path(&cwd, "--git-common-dir") else {
        return ProjectResolution {
            project: Some(Project {
                label: path_label(&cwd),
                identity: ProjectIdentity::CanonicalCwd(cwd),
            }),
            worktree: None,
        };
    };
    let Some(root) = git_path(&cwd, "--show-toplevel") else {
        return ProjectResolution::default();
    };

    let identity = ProjectIdentity::GitCommonDir(common_dir);
    let label = path_label(identity.display_path());

    ProjectResolution {
        project: Some(Project { identity, label }),
        worktree: Some(Worktree {
            writable: is_writable(&root),
            branch: git_stdout(&cwd, &["symbolic-ref", "--short", "HEAD"]),
            root,
        }),
    }
}

fn git_path(cwd: &Path, query: &str) -> Option<PathBuf> {
    let output = git_stdout(cwd, &["rev-parse", "--path-format=absolute", query])?;
    std::fs::canonicalize(output).ok()
}

fn git_stdout(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn is_writable(path: &Path) -> bool {
    Command::new("test")
        .arg("-w")
        .arg(path)
        .status()
        .is_ok_and(|status| status.success())
}

fn path_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
