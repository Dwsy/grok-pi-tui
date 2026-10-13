use super::format_rule_string;
use crate::{ImportPatternMode, ImportPermission, ImportRuleAction, ImportTool};

#[test]
fn format_rule_string_matches_claude_rule_text() {
    let cases = [
        (
            ImportTool::Bash,
            Some("npm run build"),
            ImportPatternMode::Glob,
            "Bash(npm run build)",
        ),
        (
            ImportTool::Bash,
            Some("sed"),
            ImportPatternMode::Glob,
            "Bash(sed)",
        ),
        (ImportTool::Bash, None, ImportPatternMode::Glob, "Bash"),
        (ImportTool::Any, None, ImportPatternMode::Glob, "*"),
        (
            ImportTool::Any,
            Some("src/**"),
            ImportPatternMode::Glob,
            "src/**",
        ),
        (
            ImportTool::WebFetch,
            Some("example.com"),
            ImportPatternMode::Domain,
            "WebFetch(domain:example.com)",
        ),
    ];

    for (tool, pattern, pattern_mode, expected) in cases {
        let rule = ImportPermission {
            action: ImportRuleAction::Allow,
            tool,
            pattern: pattern.map(str::to_string),
            pattern_mode,
        };

        assert_eq!(expected, format_rule_string(&rule));
    }
}
