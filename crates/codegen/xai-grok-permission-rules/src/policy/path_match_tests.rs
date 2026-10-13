use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::policy::CompiledPolicy;
use crate::rules::parse_permission_rule;
use crate::types::{AccessKind, Decision, PermissionConfig, PermissionRule, RuleAction};

#[test]
fn symlinked_cwd_rules_match_every_spelling_of_a_workspace_path() {
    let fixture = SymlinkedWorkspace::new();
    let cwd = fixture.cwd();
    let ws = fixture.physical_ws();
    let notes = path_string(&ws.join("notes.toml"));
    let logical_notes = path_string(&cwd.join("notes.toml"));
    let physical_ws_glob = format!("{}/**", path_string(&ws));
    let read = |path: &str| AccessKind::Read(Some(path.to_owned()));
    let edit = |path: &str| AccessKind::Edit(path.to_owned());
    let cases = [
        (
            "alias read",
            vec![deny("Read(notes.toml)")],
            read("alias"),
            reject("read", "notes.toml"),
        ),
        (
            "physical edit",
            vec![deny("Edit(notes.toml)")],
            edit(&notes),
            reject("edit", "notes.toml"),
        ),
        (
            "physical edit through sub/..",
            vec![deny("Edit(notes.toml)")],
            edit(&path_string(&ws.join("sub/../notes.toml"))),
            reject("edit", "notes.toml"),
        ),
        (
            "new file spelled physically",
            vec![deny("Edit(new.toml)")],
            edit(&path_string(&ws.join("new.toml"))),
            reject("edit", "new.toml"),
        ),
        (
            "grep physical path",
            vec![deny("Read(notes.toml)")],
            AccessKind::Grep {
                path: Some(notes.clone()),
                glob: None,
            },
            reject("read", "notes.toml"),
        ),
        (
            "ask alias",
            vec![ask("Read(notes.toml)")],
            read("alias"),
            Some(Decision::Ask),
        ),
        (
            "../ws from symlinked cwd",
            vec![deny("Read(notes.toml)")],
            read("../ws/notes.toml"),
            reject("read", "notes.toml"),
        ),
        (
            "logical absolute deny, alias",
            vec![deny(&format!("Read({logical_notes})"))],
            read("alias"),
            reject("read", &logical_notes),
        ),
        (
            "logical absolute deny, physical",
            vec![deny(&format!("Read({logical_notes})"))],
            read(&notes),
            reject("read", &logical_notes),
        ),
        (
            "relative deny beats physical absolute allow",
            vec![
                allow(&format!("Edit({physical_ws_glob})")),
                deny("Edit(notes.toml)"),
            ],
            edit(&notes),
            reject("edit", "notes.toml"),
        ),
        (
            "physical allow",
            vec![allow("Edit(./**)")],
            edit(&notes),
            Some(Decision::Allow),
        ),
        (
            "allow stops at physical sibling",
            vec![allow("Edit(./**)")],
            edit(&path_string(&fixture.physical_real().join("other.toml"))),
            None,
        ),
        (
            "allow stops at physical sibling through ws/..",
            vec![allow("Edit(./**)")],
            edit(&path_string(&ws.join("../other.toml"))),
            None,
        ),
        (
            "allow ignores .. escape, relative",
            vec![allow("Edit(./**)")],
            edit("../../b/c/real/ws/x"),
            None,
        ),
        (
            "allow ignores .. escape, absolute",
            vec![allow("Edit(./**)")],
            edit(&path_string(&cwd.join("../../b/c/real/ws/x"))),
            None,
        ),
    ];

    let expected: Vec<_> = cases
        .iter()
        .map(|(name, _, _, decision)| (*name, decision.clone()))
        .collect();
    let actual: Vec<_> = cases
        .iter()
        .map(|(name, rules, access, _)| {
            let policy = CompiledPolicy::new(PermissionConfig::new(rules.clone()));
            (*name, policy.evaluate_with_cwd(access, Some(&cwd)))
        })
        .collect();
    assert_eq!(expected, actual);
}

#[test]
fn symlinked_cwd_shell_reads_match_relative_deny_only_for_workspace_files() {
    let fixture = SymlinkedWorkspace::new();
    let cwd = fixture.cwd();
    let policy = CompiledPolicy::new(PermissionConfig::new(vec![deny("Read(notes.toml)")]));
    let ws = fixture.physical_ws();
    let cases = [
        ("cat alias".to_owned(), reject("read", "notes.toml")),
        (
            format!("cat {}", path_string(&ws.join("notes.toml"))),
            reject("read", "notes.toml"),
        ),
        (
            format!("cat {}", path_string(&ws.join("sub/../notes.toml"))),
            reject("read", "notes.toml"),
        ),
        ("cat ../../b/c/real/ws/notes.toml".to_owned(), None),
    ];

    let expected: Vec<_> = cases
        .iter()
        .map(|(cmd, decision)| (cmd.as_str(), decision.clone()))
        .collect();
    let actual: Vec<_> = cases
        .iter()
        .map(|(cmd, _)| (cmd.as_str(), policy.evaluate_shell_file_access(cmd, &cwd)))
        .collect();
    assert_eq!(expected, actual);
}

/// `<tmp>/b/c/real/ws/{notes.toml, alias -> notes.toml, sub/}` and `<tmp>/b/c/real/other.toml`, entered as cwd `<tmp>/link/ws` through `<tmp>/link -> <tmp>/b/c/real`
struct SymlinkedWorkspace {
    _tmp: TempDir,
    root: PathBuf,
}

impl SymlinkedWorkspace {
    fn new() -> SymlinkedWorkspace {
        let tmp = tempfile::tempdir().expect("create tempdir");
        // The canonical root keeps `link` the only symlink, so `..` counts match on every OS
        let root = dunce::canonicalize(tmp.path()).expect("canonicalize tempdir");
        let real = root.join("b/c/real");
        std::fs::create_dir_all(real.join("ws/sub")).expect("create real workspace");
        std::fs::write(real.join("other.toml"), b"beta = 2\n").expect("write other.toml");
        std::fs::write(real.join("ws/notes.toml"), b"alpha = 1\n").expect("write notes.toml");
        symlink("notes.toml", real.join("ws/alias")).expect("link alias to notes.toml");
        symlink(&real, root.join("link")).expect("link cwd parent to real");
        SymlinkedWorkspace { _tmp: tmp, root }
    }

    fn cwd(&self) -> PathBuf {
        self.root.join("link/ws")
    }

    fn physical_real(&self) -> PathBuf {
        self.root.join("b/c/real")
    }

    fn physical_ws(&self) -> PathBuf {
        self.physical_real().join("ws")
    }
}

fn deny(rule: &str) -> PermissionRule {
    parse_permission_rule(rule, RuleAction::Deny).expect("parse deny rule")
}

fn ask(rule: &str) -> PermissionRule {
    parse_permission_rule(rule, RuleAction::Ask).expect("parse ask rule")
}

fn allow(rule: &str) -> PermissionRule {
    parse_permission_rule(rule, RuleAction::Allow).expect("parse allow rule")
}

fn reject(tool: &str, pattern: &str) -> Option<Decision> {
    Some(Decision::Reject(format!(
        "Denied by permission policy: deny rule on {tool} matching \"{pattern}\""
    )))
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
