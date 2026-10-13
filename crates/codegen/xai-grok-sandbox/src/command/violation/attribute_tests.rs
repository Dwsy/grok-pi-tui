use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{OwnTargets, shell_path_tokens};

const CWD: &str = "/opt/ws-fixture/homedir/w1-scratch/ws";
const HOME: &str = "/opt/ws-fixture/homedir";

fn argv(script: &str) -> Vec<OsString> {
    vec![OsString::from("-lc"), OsString::from(script)]
}

fn targets(script: &str, env_bases: &[PathBuf]) -> OwnTargets {
    let argv = argv(script);
    OwnTargets::of(
        Path::new(CWD),
        argv.iter().map(OsString::as_os_str),
        Path::new(CWD),
        env_bases,
        Some(Path::new(HOME)),
    )
}

#[test]
fn the_cwd_the_served_root_and_the_env_bases_are_trees() {
    let own = targets(
        "echo hi",
        &[PathBuf::from("/opt/ws-fixture/homedir/w1-scratch/pyuser")],
    );
    assert!(own.covers(Path::new(
        "/opt/ws-fixture/homedir/w1-scratch/ws/.grok/config.toml"
    )));
    assert!(own.covers(Path::new(
        "/opt/ws-fixture/homedir/w1-scratch/pyuser/lib/python3.14/site-packages/x.py"
    )));
    // A path beside the tree is not below it
    assert!(!own.covers(Path::new("/opt/ws-fixture/homedir/w1-scratch/ws2")));
    assert!(!own.covers(Path::new(
        "/opt/ws-fixture/homedir/Library/LaunchAgents/evil.plist"
    )));
}

#[test]
fn argv_path_tokens_cover_themselves_their_subtree_and_their_ancestors() {
    let own = targets(
        "mkdir -p /opt/ws-fixture/homedir/out/deep && echo hi > ../outside.txt",
        &[],
    );
    // The token, below it, above it (`mkdir -p` is refused on the first missing ancestor)
    assert!(own.covers(Path::new("/opt/ws-fixture/homedir/out/deep")));
    assert!(own.covers(Path::new("/opt/ws-fixture/homedir/out/deep/file")));
    assert!(own.covers(Path::new("/opt/ws-fixture/homedir/out")));
    // The relative redirection target, joined to the cwd and folded
    assert!(own.covers(Path::new("/opt/ws-fixture/homedir/w1-scratch/outside.txt")));
    // Siblings of a named path are not the command's
    assert!(!own.covers(Path::new("/opt/ws-fixture/homedir/out2")));
    assert!(!own.covers(Path::new("/opt/ws-fixture/homedir/w1-scratch/other.txt")));
}

#[test]
fn quoted_spans_variables_and_bare_names_read_as_a_shell_would() {
    assert_eq!(
        vec![
            "/opt/ws-fixture/homedir/Library/Application Support/MyTool".to_owned(),
            "./out.txt".to_owned(),
            "~/notes".to_owned(),
            "/etc/x".to_owned(),
        ],
        shell_path_tokens(
            r#"cp '/opt/ws-fixture/homedir/Library/Application Support/MyTool' ./out.txt; cat ~/notes | python3 -c 'open("/etc/x", "w")' --target=$HOME/x plain-name .hidden"#
        )
    );
    // `~/` expands against the home when there is one; a `$VAR` never names a path
    let own = targets("touch ~/notes/today.md \"$HOME/other\"", &[]);
    assert!(own.covers(Path::new("/opt/ws-fixture/homedir/notes/today.md")));
    assert!(!own.covers(Path::new("/opt/ws-fixture/homedir/other")));
    let no_home = OwnTargets::of(
        Path::new(CWD),
        argv("touch ~/notes").iter().map(OsString::as_os_str),
        Path::new(CWD),
        &[],
        None,
    );
    assert!(!no_home.covers(Path::new("/opt/ws-fixture/homedir/notes")));
}

#[test]
fn a_denial_naming_a_path_the_command_never_named_is_not_covered() {
    // A printed denial for a persistence directory the command
    // line never mentions buys nothing
    let own = targets("cargo test -p xai-grok-sandbox", &[]);
    assert!(!own.covers(Path::new(
        "/opt/ws-fixture/homedir/Library/LaunchAgents/com.evil.plist"
    )));
    assert!(!own.covers(Path::new("/opt/ws-fixture/homedir/target/debug/deps")));
    // Non-UTF-8 arguments are skipped, not a panic
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let raw = [OsString::from_vec(vec![0x2f, 0xff, 0xfe])];
        let own = OwnTargets::of(
            Path::new(CWD),
            raw.iter().map(OsString::as_os_str),
            Path::new(CWD),
            &[],
            None,
        );
        assert!(!own.covers(Path::new("/x")));
    }
}
