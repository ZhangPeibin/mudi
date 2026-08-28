//! Resolve a Session's working directory into canonical Project identity.

use crate::application::handlers::ManagedTargetResolver;
use crate::application::managed_session::ManagedLaunchTarget;
use crate::domain::{Project, ProjectIdentity};
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectResolution {
    pub project: Option<Project>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalProjectContext {
    pub project: Project,
    pub working_directory: PathBuf,
}

pub struct CanonicalManagedTargetResolver;

impl ManagedTargetResolver for CanonicalManagedTargetResolver {
    fn resolve(&self, directory: &Path) -> Result<ManagedLaunchTarget> {
        let context = resolve_selected_directory(directory)?;
        Ok(ManagedLaunchTarget {
            working_directory: context.working_directory,
        })
    }
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

    if !selected_directory_is_in_git_project(&cwd)? {
        return Ok(CanonicalProjectContext {
            project: Project {
                label: path_label(&cwd),
                identity: ProjectIdentity::new(cwd.clone()),
            },
            working_directory: cwd,
        });
    }
    let root = selected_git_path(&cwd, "--show-toplevel")?;
    if !cwd.starts_with(&root) {
        return Err(anyhow!(
            "Selected directory has an ambiguous Git identity: {} is outside {}",
            cwd.display(),
            root.display()
        ));
    }
    let identity = ProjectIdentity::new(root.clone());
    let label = path_label(&root);

    Ok(CanonicalProjectContext {
        project: Project { identity, label },
        working_directory: cwd,
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

fn selected_directory_is_in_git_project(cwd: &Path) -> Result<bool> {
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
                "Selected directory has Git metadata but no working-tree identity: {}",
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

/// Resolve an observed `cwd`, suppressing refresh diagnostics for callers that
/// only need the best available Project identity.
pub fn resolve(cwd: Option<&Path>) -> ProjectResolution {
    resolve_checked(cwd).unwrap_or_default()
}

/// Resolve an observed Session cwd while preserving failures for refresh diagnostics.
/// A recorded absolute path remains historical Project evidence when the directory
/// is temporarily unavailable.
pub fn resolve_checked(cwd: Option<&Path>) -> Result<ProjectResolution> {
    let Some(recorded_cwd) = cwd else {
        return Ok(ProjectResolution::default());
    };
    let cwd = match std::fs::canonicalize(recorded_cwd) {
        Ok(cwd) => cwd,
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound && recorded_cwd.is_absolute() =>
        {
            return Ok(recorded_path_project(recorded_cwd));
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Session working directory cannot be canonicalized: {}",
                    recorded_cwd.display()
                )
            });
        }
    };
    let context = match resolve_selected_directory(&cwd) {
        Ok(context) => context,
        Err(error) if recorded_cwd.is_absolute() && selected_directory_is_unavailable(&error) => {
            return Ok(recorded_path_project(recorded_cwd));
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Session Project identity cannot be resolved: {}",
                    cwd.display()
                )
            });
        }
    };

    Ok(ProjectResolution {
        project: Some(context.project),
    })
}

fn selected_directory_is_unavailable(error: &anyhow::Error) -> bool {
    error
        .to_string()
        .starts_with("Selected directory is unreadable:")
        && error.chain().any(|cause| {
            cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
                matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                )
            })
        })
}

fn recorded_path_project(cwd: &Path) -> ProjectResolution {
    ProjectResolution {
        project: Some(Project {
            identity: ProjectIdentity::new(cwd.to_path_buf()),
            label: path_label(cwd),
        }),
    }
}

fn path_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
