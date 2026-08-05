use super::*;

#[test]
fn system_temporary_directories_are_hidden_from_provider_inventory() {
    assert!(is_system_temporary_cwd(Some("/private/tmp/session")));
    assert!(is_system_temporary_cwd(Some("/var/folders/a/b")));
    assert!(!is_system_temporary_cwd(Some("/Users/test/code/project")));
    assert!(!is_system_temporary_cwd(None));
}

#[test]
fn empty_inventory_produces_no_rows() {
    assert!(rows(&[], Provider::ClaudeCode, 1_000, &HashSet::new()).is_empty());
}
