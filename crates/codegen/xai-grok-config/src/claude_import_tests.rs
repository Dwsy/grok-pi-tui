use super::{is_claude_import_marked, user_config_file};

#[test]
fn only_an_explicit_true_marker_reads_as_imported() {
    let cases = [
        (None, false),
        (Some("[other]\nkey = \"value\"\n"), false),
        (Some("[claude_compat]\nimported = false\n"), false),
        (Some("[claude_compat]\nimported = "), false),
        (Some("[claude_compat]\nimported = true\n"), true),
    ];

    for (config, expected) in cases {
        let home = tempfile::tempdir().expect("create temp grok home");
        if let Some(config) = config {
            std::fs::write(user_config_file(home.path()), config).expect("write config");
        }

        assert_eq!(expected, is_claude_import_marked(home.path()), "{config:?}");
    }
}
