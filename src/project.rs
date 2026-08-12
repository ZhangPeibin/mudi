//! Resolve a Session's working directory into canonical Project and Worktree identity.

use crate::domain::{Project, ProjectIdentity, Worktree};
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectResolution {
    pub project: Option<Project>,
    pub worktree: Option<Worktree>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalProjectContext {
    pub project: Project,
    pub worktree: Option<Worktree>,
}

/// Resolve an explicitly selected local directory without guessing identity.
pub fn resolve_selected_directory(cwd: &Path) -> Result<CanonicalProjectContext> {
    let cwd = std::fs::canonicalize(cwd).with_context(|| {
        format!(
            "Selected directory cannot be canonicalized: {}",
            cwd.display()
        )
    })?;
    if !cwd.is_dir() {
        return Err(anyhow!(
            "Selected path is not a directory: {}",
            cwd.display()
        ));
    }
    std::fs::read_dir(&cwd)
        .with_context(|| format!("Selected directory is unreadable: {}", cwd.display()))?;

    if !selected_directory_is_in_git_worktree(&cwd)? {
        return Ok(CanonicalProjectContext {
            project: Project {
                label: path_label(&cwd),
                identity: ProjectIdentity::CanonicalCwd(cwd),
            },
            worktree: None,
        });
    }
    let common_dir = selected_git_path(&cwd, "--git-common-dir")?;
    let root = selected_git_path(&cwd, "--show-toplevel")?;
    if !cwd.starts_with(&root) {
        return Err(anyhow!(
            "Selected directory has an ambiguous Git identity: {} is outside {}",
            cwd.display(),
            root.display()
        ));
    }
    let identity = ProjectIdentity::GitCommonDir(common_dir);
    let label = path_label(identity.display_path());

    Ok(CanonicalProjectContext {
        project: Project { identity, label },
        worktree: Some(Worktree {
            writable: is_writable(&root),
            branch: git_stdout(&cwd, &["symbolic-ref", "--short", "HEAD"]),
            root,
        }),
    })
}

fn selected_git_path(cwd: &Path, query: &str) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--path-format=absolute", query])
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .output()
        .context("Selected directory Git identity cannot be inspected")?;
    if !output.status.success() {
        return Err(anyhow!(
            "Selected directory has an ambiguous Git identity: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let value = git_output_path(output.stdout)?;
    if !value.is_absolute() {
        return Err(anyhow!(
            "Selected directory has an ambiguous Git identity: Git returned a relative path"
        ));
    }
    std::fs::canonicalize(&value).with_context(|| {
        format!(
            "Selected directory has a non-canonical Git identity: {}",
            value.display()
        )
    })
}

fn git_output_path(mut output: Vec<u8>) -> Result<PathBuf> {
    if output.last() == Some(&b'\n') {
        output.pop();
        if output.last() == Some(&b'\r') {
            output.pop();
        }
    }
    if output.is_empty() || output.contains(&0) {
        return Err(anyhow!("Selected directory has an ambiguous Git identity"));
    }

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(output)))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(String::from_utf8(output).context(
            "Selected directory Git identity is not valid UTF-8",
        )?))
    }
}

fn selected_directory_is_in_git_worktree(cwd: &Path) -> Result<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--is-inside-work-tree"])
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .output()
        .context("Selected directory Git identity cannot be inspected")?;
    if output.status.success() {
        return match output.stdout.as_slice() {
            b"true\n" | b"true\r\n" => Ok(true),
            b"false\n" | b"false\r\n" => Err(anyhow!(
                "Selected directory has Git metadata but no Worktree identity: {}",
                cwd.display()
            )),
            _ => Err(anyhow!(
                "Selected directory has an ambiguous Git identity: {}",
                cwd.display()
            )),
        };
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("not a git repository") {
        if has_git_marker(cwd)? {
            return Err(anyhow!(
                "Selected directory has an ambiguous Git identity: {}",
                cwd.display()
            ));
        }
        return Ok(false);
    }

    Err(anyhow!(
        "Selected directory Git identity cannot be resolved: {}",
        stderr.trim()
    ))
}

fn has_git_marker(cwd: &Path) -> Result<bool> {
    for ancestor in cwd.ancestors() {
        match std::fs::symlink_metadata(ancestor.join(".git")) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Selected directory Git identity cannot be inspected at {}",
                        ancestor.display()
                    )
                });
            }
        }
    }
    Ok(false)
}

/// Resolve `cwd` without reading repository contents. Unresolvable paths return
/// unavailable identity rather than guessing.
pub fn resolve(cwd: Option<&Path>) -> ProjectResolution {
    resolve_checked(cwd).unwrap_or_default()
}

/// Resolve an observed Session cwd while preserving failures for refresh diagnostics.
/// A directory that no longer exists is normal historical evidence and remains unresolved.
pub fn resolve_checked(cwd: Option<&Path>) -> Result<ProjectResolution> {
    let Some(cwd) = cwd else {
        return Ok(ProjectResolution::default());
    };
    let cwd = match std::fs::canonicalize(cwd) {
        Ok(cwd) => cwd,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProjectResolution::default());
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Session working directory cannot be canonicalized: {}",
                    cwd.display()
                )
            });
        }
    };
    let context = resolve_selected_directory(&cwd).with_context(|| {
        format!(
            "Session Project or Worktree identity cannot be resolved: {}",
            cwd.display()
        )
    })?;

    Ok(ProjectResolution {
        project: Some(context.project),
        worktree: context.worktree,
    })
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
