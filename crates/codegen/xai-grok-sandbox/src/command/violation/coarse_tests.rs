use std::path::{Path, PathBuf};

use crate::command::policy::{DenyEntry, EnvPolicy, NetworkPolicy, ReadPolicy, SandboxPolicy};
use crate::command::violation::{Blocked, Capability};

use super::{
    decode_coarse, is_likely_denial, is_never_sandbox, lexical_join, nearest_existing_dir,
    stderr_tail,
};

fn workspace_policy(root: &Path) -> SandboxPolicy {
    SandboxPolicy {
        read: ReadPolicy::AllExcept { deny: Vec::new() },
        write_roots: vec![root.to_path_buf()],
        network: NetworkPolicy::Off,
        env: EnvPolicy::default(),
        protected: Vec::new(),
        build_cache_trees: Vec::new(),
        unread_git_metadata: Vec::new(),
    }
}

// Every fixture is a literal `include_str!` (no wrapping macro): gazelle resolves only literal
// paths into the Bazel target's `compile_data`, so a `concat!`-built path would compile under
// cargo and fail under Bazel.
fn write(path: &str) -> Blocked {
    Blocked::FsWrite {
        path: PathBuf::from(path),
    }
}

fn read(path: &str) -> Blocked {
    Blocked::FsRead {
        path: PathBuf::from(path),
    }
}

fn net(host: &str, port: Option<u16>) -> Blocked {
    Blocked::Net {
        host: Some(host.to_owned()),
        port,
    }
}

fn unknown(snippet: &str) -> Blocked {
    Blocked::Unknown {
        stderr_snippet: snippet.to_owned(),
    }
}

/// The scratch workspace of the macOS captures, as the fixtures spell it (README).
const CWD: &str = "/opt/ws-fixture/homedir/w1-scratch/ws";

/// `(name, fixture text, expected)`.
type Case = (&'static str, &'static str, Option<fn() -> Blocked>);

fn masked_net(port: Option<u16>) -> Blocked {
    Blocked::Net { host: None, port }
}

/// Real Seatbelt captures (`tests/fixtures/violations/README.md` has the provenance per file).
const MACOS_CASES: &[Case] = &[
    (
        "ls-ssh-eperm",
        include_str!("../../../tests/fixtures/violations/macos/ls-ssh-eperm.txt"),
        Some(|| read("/opt/ws-fixture/homedir/.ssh")),
    ),
    (
        "cat-ssh-eperm",
        include_str!("../../../tests/fixtures/violations/macos/cat-ssh-eperm.txt"),
        Some(|| read("/opt/ws-fixture/fakehome/.ssh/id_rsa")),
    ),
    (
        "sh-redirect-relative",
        include_str!("../../../tests/fixtures/violations/macos/sh-redirect-relative.txt"),
        Some(|| write("/opt/ws-fixture/homedir/w1-scratch/outside.txt")),
    ),
    (
        "touch-git-hooks",
        include_str!("../../../tests/fixtures/violations/macos/touch-git-hooks.txt"),
        Some(|| write("/opt/ws-fixture/homedir/w1-scratch/ws/.git/hooks/x")),
    ),
    (
        "mkdir-ws-grok",
        include_str!("../../../tests/fixtures/violations/macos/mkdir-ws-grok.txt"),
        Some(|| write("/opt/ws-fixture/homedir/w1-scratch/ws/.grok")),
    ),
    // `rename A to B`: the kernel refused unlinking the source
    (
        "mv-rename-root",
        include_str!("../../../tests/fixtures/violations/macos/mv-rename-root.txt"),
        Some(|| write("/opt/ws-fixture/homedir/w1-scratch/ws")),
    ),
    (
        "curl-resolve-off",
        include_str!("../../../tests/fixtures/violations/macos/curl-resolve-off.txt"),
        Some(|| net("example.com", None)),
    ),
    // `/bin/bash: connect: …` is the network, never a write to `/bin/bash`
    (
        "bash-dev-tcp-connect",
        include_str!("../../../tests/fixtures/violations/macos/bash-dev-tcp-connect.txt"),
        Some(|| masked_net(None)),
    ),
    (
        "curl-tunnel-403",
        include_str!("../../../tests/fixtures/violations/macos/curl-tunnel-403.txt"),
        None,
    ),
    (
        "curl-noproxy-ip",
        include_str!("../../../tests/fixtures/violations/macos/curl-noproxy-ip.txt"),
        Some(|| net("1.1.1.1", Some(443))),
    ),
    (
        "bash-nested-home",
        include_str!("../../../tests/fixtures/violations/macos/bash-nested-home.txt"),
        Some(|| write("/opt/ws-fixture/homedir/w1-nested-x")),
    ),
    (
        "sandbox-exec-setuid",
        include_str!("../../../tests/fixtures/violations/macos/sandbox-exec-setuid.txt"),
        Some(|| Blocked::Capability {
            what: Capability::SetUid,
        }),
    ),
    (
        "bash-kill-outside",
        include_str!("../../../tests/fixtures/violations/macos/bash-kill-outside.txt"),
        Some(|| {
            unknown(
                include_str!("../../../tests/fixtures/violations/macos/bash-kill-outside.txt")
                    .trim(),
            )
        }),
    ),
    (
        "ls-symlink-relative",
        include_str!("../../../tests/fixtures/violations/macos/ls-symlink-relative.txt"),
        Some(|| {
            unknown(
                include_str!("../../../tests/fixtures/violations/macos/ls-symlink-relative.txt")
                    .trim(),
            )
        }),
    ),
    (
        "pip-user-eperm",
        include_str!("../../../tests/fixtures/violations/macos/pip-user-eperm.txt"),
        Some(|| write("/opt/ws-fixture/homedir/w1-scratch/pyuser")),
    ),
    // A cwd-relative path into a dot directory is joined to the cwd (`decode` then flags
    // `.git/config` as protected from the policy); a bare name would still be `Unknown`
    (
        "git-config-local",
        include_str!("../../../tests/fixtures/violations/macos/git-config-local.txt"),
        Some(|| write("/opt/ws-fixture/homedir/w1-scratch/ws/.git/config")),
    ),
    (
        "git-config-global-lock",
        include_str!("../../../tests/fixtures/violations/macos/git-config-global-lock.txt"),
        Some(|| write("/opt/ws-fixture/homedir/.gitconfig")),
    ),
    (
        "mkdir-config-git",
        include_str!("../../../tests/fixtures/violations/macos/mkdir-config-git.txt"),
        Some(|| write("/opt/ws-fixture/homedir/.config/git")),
    ),
    (
        "chmod-setuid-copy",
        include_str!("../../../tests/fixtures/violations/macos/chmod-setuid-copy.txt"),
        Some(|| write("/opt/ws-fixture/homedir/w1-scratch/ws/idcopy")),
    ),
    (
        "python-getaddrinfo",
        include_str!("../../../tests/fixtures/violations/macos/python-getaddrinfo.txt"),
        Some(|| masked_net(None)),
    ),
    // The unified-log report: `deny(1) <operation> <path>` is the one Seatbelt shape that is a
    // marker, and the operation says it is a write
    (
        "seatbelt-deny-report",
        include_str!("../../../tests/fixtures/violations/macos/seatbelt-deny-report.txt"),
        Some(|| write("/private/tmp/w1-deny/P0.txt")),
    ),
];

/// Tool shapes transcribed from their source, not captured (README): the quoted-token rule and
/// the dot-directory-relative join.
const TRANSCRIBED_CASES: &[Case] = &[
    (
        "python-eperm",
        include_str!("../../../tests/fixtures/violations/transcribed/python-eperm.txt"),
        Some(|| write("/opt/ws-fixture/dev/notes.txt")),
    ),
    (
        "node-eperm",
        include_str!("../../../tests/fixtures/violations/transcribed/node-eperm.txt"),
        Some(|| write("/opt/ws-fixture/dev/notes.txt")),
    ),
    (
        "npm-cache-eperm",
        include_str!("../../../tests/fixtures/violations/transcribed/npm-cache-eperm.txt"),
        Some(|| write("/opt/ws-fixture/dev/.npm/_cacache")),
    ),
    (
        "git-hooks-relative",
        include_str!("../../../tests/fixtures/violations/transcribed/git-hooks-relative.txt"),
        Some(|| write("/opt/ws-fixture/homedir/w1-scratch/ws/.git/hooks/pre-commit")),
    ),
];

/// "Permission denied" from somewhere other than the sandbox: a remote refusing a key or
/// password and the OS refusing a socket are never a denial; an HTTP 403 or an RBAC refusal with no path is a coarse `Unknown`, which never cards. The
/// bare word `sandbox` beside a path is ordinary output.
const NEGATIVE_CASES: &[Case] = &[
    (
        "cargo-test-sandbox-word",
        include_str!("../../../tests/fixtures/violations/negative/cargo-test-sandbox-word.txt"),
        None,
    ),
    (
        "chromium-no-sandbox",
        include_str!("../../../tests/fixtures/violations/negative/chromium-no-sandbox.txt"),
        None,
    ),
    (
        "deno-permission-denied",
        include_str!("../../../tests/fixtures/violations/negative/deno-permission-denied.txt"),
        None,
    ),
    (
        "git-push-publickey",
        include_str!("../../../tests/fixtures/violations/negative/git-push-publickey.txt"),
        None,
    ),
    (
        "ssh-password",
        include_str!("../../../tests/fixtures/violations/negative/ssh-password.txt"),
        None,
    ),
    (
        "docker-socket-eacces",
        include_str!("../../../tests/fixtures/violations/negative/docker-socket-eacces.txt"),
        None,
    ),
    // The URL's `//registry.npmjs.org/pkg` is not a path token
    (
        "npm-403-url",
        include_str!("../../../tests/fixtures/violations/negative/npm-403-url.txt"),
        Some(|| {
            unknown(
                include_str!("../../../tests/fixtures/violations/negative/npm-403-url.txt").trim(),
            )
        }),
    ),
    (
        "kubectl-rbac",
        include_str!("../../../tests/fixtures/violations/negative/kubectl-rbac.txt"),
        Some(|| {
            unknown(
                include_str!("../../../tests/fixtures/violations/negative/kubectl-rbac.txt").trim(),
            )
        }),
    ),
];

#[test]
fn macos_captures_decode_to_the_expected_target() {
    let policy = workspace_policy(Path::new(CWD));
    for (name, stderr, expected) in MACOS_CASES {
        let actual = decode_coarse(stderr, Path::new(CWD), &policy);
        assert_eq!(expected.map(|f| f()), actual, "fixture macos/{name}");
    }
    for (name, stderr, expected) in TRANSCRIBED_CASES {
        let actual = decode_coarse(stderr, Path::new(CWD), &policy);
        assert_eq!(expected.map(|f| f()), actual, "fixture transcribed/{name}");
    }
}

/// The Seatbelt report is a marker only in its full shape — `deny(1)` plus the operation plus the
/// target; the operation decides read against write, whatever the program or the verbs say.
#[test]
fn the_seatbelt_report_shape_is_a_marker_and_its_operation_decides_the_access() {
    let policy = workspace_policy(Path::new(CWD));
    let report = |op: &str, target: &str| format!("Sandbox: cat(4242) deny(1) {op} {target}\n");
    assert_eq!(
        Some(read("/opt/ws-fixture/homedir/.aws/credentials")),
        decode_coarse(
            &report("file-read-data", "/opt/ws-fixture/homedir/.aws/credentials"),
            Path::new(CWD),
            &policy
        )
    );
    // `cat` is a read program, yet `file-write-data` is a write
    assert_eq!(
        Some(write("/opt/ws-fixture/homedir/.zshrc")),
        decode_coarse(
            &report("file-write-data", "/opt/ws-fixture/homedir/.zshrc"),
            Path::new(CWD),
            &policy
        )
    );
    // The network report names no host: the masked address leaves the port
    assert_eq!(
        Some(masked_net(Some(80))),
        decode_coarse(
            &report("network-outbound", "remote:*:80"),
            Path::new(CWD),
            &policy
        )
    );
    assert_eq!(
        Some(masked_net(None)),
        decode_coarse(
            &report("network-outbound", "/private/var/run/mDNSResponder"),
            Path::new(CWD),
            &policy
        )
    );
    // `deny(1)` with an operation that is not a filesystem one, or alone, is nothing; a
    // filesystem operation with no target is a marker with no path — `Unknown`, never a card
    for line in ["Sandbox: python3(77) deny(1) system-debug\n", "deny(1)\n"] {
        assert_eq!(None, decode_coarse(line, Path::new(CWD), &policy), "{line}");
    }
    let no_target = "Sandbox: bash(1) deny(1) file-write-create\n";
    assert_eq!(
        Some(unknown(no_target.trim())),
        decode_coarse(no_target, Path::new(CWD), &policy)
    );
    // Everyday output that says `sandbox` — even with a path outside the roots on the line
    assert!(!is_likely_denial(
        "     Running unittests src/lib.rs (/opt/ws-fixture/target/debug/deps/x-1)\n"
    ));
    assert!(!is_likely_denial(
        "Sandbox init failed, launching /opt/ws-fixture/Chromium without --no-sandbox\n"
    ));
}

/// A quoted path is one token, spaces and all; an unquoted one grows across a space only while
/// the spaced prefix is a directory on disk (`~/Library/Application Support`).
#[test]
fn quoted_and_spaced_paths_are_read_whole() {
    let policy = workspace_policy(Path::new(CWD));
    // Quoted: node, python, git's typographic quotes
    for (line, expected) in [
        (
            "Error: EPERM: operation not permitted, open '/opt/ws-fixture/homedir/Library/Application Support/MyTool/state.json'\n",
            "/opt/ws-fixture/homedir/Library/Application Support/MyTool/state.json",
        ),
        (
            "PermissionError: [Errno 1] Operation not permitted: '/opt/ws-fixture/homedir/My Notes/today.md'\n",
            "/opt/ws-fixture/homedir/My Notes/today.md",
        ),
        (
            "mkdir: cannot create directory ‘/opt/ws-fixture/homedir/My Notes’: Permission denied\n",
            "/opt/ws-fixture/homedir/My Notes",
        ),
        (
            "error: unable to create file \"/opt/ws-fixture/homedir/a b/c\": Operation not permitted\n",
            "/opt/ws-fixture/homedir/a b/c",
        ),
    ] {
        assert_eq!(
            Some(write(expected)),
            decode_coarse(line, Path::new(CWD), &policy),
            "{line}"
        );
    }
    // Unquoted, with the spaced directory on disk: bash's `<path>: Operation not permitted`
    let dir = std::env::temp_dir().join(format!("grok-sandbox-spaced-{}", std::process::id()));
    let support = dir.join("Library").join("Application Support");
    std::fs::create_dir_all(&support).unwrap();
    let target = support.join("MyTool").join("state.json");
    let line = format!("bash: {}: Operation not permitted\n", target.display());
    assert_eq!(
        Some(Blocked::FsWrite {
            path: target.clone()
        }),
        decode_coarse(&line, Path::new(CWD), &policy)
    );
    // The same words with no such directory stop at the space
    let elsewhere = dir.join("Library").join("Application Scripts").join("x");
    let line = format!("bash: {}: Operation not permitted\n", elsewhere.display());
    assert_eq!(
        Some(Blocked::FsWrite {
            path: dir.join("Library").join("Application"),
        }),
        decode_coarse(&line, Path::new(CWD), &policy)
    );
    // Prose after a path is not part of it: `rename A to B` keeps A
    let a = support.join("a");
    let b = support.join("b");
    let line = format!(
        "mv: rename {} to {}: Operation not permitted\n",
        a.display(),
        b.display()
    );
    assert_eq!(
        Some(Blocked::FsWrite { path: a }),
        decode_coarse(&line, Path::new(CWD), &policy)
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A path the output names beneath an automount root is never looked up (a lookup there can
/// mount a remote share and stall the decode): the line names no target, in any case spelling,
/// relative or as a rename's side. A write root beneath one is the user's and decodes as usual.
#[test]
fn a_path_beneath_an_automount_root_is_never_looked_up() {
    let policy = workspace_policy(Path::new(CWD));
    let probes = super::DIR_PROBES.get();
    for line in [
        "bash: /net/evil.example Support/x: Operation not permitted\n",
        "bash: /NET/evil.example/x: Operation not permitted\n",
        "bash: /Network/Servers/evil.example/x: Operation not permitted\n",
        "bash: /Volumes/share/x: Operation not permitted\n",
        "bash: ../../../../../net/evil.example Support/x: Operation not permitted\n",
    ] {
        let decoded = decode_coarse(line, Path::new(CWD), &policy);
        assert!(
            matches!(decoded, Some(Blocked::Unknown { .. })),
            "{line}: {decoded:?}"
        );
    }
    assert_eq!(
        probes,
        super::DIR_PROBES.get(),
        "no spaced name was looked up"
    );
    let line = "mv: rename /opt/ws-fixture/a to /net/evil.example/b: Operation not permitted\n";
    let decoded = decode_coarse(line, Path::new(CWD), &policy);
    assert!(
        matches!(decoded, Some(Blocked::Unknown { .. })),
        "{decoded:?}"
    );
    let probes = super::DIR_PROBES.get();

    let policy = workspace_policy(Path::new("/Volumes/Work/ws"));
    assert_eq!(
        Some(write("/Volumes/Work/ws/x")),
        decode_coarse(
            "touch: cannot touch '/Volumes/Work/ws/x': Permission denied\n",
            Path::new(CWD),
            &policy
        )
    );
    decode_coarse(
        "bash: /Volumes/Work/ws/My Notes/x: Operation not permitted\n",
        Path::new(CWD),
        &policy,
    );
    assert_eq!(probes + 1, super::DIR_PROBES.get());
}

#[test]
fn negative_fixtures_never_name_a_target() {
    let policy = workspace_policy(Path::new(CWD));
    for (name, stderr, expected) in NEGATIVE_CASES {
        let actual = decode_coarse(stderr, Path::new(CWD), &policy);
        assert_eq!(expected.map(|f| f()), actual, "fixture negative/{name}");
        assert!(
            !matches!(
                actual,
                Some(Blocked::FsWrite { .. } | Blocked::FsRead { .. } | Blocked::Net { .. })
            ),
            "fixture negative/{name} must not name a target"
        );
    }
    // The same key refusal with the CRLF ssh really prints
    assert_eq!(
        None,
        decode_coarse(
            "git@github.com: Permission denied (publickey).\r\nfatal: Could not read from remote repository.\n",
            Path::new(CWD),
            &policy
        )
    );
}

#[test]
fn remote_auth_and_socket_refusals_are_never_sandbox() {
    assert!(is_never_sandbox(
        "git@github.com: Permission denied (publickey)."
    ));
    assert!(is_never_sandbox(
        "user@10.0.0.7: Permission denied, please try again."
    ));
    assert!(is_never_sandbox(
        "dial unix /var/run/docker.sock: connect: permission denied"
    ));
    assert!(!is_never_sandbox(
        "touch: cannot touch '/etc/x': Permission denied"
    ));
    assert!(!is_never_sandbox(
        "bash: line 1: /opt/ws-fixture/dev/notes@work.txt: Permission denied"
    ));
    assert!(!is_likely_denial(
        "git@github.com: Permission denied (publickey).\n"
    ));
}

#[test]
fn a_path_inside_a_url_is_never_the_target() {
    let policy = workspace_policy(Path::new(CWD));
    // The URL is skipped and the real path after it is taken
    let line = "error: PUT https://registry.npmjs.org/pkg failed, cannot write /srv/out/log: Permission denied\n";
    assert_eq!(
        Some(write("/srv/out/log")),
        decode_coarse(line, Path::new(CWD), &policy)
    );
    let line = "fatal: unable to access 'https://github.com/x/y/': Operation not permitted\n";
    assert_eq!(
        Some(unknown(line.trim())),
        decode_coarse(line, Path::new(CWD), &policy)
    );
}

#[test]
fn stderr_without_a_marker_is_not_a_denial() {
    let policy = workspace_policy(Path::new(CWD));
    assert_eq!(
        None,
        decode_coarse(
            "error[E0425]: cannot find value `x` in this scope\n",
            Path::new(CWD),
            &policy
        )
    );
    assert!(!is_likely_denial("npm ERR! missing script: build"));
    assert!(is_likely_denial("x: Permission denied"));
    // A death with no text (a signal) is not a denial the coarse channel can read
    assert_eq!(None, decode_coarse("", Path::new(CWD), &policy));
}

/// `sandbox-exec: execvp() of '/usr/bin/sudo' failed: Operation not permitted` (exit 71, no
/// `Sandbox:` line): Seatbelt refused the setuid exec before any rule ran.
#[test]
fn seatbelt_setuid_exec_refusal_is_a_capability() {
    let policy = workspace_policy(Path::new(CWD));
    assert_eq!(
        Some(Blocked::Capability {
            what: Capability::SetUid
        }),
        decode_coarse(
            "sandbox-exec: execvp() of '/usr/bin/sudo' failed: Operation not permitted\n",
            Path::new(CWD),
            &policy
        )
    );
}

#[test]
fn capability_programs_are_never_paths() {
    let policy = workspace_policy(Path::new(CWD));
    for (stderr, what) in [
        ("mount: /mnt/data: permission denied.", Capability::Mount),
        (
            "sudo: /etc/sudoers: Operation not permitted",
            Capability::SetUid,
        ),
        (
            "lldb: ptrace(PT_ATTACHEXC, ...): Operation not permitted",
            Capability::Ptrace,
        ),
    ] {
        assert_eq!(
            Some(Blocked::Capability { what }),
            decode_coarse(stderr, Path::new(CWD), &policy),
            "{stderr}"
        );
    }
}

#[test]
fn relative_paths_join_the_cwd_and_fold_dots() {
    let cwd = Path::new("/opt/ws-fixture/dev/ws/sub");
    assert_eq!(
        PathBuf::from("/opt/ws-fixture/dev/ws/sub/out.txt"),
        lexical_join(cwd, "./out.txt")
    );
    assert_eq!(
        PathBuf::from("/opt/ws-fixture/dev/ws/other/x"),
        lexical_join(cwd, "../other/./x")
    );
    assert_eq!(
        PathBuf::from("/etc/passwd"),
        lexical_join(cwd, "/etc/passwd")
    );
}

#[test]
fn dot_directory_relative_paths_join_the_cwd_but_bare_names_do_not() {
    let policy = workspace_policy(Path::new(CWD));
    assert_eq!(
        Some(write(
            "/opt/ws-fixture/homedir/w1-scratch/ws/.grok/config.toml"
        )),
        decode_coarse(
            "sh: 1: cannot create .grok/config.toml: Operation not permitted\n",
            Path::new(CWD),
            &policy
        )
    );
    for bare in [
        "sh: 1: cannot create config.toml: Operation not permitted\n",
        "sh: 1: cannot create .hidden: Operation not permitted\n",
        "sh: 1: cannot create ..weird/x: Operation not permitted\n",
        "sh: 1: cannot create ... : Operation not permitted\n",
    ] {
        assert!(
            matches!(
                decode_coarse(bare, Path::new(CWD), &policy),
                Some(Blocked::Unknown { .. })
            ),
            "{bare}"
        );
    }
}

#[test]
fn ambiguous_marker_falls_back_to_what_the_policy_refuses() {
    let mut policy = workspace_policy(Path::new(CWD));
    policy.read = ReadPolicy::AllExcept {
        deny: vec![DenyEntry::Path(PathBuf::from(
            "/opt/ws-fixture/homedir/w1-scratch/ws/secret",
        ))],
    };
    // The workspace is writable, so a denial there can only be the read the policy refused
    let line = "bash: line 1: /opt/ws-fixture/homedir/w1-scratch/ws/secret: Permission denied\n";
    assert_eq!(
        Some(read("/opt/ws-fixture/homedir/w1-scratch/ws/secret")),
        decode_coarse(line, Path::new(CWD), &policy)
    );
    // Outside the workspace a bare denial is the write the policy refused
    let line = "bash: line 1: /etc/motd: Permission denied\n";
    assert_eq!(
        Some(write("/etc/motd")),
        decode_coarse(line, Path::new(CWD), &policy)
    );
}

#[test]
fn proposal_climbs_to_the_nearest_existing_directory() {
    let dir = std::env::temp_dir().join(format!("grok-sandbox-coarse-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("existing.txt");
    std::fs::write(&file, "x").unwrap();
    assert_eq!(dir, nearest_existing_dir(&file));
    assert_eq!(
        dir,
        nearest_existing_dir(&dir.join("missing").join("deeper"))
    );
    assert_eq!(dir, nearest_existing_dir(&dir));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The macOS resolver names the host in the segment before its phrase; the card offers it.
#[test]
fn resolver_failure_names_the_host_before_the_phrase() {
    let policy = workspace_policy(Path::new(CWD));
    assert_eq!(
        Some(net("registry.npmjs.org", None)),
        decode_coarse(
            "bash: line 1: registry.npmjs.org: nodename nor servname provided, or not known\n",
            Path::new(CWD),
            &policy
        )
    );
    // A loopback failure is the proxy itself refusing, never a violation
    assert_eq!(
        None,
        decode_coarse(
            "curl: (7) Failed to connect to 127.0.0.1 port 8123 after 0 ms: Couldn't connect to server\n",
            Path::new(CWD),
            &policy
        )
    );
}

/// Only a parsed loopback address (or `localhost`) is the proxy refusing; a name that merely
/// starts with `127.` is a remote host whose blocked connection is decoded.
#[test]
fn only_a_parsed_loopback_address_is_the_proxy() {
    let policy = workspace_policy(Path::new(CWD));
    let failed = |host: &str| {
        decode_coarse(
            &format!(
                "curl: (7) Failed to connect to {host} port 443 after 3 ms: Couldn't connect to server\n"
            ),
            Path::new(CWD),
            &policy,
        )
    };
    for remote in [
        "127.evil.com",
        "127.0.0.1.nip.io",
        "1270.0.0.1",
        "127.example",
    ] {
        assert_eq!(Some(net(remote, Some(443))), failed(remote), "{remote}");
    }
    for loopback in [
        "127.0.0.1",
        "127.3.2.1",
        "::1",
        "[::1]",
        "::ffff:127.0.0.1",
        "localhost",
        "LocalHost.",
    ] {
        assert_eq!(None, failed(loopback), "{loopback}");
    }
}

#[test]
fn stderr_tail_keeps_the_last_bytes_on_a_char_boundary() {
    let text = format!("{}é", "a".repeat(600));
    let tail = stderr_tail(&text, 10);
    assert!(tail.len() <= 10);
    assert!(tail.ends_with('é'));
    assert_eq!("short", stderr_tail("  short \n", 512));
}
