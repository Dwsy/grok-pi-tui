use std::path::Path;
use std::time::{Duration, Instant};

use xai_grok_markdown::Syntect;

#[test]
fn readme_edit_links_keep_syntax_colors_without_seconds_of_regex_compilation() {
    let syntect = Syntect::new(include_bytes!("../assets/tokyo-night.tmTheme"));
    // The actual navigation row from the README edit that froze the TUI.
    let old = "[Download latest release](https://github.com/Dwsy/grok-pi/releases/latest) · [ZH](docs/README.zh-CN.md) · [Feature matrix](docs/FEATURE_MATRIX.md) · [Architecture](docs/NATIVE_GROK_TUI_ALIGNMENT.md) · [Verification](docs/VERIFICATION.md) · [Changelog](CHANGELOG.MD) · [更新日志](docs/CHANGELOG.zh-CN.md)";
    let new = old.replace("Download latest release", "Get latest release");
    let started = Instant::now();

    for text in [old, new.as_str()] {
        let mut highlighter = syntect
            .highlight_lines_by_file_path(Path::new("README.md"))
            .expect("Markdown syntax");
        let input = format!("{text}\n");
        let spans = highlighter
            .highlight_line(&input, &syntect.syntax_set)
            .expect("valid Markdown highlighting");
        assert_eq!(
            spans.iter().map(|(_, text)| *text).collect::<String>(),
            input,
            "syntax highlighting must not change the diff text",
        );
        assert!(
            spans.len() > 10,
            "keep actual Markdown syntax spans instead of bypassing highlighting",
        );
    }

    let elapsed = started.elapsed();
    eprintln!("README old/new syntax passes: {elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(1),
        "README links must not spend seconds compiling syntax regexes: {elapsed:?}",
    );
}
