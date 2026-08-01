use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use sessionmesh::collector::build_sessions;
use sessionmesh::config::Config;
use sessionmesh::domain::{ContextState, ProjectIdentity, Provider, Usage};
use sessionmesh::index::{Fingerprint, Index};
use sessionmesh::pricing::RateCard;
use sessionmesh::project::resolve;
use sessionmesh::providers::ParsedSession;

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let suffix = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sessionmesh-project-resolution-{}-{suffix}",
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
            "user.name=SessionMesh Tests",
            "-c",
            "user.email=sessionmesh@example.invalid",
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
fn linked_worktrees_share_project_identity_and_keep_worktree_metadata() {
    let temp = TempDir::new();
    let repository = temp.path().join("sessionmesh");
    let linked = temp.path().join("sessionmesh-feature");
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

    assert_eq!(
        main.project.as_ref().unwrap().identity,
        feature.project.as_ref().unwrap().identity
    );

    let main_worktree = main.worktree.unwrap();
    let feature_worktree = feature.worktree.unwrap();
    assert_ne!(main_worktree.root, feature_worktree.root);
    assert_eq!(main_worktree.branch.as_deref(), Some("main"));
    assert_eq!(feature_worktree.branch.as_deref(), Some("feature/identity"));
    assert!(main_worktree.writable);
    assert!(feature_worktree.writable);
}

#[test]
fn non_git_directory_is_a_standalone_project_without_a_worktree() {
    let temp = TempDir::new();
    let notes = temp.path().join("research-notes");
    fs::create_dir_all(&notes).unwrap();

    let resolved = resolve(Some(&notes));

    assert_eq!(
        resolved.project.unwrap().identity,
        ProjectIdentity::CanonicalCwd(fs::canonicalize(&notes).unwrap())
    );
    assert!(resolved.worktree.is_none());
}

#[test]
fn independent_clones_remain_distinct_projects_with_the_same_remote() {
    let temp = TempDir::new();
    let origin = temp.path().join("origin");
    let first_parent = temp.path().join("first");
    let second_parent = temp.path().join("second");
    let first = first_parent.join("sessionmesh");
    let second = second_parent.join("sessionmesh");
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
    assert_eq!(first.label, "sessionmesh");
    assert_eq!(second.label, "sessionmesh");
}

#[test]
fn unavailable_cwd_does_not_invent_project_or_worktree_identity() {
    let temp = TempDir::new();
    let missing = temp.path().join("does-not-exist");

    let resolved = resolve(Some(&missing));

    assert!(resolved.project.is_none());
    assert!(resolved.worktree.is_none());
}

#[test]
fn symlink_and_nested_path_spellings_resolve_to_the_same_identities() {
    let temp = TempDir::new();
    let repository = temp.path().join("sessionmesh");
    let nested = repository.join("src/domain");
    let alias = temp.path().join("sessionmesh-alias");
    repository_with_commit(&repository);
    fs::create_dir_all(&nested).unwrap();
    std::os::unix::fs::symlink(&repository, &alias).unwrap();

    let direct = resolve(Some(&nested));
    let through_alias = resolve(Some(&alias.join("src/../src/domain")));

    assert_eq!(
        direct.project.unwrap().identity,
        through_alias.project.unwrap().identity
    );
    assert_eq!(
        direct.worktree.unwrap().root,
        through_alias.worktree.unwrap().root
    );
}

#[test]
fn snapshot_carries_resolved_identity_and_disambiguates_distinct_projects() {
    let temp = TempDir::new();
    let origin = temp.path().join("origin");
    let first_parent = temp.path().join("first");
    let second_parent = temp.path().join("second");
    let first = first_parent.join("sessionmesh");
    let second = second_parent.join("sessionmesh");
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

    let sessions = build_sessions(&cfg, &index, &RateCard::load(None));
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
    assert_eq!(first_a.project.as_ref().unwrap().label, "first/sessionmesh");
    assert_eq!(first_b.project.as_ref().unwrap().label, "first/sessionmesh");
    assert_eq!(second.project.as_ref().unwrap().label, "second/sessionmesh");
    assert_eq!(
        first_a.worktree.as_ref().unwrap().root,
        fs::canonicalize(&first).unwrap()
    );
    assert_eq!(
        first_a.worktree.as_ref().unwrap().branch.as_deref(),
        Some("main")
    );
}
