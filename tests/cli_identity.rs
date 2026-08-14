use std::path::Path;

#[test]
fn cargo_builds_the_canonical_and_legacy_cli_entry_points() {
    assert_eq!(
        Path::new(env!("CARGO_BIN_EXE_mudi"))
            .file_name()
            .and_then(|name| name.to_str()),
        Some("mudi")
    );
    assert_eq!(
        Path::new(env!("CARGO_BIN_EXE_sessionmesh"))
            .file_name()
            .and_then(|name| name.to_str()),
        Some("sessionmesh")
    );
}
