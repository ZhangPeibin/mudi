use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use mudi::adapters::config::Config;
use mudi::adapters::models::ModelCatalog;
use mudi::adapters::observation::{
    build_sessions, build_sessions_with_resolver, ProjectResolverCache,
};
use mudi::adapters::persistence::index::{Fingerprint, Index};
use mudi::adapters::project::{resolve, resolve_selected_directory};
use mudi::adapters::providers::ParsedSession;
use mudi::domain::{ContextState, ProjectIdentity, Provider, Usage};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let suffix = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mudi-project-resolution-{}-{suffix}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repository_with_commit(path: &Path) {
    fs::create_dir_all(path).unwrap();
    git(path, &["init", "-b", "main"]);
    fs::write(path.join("README.md"), "fixture\n").unwrap();
    git(path, &["add", "README.md"]);
    git(
        path,
        &[
            "-c",
            "user.name=Mudi Tests",
            "-c",
            "user.email=mudi@example.invalid",
            "commit",
            "-m",
            "fixture",
        ],
    );
}

fn parsed_session(id: &str, cwd: &Path) -> ParsedSession {
    ParsedSession {
        provider: Provider::ClaudeCode,
        id: id.to_string(),
        cwd: Some(cwd.to_string_lossy().into_owned()),
        latest_model: None,
        context: ContextState::default(),
        usage: Usage::default(),
        per_model: vec![],
        compactions: vec![],
        last_activity_ms: 0,
    }
}

#[test]
fn linked_git_working_directories_are_independent_projects() {
    let temp = TempDir::new();
    let repository = temp.path().join("mudi");
    let linked = temp.path().join("mudi-feature");
    repository_with_commit(&repository);
    git(
        &repository,
        &[
            "worktree",
            "add",
            "-b",
            "feature/identity",
            linked.to_str().unwrap(),
        ],
    );

    let main = resolve(Some(&repository));
    let feature = resolve(Some(&linked));

    assert_ne!(
        main.project.as_ref().unwrap().identity,
        feature.project.as_ref().unwrap().identity
    );
    assert_eq!(
        main.project.as_ref().unwrap().identity,
        ProjectIdentity::new(fs::canonicalize(&repository).unwrap())
    );
    assert_eq!(
        feature.project.as_ref().unwrap().identity,
        ProjectIdentity::new(fs::canonicalize(&linked).unwrap())
    );
}

#[test]
fn selected_git_subdirectory_resolves_project_root_and_preserves_working_directory() {
    let temp = TempDir::new();
    let repository = temp.path().join("mudi");
    let nested = repository.join("src/domain");
    repository_with_commit(&repository);
    fs::create_dir_all(&nested).unwrap();

    let resolved = resolve_selected_directory(&nested).unwrap();

    assert_eq!(
        resolved.project.identity,
        ProjectIdentity::new(fs::canonicalize(&repository).unwrap())
    );
    assert_eq!(
        resolved.working_directory,
        fs::canonicalize(nested).unwrap()
    );
}

#[test]
fn selected_directory_with_broken_git_metadata_is_rejected_as_ambiguous() {
    let temp = TempDir::new();
    let directory = temp.path().join("broken-git-directory");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join(".git"), "gitdir: ../missing-git-dir\n").unwrap();

    let error = resolve_selected_directory(&directory).unwrap_err();

    assert!(
        error.to_string().contains("ambiguous Git identity"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn selected_non_git_directory_becomes_its_own_canonical_project() {
    let temp = TempDir::new();
    let directory = temp.path().join("notes");
    fs::create_dir(&directory).unwrap();

    let resolved = resolve_selected_directory(&directory).unwrap();
    let canonical = fs::canonicalize(&directory).unwrap();

    assert_eq!(resolved.project.identity, ProjectIdentity::new(canonical));
    assert_eq!(resolved.project.label, "notes");
}

#[test]
fn selected_non_canonical_directory_is_rejected_without_guessing() {
    let temp = TempDir::new();
    let missing = temp.path().join("missing");

    let error = resolve_selected_directory(&missing).unwrap_err();

    assert!(
        error.to_string().contains("cannot be canonicalized"),
        "unexpected error: {error:#}"
    );
}

#[cfg(unix)]
#[test]
fn selected_unreadable_directory_is_rejected() {
    let temp = TempDir::new();
    let directory = temp.path().join("unreadable");
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o000)).unwrap();

    let result = resolve_selected_directory(&directory);

    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let error = result.unwrap_err();
    assert!(
        error.to_string().contains("unreadable"),
        "unexpected error: {error:#}"
    );
}

#[cfg(unix)]
#[test]
fn unreadable_recorded_absolute_cwd_remains_a_project() {
    let temp = TempDir::new();
    let directory = temp.path().join("unreadable-observed-cwd");
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o000)).unwrap();

    let resolved = resolve(Some(&directory));

    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        resolved.project.unwrap().identity,
        ProjectIdentity::new(directory)
    );
}

#[test]
fn non_git_directory_is_a_standalone_project() {
    let temp = TempDir::new();
    let notes = temp.path().join("research-notes");
    fs::create_dir_all(&notes).unwrap();

    let resolved = resolve(Some(&notes));

    assert_eq!(
        resolved.project.unwrap().identity,
        ProjectIdentity::new(fs::canonicalize(&notes).unwrap())
    );
}

#[test]
fn independent_clones_remain_distinct_projects_with_the_same_remote() {
    let temp = TempDir::new();
    let origin = temp.path().join("origin");
    let first_parent = temp.path().join("first");
    let second_parent = temp.path().join("second");
    let first = first_parent.join("mudi");
    let second = second_parent.join("mudi");
    repository_with_commit(&origin);
    fs::create_dir_all(&first_parent).unwrap();
    fs::create_dir_all(&second_parent).unwrap();
    git(
        temp.path(),
        &["clone", origin.to_str().unwrap(), first.to_str().unwrap()],
    );
    git(
        temp.path(),
        &["clone", origin.to_str().unwrap(), second.to_str().unwrap()],
    );

    let first = resolve(Some(&first)).project.unwrap();
    let second = resolve(Some(&second)).project.unwrap();

    assert_ne!(first.identity, second.identity);
    assert_eq!(first.label, "mudi");
    assert_eq!(second.label, "mudi");
}

#[test]
fn unavailable_recorded_absolute_cwd_remains_a_project() {
    let temp = TempDir::new();
    let missing = temp.path().join("does-not-exist");

    let resolved = resolve(Some(&missing));

    assert_eq!(
        resolved.project.unwrap().identity,
        ProjectIdentity::new(missing)
    );
}

#[test]
fn symlink_and_nested_path_spellings_resolve_to_the_same_identities() {
    let temp = TempDir::new();
    let repository = temp.path().join("mudi");
    let nested = repository.join("src/domain");
    let alias = temp.path().join("mudi-alias");
    repository_with_commit(&repository);
    fs::create_dir_all(&nested).unwrap();
    std::os::unix::fs::symlink(&repository, &alias).unwrap();

    let direct = resolve(Some(&nested));
    let through_alias = resolve(Some(&alias.join("src/../src/domain")));

    assert_eq!(
        direct.project.unwrap().identity,
        through_alias.project.unwrap().identity
    );
}

#[test]
fn snapshot_carries_resolved_identity_and_disambiguates_distinct_projects() {
    let temp = TempDir::new();
    let origin = temp.path().join("origin");
    let first_parent = temp.path().join("first");
    let second_parent = temp.path().join("second");
    let first = first_parent.join("mudi");
    let second = second_parent.join("mudi");
    repository_with_commit(&origin);
    fs::create_dir_all(&first_parent).unwrap();
    fs::create_dir_all(&second_parent).unwrap();
    git(
        temp.path(),
        &["clone", origin.to_str().unwrap(), first.to_str().unwrap()],
    );
    git(
        temp.path(),
        &["clone", origin.to_str().unwrap(), second.to_str().unwrap()],
    );

    let mut index = Index::open_in_memory().unwrap();
    for (position, (id, cwd)) in [
        ("first-a", first.as_path()),
        ("first-b", first.as_path()),
        ("second", second.as_path()),
    ]
    .into_iter()
    .enumerate()
    {
        index
            .upsert(
                &parsed_session(id, cwd),
                &temp.path().join(format!("{id}.jsonl")),
                Fingerprint {
                    mtime_ns: position as i64,
                    size: 1,
                },
            )
            .unwrap();
    }
    let cfg = Config {
        claude_dir: temp.path().join("claude"),
        codex_dir: temp.path().join("codex"),
        data_dir: temp.path().join("data"),
        extra_dirs: vec![],
    };

    let sessions = build_sessions(&cfg, &index, &ModelCatalog::load(None));
    let first_a = sessions
        .iter()
        .find(|session| session.id == "first-a")
        .unwrap();
    let first_b = sessions
        .iter()
        .find(|session| session.id == "first-b")
        .unwrap();
    let second = sessions
        .iter()
        .find(|session| session.id == "second")
        .unwrap();

    assert_eq!(first_a.project, first_b.project);
    assert_ne!(
        first_a.project.as_ref().unwrap().identity,
        second.project.as_ref().unwrap().identity
    );
    assert_eq!(first_a.project.as_ref().unwrap().label, "first/mudi");
    assert_eq!(first_b.project.as_ref().unwrap().label, "first/mudi");
    assert_eq!(second.project.as_ref().unwrap().label, "second/mudi");
    assert_eq!(
        first_a.project.as_ref().unwrap().identity.display_path(),
        fs::canonicalize(&first).unwrap()
    );
}

#[test]
fn project_resolution_observes_git_state_changes_on_each_refresh() {
    let temp = TempDir::new();
    let project_directory = temp.path().join("cached-project");
    let working_directory = project_directory.join("nested");
    fs::create_dir_all(&working_directory).unwrap();
    let source_path = temp.path().join("cached-session.jsonl");
    let mut index = Index::open_in_memory().unwrap();
    index
        .upsert(
            &parsed_session("cached-session", &working_directory),
            &source_path,
            Fingerprint {
                mtime_ns: 1,
                size: 1,
            },
        )
        .unwrap();
    let cfg = Config {
        claude_dir: temp.path().join("claude"),
        codex_dir: temp.path().join("codex"),
        data_dir: temp.path().join("data"),
        extra_dirs: vec![],
    };
    let models = ModelCatalog::load(None);
    let mut resolver = ProjectResolverCache::default();

    let first = build_sessions_with_resolver(&cfg, &index, &models, &mut resolver).unwrap();
    assert_eq!(
        first.sessions[0]
            .project
            .as_ref()
            .unwrap()
            .identity
            .display_path(),
        fs::canonicalize(&working_directory).unwrap()
    );

    repository_with_commit(&project_directory);
    let initialized = build_sessions_with_resolver(&cfg, &index, &models, &mut resolver).unwrap();
    assert_eq!(
        initialized.sessions[0]
            .project
            .as_ref()
            .unwrap()
            .identity
            .display_path(),
        fs::canonicalize(&project_directory).unwrap()
    );

    fs::remove_dir_all(project_directory.join(".git")).unwrap();
    let removed = build_sessions_with_resolver(&cfg, &index, &models, &mut resolver).unwrap();
    assert_eq!(
        removed.sessions[0]
            .project
            .as_ref()
            .unwrap()
            .identity
            .display_path(),
        fs::canonicalize(&working_directory).unwrap()
    );
}
