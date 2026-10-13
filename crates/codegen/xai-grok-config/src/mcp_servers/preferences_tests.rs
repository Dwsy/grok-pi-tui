use super::*;

#[test]
fn missing_and_unparseable_preferences_read_as_empty() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("mcp_preferences.json");

    assert!(matches!(
        load_mcp_preferences_from(&path),
        McpPreferencesLoad::Missing
    ));
    assert_eq!(
        McpPreferencesFile::default(),
        load_mcp_preferences_from(&path).file()
    );

    std::fs::write(&path, "not json").expect("write corrupt preferences");
    assert!(matches!(
        load_mcp_preferences_from(&path),
        McpPreferencesLoad::Corrupt
    ));
    assert_eq!(
        McpPreferencesFile::default(),
        load_mcp_preferences_from(&path).file()
    );
}
