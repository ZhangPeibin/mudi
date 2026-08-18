use mudi::adapters::config::Config;
use mudi::adapters::persistence::index::Index;
use mudi::adapters::providers::claude;
use mudi::application::session_labels::{CurationDisposition, SessionLabels};
use mudi::domain::{Provider, SessionIdentity};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

fn temp_label_store(test_name: &str) -> (PathBuf, PathBuf) {
    let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "mudi-{test_name}-{}-{sequence}",
        std::process::id()
    ));
    let path = directory.join("session-labels.json");
    (directory, path)
}

#[test]
fn user_alias_survives_a_label_store_reload() {
    let (directory, path) = temp_label_store("alias-reload");
    let mut labels = SessionLabels::load(&path);

    labels
        .set_alias(Provider::ClaudeCode, "claude-session", "Auth fix")
        .unwrap();

    let reloaded = SessionLabels::load(&path);
    assert_eq!(
        reloaded.alias(Provider::ClaudeCode, "claude-session"),
        Some("Auth fix")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn launch_alias_association_is_idempotent_and_preserves_the_first_alias() {
    let (_directory, path) = temp_label_store("launch-alias-association");
    let mut labels = SessionLabels::load(&path);

    assert!(labels
        .associate_alias(Provider::ClaudeCode, "claude-session", "Launch title")
        .unwrap());
    let associated_bytes = fs::read(&path).unwrap();

    assert!(!labels
        .associate_alias(
            Provider::ClaudeCode,
            "claude-session",
            "Conflicting launch title",
        )
        .unwrap());

    assert_eq!(fs::read(&path).unwrap(), associated_bytes);
    assert_eq!(
        SessionLabels::load(&path).alias(Provider::ClaudeCode, "claude-session"),
        Some("Launch title")
    );
}

#[test]
fn hidden_disposition_survives_reload_and_is_provider_scoped() {
    let (directory, path) = temp_label_store("hidden-provider-identity");
    let mut labels = SessionLabels::load(&path);
    let claude = SessionIdentity {
        provider: Provider::ClaudeCode,
        native_id: "shared-native-id".into(),
    };

    labels.hide_sessions(std::slice::from_ref(&claude)).unwrap();

    let reloaded = SessionLabels::load(&path);
    assert_eq!(
        reloaded.disposition(Provider::ClaudeCode, "shared-native-id"),
        Some(CurationDisposition::Hidden)
    );
    assert_eq!(
        reloaded.disposition(Provider::CodexCli, "shared-native-id"),
        None
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn restore_and_permanent_exclusion_have_distinct_atomic_metadata_effects() {
    let (directory, path) = temp_label_store("curation-actions");
    let mut labels = SessionLabels::load(&path);
    let restored = SessionIdentity {
        provider: Provider::ClaudeCode,
        native_id: "restore-me".into(),
    };
    let deleted = SessionIdentity {
        provider: Provider::CodexCli,
        native_id: "exclude-me".into(),
    };
    labels
        .set_metadata(
            deleted.provider,
            &deleted.native_id,
            "Disposable title",
            "Disposable description",
        )
        .unwrap();
    labels
        .hide_sessions(&[restored.clone(), deleted.clone()])
        .unwrap();

    labels
        .remove_from_hide(std::slice::from_ref(&restored))
        .unwrap();
    labels
        .permanently_exclude_sessions(std::slice::from_ref(&deleted))
        .unwrap();

    let reloaded = SessionLabels::load(&path);
    assert_eq!(
        reloaded.disposition(restored.provider, &restored.native_id),
        None
    );
    assert_eq!(
        reloaded.disposition(deleted.provider, &deleted.native_id),
        Some(CurationDisposition::PermanentlyExcluded)
    );
    assert_eq!(reloaded.alias(deleted.provider, &deleted.native_id), None);
    assert_eq!(
        reloaded.description(deleted.provider, &deleted.native_id),
        None
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn permanent_exclusion_cannot_be_downgraded_or_relabelled() {
    let (directory, path) = temp_label_store("permanent-exclusion-terminal");
    let mut labels = SessionLabels::load(&path);
    let session = SessionIdentity {
        provider: Provider::CodexCli,
        native_id: "excluded-session".into(),
    };
    labels
        .permanently_exclude_sessions(std::slice::from_ref(&session))
        .unwrap();

    labels
        .hide_sessions(std::slice::from_ref(&session))
        .unwrap();
    assert!(labels
        .set_metadata(
            session.provider,
            &session.native_id,
            "Should not return",
            "Should not return",
        )
        .is_err());

    let reloaded = SessionLabels::load(&path);
    assert_eq!(
        reloaded.disposition(session.provider, &session.native_id),
        Some(CurationDisposition::PermanentlyExcluded)
    );
    assert_eq!(reloaded.alias(session.provider, &session.native_id), None);
    assert_eq!(
        reloaded.description(session.provider, &session.native_id),
        None
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn version_one_store_migrates_when_description_is_saved() {
    let (directory, path) = temp_label_store("v1-description-migration");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        &path,
        br#"{
  "version": 1,
  "aliases": [
    {"provider": "claude", "session_id": "claude-session", "alias": "Existing title"}
  ]
}"#,
    )
    .unwrap();
    let mut labels = SessionLabels::load(&path);

    assert_eq!(
        labels.alias(Provider::ClaudeCode, "claude-session"),
        Some("Existing title")
    );
    assert_eq!(
        labels.description(Provider::ClaudeCode, "claude-session"),
        None
    );
    labels
        .set_metadata(
            Provider::ClaudeCode,
            "claude-session",
            "Existing title",
            "Why this Session exists",
        )
        .unwrap();

    let reloaded = SessionLabels::load(&path);
    assert_eq!(
        reloaded.alias(Provider::ClaudeCode, "claude-session"),
        Some("Existing title")
    );
    assert_eq!(
        reloaded.description(Provider::ClaudeCode, "claude-session"),
        Some("Why this Session exists")
    );
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(written["version"], 3);

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn session_metadata_round_trips_unicode_and_empty_values_clear_both_fields() {
    let (directory, path) = temp_label_store("metadata-round-trip");
    let mut labels = SessionLabels::load(&path);

    labels
        .set_metadata(
            Provider::CodexCli,
            "codex-session",
            "登录修复",
            "解释为什么需要这个会话",
        )
        .unwrap();
    let reloaded = SessionLabels::load(&path);
    assert_eq!(
        reloaded.alias(Provider::CodexCli, "codex-session"),
        Some("登录修复")
    );
    assert_eq!(
        reloaded.description(Provider::CodexCli, "codex-session"),
        Some("解释为什么需要这个会话")
    );

    labels
        .set_metadata(Provider::CodexCli, "codex-session", "", "")
        .unwrap();
    let cleared = SessionLabels::load(&path);
    assert_eq!(cleared.alias(Provider::CodexCli, "codex-session"), None);
    assert_eq!(
        cleared.description(Provider::CodexCli, "codex-session"),
        None
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn empty_metadata_fields_clear_independently() {
    let (directory, path) = temp_label_store("metadata-independent-clear");
    let mut labels = SessionLabels::load(&path);
    labels
        .set_metadata(
            Provider::ClaudeCode,
            "claude-session",
            "Initial title",
            "Initial description",
        )
        .unwrap();

    labels
        .set_metadata(
            Provider::ClaudeCode,
            "claude-session",
            "",
            "Initial description",
        )
        .unwrap();
    assert_eq!(labels.alias(Provider::ClaudeCode, "claude-session"), None);
    assert_eq!(
        labels.description(Provider::ClaudeCode, "claude-session"),
        Some("Initial description")
    );

    labels
        .set_metadata(Provider::ClaudeCode, "claude-session", "Restored title", "")
        .unwrap();
    assert_eq!(
        labels.alias(Provider::ClaudeCode, "claude-session"),
        Some("Restored title")
    );
    assert_eq!(
        labels.description(Provider::ClaudeCode, "claude-session"),
        None
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn description_rejects_control_characters_and_more_than_240_characters() {
    let (directory, path) = temp_label_store("invalid-description");
    let mut labels = SessionLabels::load(&path);

    for description in ["line\nbreak".to_string(), "a".repeat(241)] {
        assert!(labels
            .set_metadata(
                Provider::ClaudeCode,
                "claude-session",
                "Valid title",
                &description,
            )
            .is_err());
    }
    labels
        .set_metadata(
            Provider::ClaudeCode,
            "claude-session",
            "Valid title",
            &"界".repeat(240),
        )
        .unwrap();

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn failed_metadata_save_publishes_neither_draft_in_memory() {
    let (directory, path) = temp_label_store("metadata-atomic-failure");
    let mut labels = SessionLabels::load(&path);
    labels
        .set_metadata(
            Provider::ClaudeCode,
            "claude-session",
            "Original title",
            "Original description",
        )
        .unwrap();
    std::fs::write(&path, b"broken during edit").unwrap();

    assert!(labels
        .set_metadata(
            Provider::ClaudeCode,
            "claude-session",
            "Draft title",
            "Draft description",
        )
        .is_err());
    assert_eq!(
        labels.alias(Provider::ClaudeCode, "claude-session"),
        Some("Original title")
    );
    assert_eq!(
        labels.description(Provider::ClaudeCode, "claude-session"),
        Some("Original description")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn failed_permanent_exclusion_keeps_hidden_disposition_and_session_metadata() {
    let (directory, path) = temp_label_store("permanent-exclusion-atomic-failure");
    let mut labels = SessionLabels::load(&path);
    let session = SessionIdentity {
        provider: Provider::ClaudeCode,
        native_id: "claude-session".into(),
    };
    labels
        .set_metadata(
            session.provider,
            &session.native_id,
            "Original title",
            "Original description",
        )
        .unwrap();
    labels
        .hide_sessions(std::slice::from_ref(&session))
        .unwrap();
    std::fs::write(&path, b"broken during delete").unwrap();

    assert!(labels
        .permanently_exclude_sessions(std::slice::from_ref(&session))
        .is_err());
    assert_eq!(
        labels.disposition(session.provider, &session.native_id),
        Some(CurationDisposition::Hidden)
    );
    assert_eq!(
        labels.alias(session.provider, &session.native_id),
        Some("Original title")
    );
    assert_eq!(
        labels.description(session.provider, &session.native_id),
        Some("Original description")
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"broken during delete");

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn version_two_store_defaults_to_visible_and_migrates_on_first_curation_write() {
    let (directory, path) = temp_label_store("v2-curation-migration");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        &path,
        br#"{
  "version": 2,
  "aliases": [],
  "descriptions": []
}"#,
    )
    .unwrap();
    let mut labels = SessionLabels::load(&path);
    let session = SessionIdentity {
        provider: Provider::CodexCli,
        native_id: "legacy-session".into(),
    };

    assert_eq!(
        labels.disposition(session.provider, &session.native_id),
        None
    );
    labels
        .hide_sessions(std::slice::from_ref(&session))
        .unwrap();

    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(written["version"], 3);
    assert_eq!(written["curation"][0]["disposition"], "hidden");

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn malformed_label_store_is_preserved_and_disables_editing() {
    let (directory, path) = temp_label_store("malformed");
    std::fs::create_dir_all(&directory).unwrap();
    let original = b"{not valid json";
    std::fs::write(&path, original).unwrap();

    let mut labels = SessionLabels::load(&path);

    assert!(!labels.is_editable());
    assert!(labels.error().unwrap().contains(path.to_str().unwrap()));
    assert!(labels
        .set_alias(Provider::CodexCli, "codex-session", "Auth fix")
        .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn unsupported_label_store_version_is_preserved_and_disables_editing() {
    let (directory, path) = temp_label_store("unsupported-version");
    std::fs::create_dir_all(&directory).unwrap();
    let original = br#"{"version":4,"aliases":[],"descriptions":[],"curation":[]}"#;
    std::fs::write(&path, original).unwrap();

    let labels = SessionLabels::load(&path);

    assert!(!labels.is_editable());
    assert!(labels
        .error()
        .unwrap()
        .contains("unsupported schema version 4"));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn invalid_alias_in_a_structured_store_disables_editing() {
    let (directory, path) = temp_label_store("invalid-stored-alias");
    std::fs::create_dir_all(&directory).unwrap();
    let original = br#"{
  "version": 1,
  "aliases": [
    {"provider": "claude", "session_id": "claude-session", "alias": "line\nbreak"}
  ]
}"#;
    std::fs::write(&path, original).unwrap();

    let labels = SessionLabels::load(&path);

    assert!(!labels.is_editable());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn failed_save_does_not_publish_the_alias_in_memory() {
    let (directory, _) = temp_label_store("failed-save");
    std::fs::create_dir_all(&directory).unwrap();
    let blocked_parent = directory.join("not-a-directory");
    let path = blocked_parent.join("session-labels.json");
    let mut labels = SessionLabels::load(&path);
    std::fs::write(&blocked_parent, b"block directory creation").unwrap();

    assert!(labels
        .set_alias(Provider::ClaudeCode, "claude-session", "Auth fix")
        .is_err());
    assert_eq!(labels.alias(Provider::ClaudeCode, "claude-session"), None);

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn sequential_process_saves_merge_the_latest_aliases() {
    let (directory, path) = temp_label_store("merge-latest");
    let mut first_process = SessionLabels::load(&path);
    let mut second_process = SessionLabels::load(&path);

    first_process
        .set_alias(Provider::ClaudeCode, "claude-session", "Claude work")
        .unwrap();
    second_process
        .set_alias(Provider::CodexCli, "codex-session", "Codex work")
        .unwrap();

    let reloaded = SessionLabels::load(&path);
    assert_eq!(
        reloaded.alias(Provider::ClaudeCode, "claude-session"),
        Some("Claude work")
    );
    assert_eq!(
        reloaded.alias(Provider::CodexCli, "codex-session"),
        Some("Codex work")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn sequential_process_saves_merge_metadata_for_distinct_sessions() {
    let (directory, path) = temp_label_store("merge-latest-metadata");
    let mut first_process = SessionLabels::load(&path);
    let mut second_process = SessionLabels::load(&path);

    first_process
        .set_metadata(
            Provider::ClaudeCode,
            "claude-session",
            "Claude work",
            "Investigate Claude behavior",
        )
        .unwrap();
    second_process
        .set_metadata(
            Provider::CodexCli,
            "codex-session",
            "Codex work",
            "Investigate Codex behavior",
        )
        .unwrap();

    let reloaded = SessionLabels::load(&path);
    assert_eq!(
        reloaded.description(Provider::ClaudeCode, "claude-session"),
        Some("Investigate Claude behavior")
    );
    assert_eq!(
        reloaded.description(Provider::CodexCli, "codex-session"),
        Some("Investigate Codex behavior")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn sequential_same_session_conflicts_use_the_last_successful_write() {
    let (directory, path) = temp_label_store("same-session-last-write");
    let mut first_process = SessionLabels::load(&path);
    let mut second_process = SessionLabels::load(&path);

    first_process
        .set_alias(Provider::CodexCli, "codex-session", "First")
        .unwrap();
    second_process
        .set_alias(Provider::CodexCli, "codex-session", "Second")
        .unwrap();

    assert_eq!(
        SessionLabels::load(&path).alias(Provider::CodexCli, "codex-session"),
        Some("Second")
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn new_label_store_is_owner_only() {
    let (directory, path) = temp_label_store("owner-only");
    let mut labels = SessionLabels::load(&path);

    labels
        .set_alias(Provider::ClaudeCode, "claude-session", "Private work")
        .unwrap();

    assert_eq!(
        std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn successful_save_atomically_replaces_the_label_store() {
    let (directory, path) = temp_label_store("atomic-replace");
    let mut labels = SessionLabels::load(&path);
    labels
        .set_alias(Provider::ClaudeCode, "claude-session", "First title")
        .unwrap();
    let first_inode = std::fs::metadata(&path).unwrap().ino();

    labels
        .set_alias(Provider::ClaudeCode, "claude-session", "Second title")
        .unwrap();

    assert_ne!(std::fs::metadata(&path).unwrap().ino(), first_inode);
    assert_eq!(
        SessionLabels::load(&path).alias(Provider::ClaudeCode, "claude-session"),
        Some("Second title")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn reload_recovers_after_the_label_store_is_repaired() {
    let (directory, path) = temp_label_store("reload-recovery");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(&path, b"broken").unwrap();
    let mut labels = SessionLabels::load(&path);
    assert!(!labels.is_editable());

    std::fs::write(
        &path,
        br#"{
  "version": 1,
  "aliases": [
    {"provider": "codex", "session_id": "codex-session", "alias": "Recovered work"}
  ]
}"#,
    )
    .unwrap();
    labels.reload();

    assert!(labels.is_editable());
    assert_eq!(
        labels.alias(Provider::CodexCli, "codex-session"),
        Some("Recovered work")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn clear_title_removes_only_the_target_session_alias() {
    let (directory, path) = temp_label_store("clear-title");
    let mut labels = SessionLabels::load(&path);
    labels
        .set_alias(Provider::ClaudeCode, "clear-me", "Temporary title")
        .unwrap();
    labels
        .set_alias(Provider::CodexCli, "keep-me", "Persistent title")
        .unwrap();

    labels
        .clear_title(Provider::ClaudeCode, "clear-me")
        .unwrap();

    let reloaded = SessionLabels::load(&path);
    assert_eq!(reloaded.alias(Provider::ClaudeCode, "clear-me"), None);
    assert_eq!(
        reloaded.alias(Provider::CodexCli, "keep-me"),
        Some("Persistent title")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn title_falls_back_to_the_last_eight_native_session_id_characters() {
    let labels = SessionLabels::default();

    assert_eq!(
        labels.title(Provider::CodexCli, "019fbc14-a464-7aa0-93bb-27d553d4736e"),
        "53d4736e"
    );
}

#[test]
fn rebuilding_the_disposable_index_preserves_session_metadata() {
    let (directory, _) = temp_label_store("index-rebuild");
    let config = Config {
        claude_dir: directory.join("claude"),
        codex_dir: directory.join("codex"),
        data_dir: directory.join("data"),
        extra_dirs: Vec::new(),
    };
    let mut labels = SessionLabels::load(config.label_store_path());
    labels
        .set_metadata(
            Provider::ClaudeCode,
            "claude-session",
            "Persistent title",
            "Persistent description",
        )
        .unwrap();
    labels
        .hide_sessions(&[SessionIdentity {
            provider: Provider::ClaudeCode,
            native_id: "claude-session".into(),
        }])
        .unwrap();

    drop(Index::open(&config.index_path()).unwrap());
    std::fs::remove_file(config.index_path()).unwrap();
    drop(Index::open(&config.index_path()).unwrap());

    assert_eq!(
        SessionLabels::load(config.label_store_path())
            .alias(Provider::ClaudeCode, "claude-session"),
        Some("Persistent title")
    );
    assert_eq!(
        SessionLabels::load(config.label_store_path())
            .description(Provider::ClaudeCode, "claude-session"),
        Some("Persistent description")
    );
    assert_eq!(
        SessionLabels::load(config.label_store_path())
            .disposition(Provider::ClaudeCode, "claude-session"),
        Some(CurationDisposition::Hidden)
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn claude_ai_title_records_do_not_become_session_labels() {
    let parsed = claude::parse_str(
        "claude-session-12345678",
        r#"
{"type":"ai-title","aiTitle":"Provider generated title"}
{"type":"assistant","timestamp":"2026-07-29T08:00:00.000Z","message":{"id":"a","model":"claude-opus-4-8","usage":{"input_tokens":10,"output_tokens":1}}}
"#,
    )
    .unwrap();

    assert_eq!(
        SessionLabels::default().title(parsed.provider, &parsed.id),
        "12345678"
    );
}

#[test]
fn duplicate_aliases_are_disambiguated_with_short_session_ids() {
    let mut labels = SessionLabels::default();
    labels
        .set_alias(Provider::ClaudeCode, "claude-session-11111111", "Auth fix")
        .unwrap();
    labels
        .set_alias(Provider::ClaudeCode, "claude-session-22222222", "Auth fix")
        .unwrap();

    assert_eq!(
        labels.titles_for(&[
            (Provider::ClaudeCode, "claude-session-11111111"),
            (Provider::ClaudeCode, "claude-session-22222222"),
        ]),
        vec!["Auth fix · 11111111", "Auth fix · 22222222"]
    );
}

#[test]
fn colliding_short_session_ids_expand_left_until_unique() {
    let labels = SessionLabels::default();

    assert_eq!(
        labels.titles_for(&[
            (Provider::CodexCli, "codex-Asame0000"),
            (Provider::CodexCli, "codex-Bsame0000"),
        ]),
        vec!["Asame0000", "Bsame0000"]
    );
}

#[test]
fn alias_trims_surrounding_whitespace_and_preserves_unicode() {
    let (directory, path) = temp_label_store("unicode-alias");
    let mut labels = SessionLabels::load(&path);

    labels
        .set_alias(Provider::CodexCli, "codex-session", "  登录修复  ")
        .unwrap();

    assert_eq!(
        SessionLabels::load(&path).alias(Provider::CodexCli, "codex-session"),
        Some("登录修复")
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn alias_rejects_empty_multiline_control_and_overlong_values() {
    let (directory, path) = temp_label_store("invalid-alias");
    let mut labels = SessionLabels::load(&path);
    let overlong = "a".repeat(81);

    for invalid in ["   ", "first\nsecond", "bell\u{7}", overlong.as_str()] {
        assert!(
            labels
                .set_alias(Provider::ClaudeCode, "claude-session", invalid)
                .is_err(),
            "Alias should reject {invalid:?}"
        );
    }
    labels
        .set_alias(Provider::ClaudeCode, "claude-session", &"a".repeat(80))
        .unwrap();

    std::fs::remove_dir_all(directory).unwrap();
}
