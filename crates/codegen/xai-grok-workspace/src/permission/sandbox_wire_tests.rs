use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use xai_grok_sandbox::command::grants::{Expiry, GrantScope, GrantSubject, HostPattern};
use xai_grok_sandbox::command::violation::{
    Blocked, Capability, Disposition, InformationalReason, Replay, Violation,
};
use xai_grok_sandbox::command::{BackendName, ProposalBounds, SandboxMode};
use xai_tool_runtime::ToolApprovalPolicy;

use super::{
    Ceiling, MAX_FOLLOWUP_CHARS, MAX_PERSISTED_TTL_SECONDS, OfferedScope, PRE_RUN_KEYS,
    ProposedGrant, SANDBOX_VIOLATION_KIND, SCHEMA_VERSION, SandboxAnswer, SandboxCard,
    SandboxCardContext, build_sandbox_violation_payload, decode_reply, is_wire_expressible,
};

const WS: &str = "/opt/ws-fixture/u/proj";
const HOME: &str = "/opt/ws-fixture/u";
const NOW: i64 = 1_800_000_000;
const DEADLINE: i64 = NOW + 600;

fn bounds() -> ProposalBounds<'static> {
    ProposalBounds {
        workspace_root: Path::new(WS),
        user_home: Some(Path::new(HOME)),
        extra_bases: &[],
    }
}

fn fs_write_violation() -> Violation {
    Violation {
        blocked: Blocked::FsWrite {
            path: PathBuf::from("/opt/ws-fixture/u/Library/Caches/npm/_cacache/index"),
        },
        proposed: Some(GrantSubject::FsWriteRoot {
            root: PathBuf::from("/opt/ws-fixture/u/Library/Caches/npm"),
        }),
        disposition: Disposition::Grantable,
        partial_output: Some(true),
        replay: Replay::Rerun,
        exit_code: Some(1),
        stderr_snippet:
            "npm ERR! EACCES: permission denied, mkdir '/opt/ws-fixture/u/Library/Caches/npm/_cacache/index'"
                .to_owned(),
    }
}

/// A read denied by a profile deny entry, proposing the directory (the desktop renders the
/// `fs_read` card from this fixture).
fn fs_read_violation() -> Violation {
    Violation {
        blocked: Blocked::FsRead {
            path: PathBuf::from("/opt/ws-fixture/u/shared/config/settings.toml"),
        },
        proposed: Some(GrantSubject::FsRead {
            root: PathBuf::from("/opt/ws-fixture/u/shared/config"),
        }),
        disposition: Disposition::Grantable,
        partial_output: Some(false),
        replay: Replay::Rerun,
        exit_code: Some(1),
        stderr_snippet:
            "cat: /opt/ws-fixture/u/shared/config/settings.toml: Operation not permitted".to_owned(),
    }
}

/// A write into a curated build-cache tree: the card offers the family,
/// never the subpath — `proposed_grant.subject` is the bare `build_caches` kind.
fn build_caches_violation() -> Violation {
    Violation {
        blocked: Blocked::FsWrite {
            path: PathBuf::from(
                "/opt/ws-fixture/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/serde-1.0.0/build.rs",
            ),
        },
        proposed: Some(GrantSubject::BuildCaches),
        disposition: Disposition::Grantable,
        partial_output: Some(true),
        replay: Replay::Rerun,
        exit_code: Some(101),
        stderr_snippet: "error: failed to unpack package `serde v1.0.0`\n\nCaused by:\n  failed to create directory `/opt/ws-fixture/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/serde-1.0.0`\n\nCaused by:\n  Operation not permitted (os error 1)"
            .to_owned(),
    }
}

/// A grantable path the command never named: informational `unattributed`.
fn unattributed_violation() -> Violation {
    Violation {
        blocked: Blocked::FsWrite {
            path: PathBuf::from("/opt/ws-fixture/u/Documents/notes/todo.txt"),
        },
        proposed: None,
        disposition: Disposition::informational(InformationalReason::Unattributed),
        partial_output: Some(false),
        replay: Replay::Rerun,
        exit_code: Some(1),
        stderr_snippet: "sh: /opt/ws-fixture/u/Documents/notes/todo.txt: Operation not permitted"
            .to_owned(),
    }
}

/// A connection the proxy held (the decider's violation): the card proposes the host on any
/// port, exactly as `SandboxNetworkDecider` builds it.
fn net_violation() -> Violation {
    Violation {
        blocked: Blocked::Net {
            host: Some("registry.npmjs.org".to_owned()),
            port: Some(443),
        },
        proposed: Some(GrantSubject::NetHost {
            host: HostPattern::new("registry.npmjs.org"),
            port: None,
        }),
        disposition: Disposition::Grantable,
        partial_output: Some(false),
        replay: Replay::Resume {
            hold_id: "h-1".to_owned(),
        },
        exit_code: None,
        stderr_snippet: String::new(),
    }
}

fn protected_violation() -> Violation {
    Violation {
        blocked: Blocked::FsWrite {
            path: PathBuf::from("/opt/ws-fixture/u/proj/.git/hooks/pre-commit"),
        },
        proposed: None,
        disposition: Disposition::informational(InformationalReason::ProtectedTarget),
        partial_output: None,
        replay: Replay::Rerun,
        exit_code: Some(1),
        stderr_snippet: "Permission denied".to_owned(),
    }
}

/// A network denial the post-run decoder read: the proxy never saw it.
fn unproxied_net_violation() -> Violation {
    Violation {
        blocked: Blocked::Net {
            host: None,
            port: Some(443),
        },
        proposed: None,
        disposition: Disposition::informational(InformationalReason::UnproxiedNetwork),
        partial_output: Some(false),
        replay: Replay::Rerun,
        exit_code: Some(7),
        stderr_snippet: "curl: (7) Failed to connect to registry.npmjs.org port 443".to_owned(),
    }
}

/// The proxy refused a host on `disallowed_web_fetch_domains`.
fn denylist_violation() -> Violation {
    Violation {
        blocked: Blocked::Net {
            host: Some("tracker.example".to_owned()),
            port: Some(443),
        },
        proposed: None,
        disposition: Disposition::informational(InformationalReason::PolicyDenylist),
        partial_output: None,
        replay: Replay::Resume {
            hold_id: "card-1".to_owned(),
        },
        exit_code: None,
        stderr_snippet: String::new(),
    }
}

fn capability_violation() -> Violation {
    Violation {
        blocked: Blocked::Capability {
            what: Capability::Ptrace,
        },
        proposed: None,
        disposition: Disposition::informational(InformationalReason::Capability),
        partial_output: None,
        replay: Replay::Rerun,
        exit_code: Some(159),
        stderr_snippet: String::new(),
    }
}

fn ctx(policy: ToolApprovalPolicy) -> SandboxCardContext<'static> {
    SandboxCardContext {
        tool_call_id: "tc-0192c1a0",
        command: "npm ci",
        policy,
        mode: SandboxMode::Enforce,
        backend: Some(BackendName::Seatbelt),
        reduced_sandbox: false,
        deadline_unix: Some(DEADLINE),
    }
}

fn grants_allowed_ceiling(violation: &Violation) -> Ceiling {
    Ceiling::for_violation(ToolApprovalPolicy::GrantsAllowed, violation)
}

fn decode(reply: Value, violation: &Violation, ceiling: &Ceiling) -> SandboxAnswer {
    decode_reply(&reply, violation, ceiling, &bounds(), NOW)
}

/// The shared wire fixtures: one directory the Rust, relay and desktop
/// tests all read, so no test carries a literal copy of a card. `card_*.json` is the payload
/// the daemon posts, pre-run keys and nested card included; `reply_*.json` is a desktop answer.
/// The update path writes back here; reads go through `FIXTURES`.
const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/sandbox_wire");

/// Every fixture by stem, compiled in with `include_str!` so the tests never read
/// `CARGO_MANIFEST_DIR` at run time.
const FIXTURES: &[(&str, &str)] = &[
    (
        "card_build_caches_grantable",
        include_str!("../../fixtures/sandbox_wire/card_build_caches_grantable.json"),
    ),
    (
        "card_capability",
        include_str!("../../fixtures/sandbox_wire/card_capability.json"),
    ),
    (
        "card_fs_read_grantable",
        include_str!("../../fixtures/sandbox_wire/card_fs_read_grantable.json"),
    ),
    (
        "card_fs_write_backend_none",
        include_str!("../../fixtures/sandbox_wire/card_fs_write_backend_none.json"),
    ),
    (
        "card_fs_write_grantable",
        include_str!("../../fixtures/sandbox_wire/card_fs_write_grantable.json"),
    ),
    (
        "card_fs_write_protected_target",
        include_str!("../../fixtures/sandbox_wire/card_fs_write_protected_target.json"),
    ),
    (
        "card_fs_write_unattributed",
        include_str!("../../fixtures/sandbox_wire/card_fs_write_unattributed.json"),
    ),
    (
        "card_net_held_grantable",
        include_str!("../../fixtures/sandbox_wire/card_net_held_grantable.json"),
    ),
    (
        "card_net_policy_denylist",
        include_str!("../../fixtures/sandbox_wire/card_net_policy_denylist.json"),
    ),
    (
        "card_net_unproxied",
        include_str!("../../fixtures/sandbox_wire/card_net_unproxied.json"),
    ),
    (
        "reply_always_reject",
        include_str!("../../fixtures/sandbox_wire/reply_always_reject.json"),
    ),
    (
        "reply_approve_call",
        include_str!("../../fixtures/sandbox_wire/reply_approve_call.json"),
    ),
    (
        "reply_build_caches_workspace",
        include_str!("../../fixtures/sandbox_wire/reply_build_caches_workspace.json"),
    ),
    (
        "reply_decode_failed",
        include_str!("../../fixtures/sandbox_wire/reply_decode_failed.json"),
    ),
    (
        "reply_decode_failed_nested",
        include_str!("../../fixtures/sandbox_wire/reply_decode_failed_nested.json"),
    ),
    (
        "reply_net_host_host_only",
        include_str!("../../fixtures/sandbox_wire/reply_net_host_host_only.json"),
    ),
    (
        "reply_net_host_matching_port",
        include_str!("../../fixtures/sandbox_wire/reply_net_host_matching_port.json"),
    ),
    (
        "reply_net_host_other_port",
        include_str!("../../fixtures/sandbox_wire/reply_net_host_other_port.json"),
    ),
];

/// Every card fixture: `(file stem, violation, policy, context override)`.
fn card_fixtures() -> Vec<(&'static str, Violation, SandboxCardContext<'static>)> {
    let grants = ctx(ToolApprovalPolicy::GrantsAllowed);
    let informational = SandboxCardContext {
        deadline_unix: None,
        ..grants
    };
    vec![
        ("card_fs_write_grantable", fs_write_violation(), grants),
        ("card_fs_read_grantable", fs_read_violation(), grants),
        (
            "card_build_caches_grantable",
            build_caches_violation(),
            SandboxCardContext {
                command: "cargo build",
                ..grants
            },
        ),
        ("card_net_held_grantable", net_violation(), grants),
        (
            "card_fs_write_protected_target",
            protected_violation(),
            informational,
        ),
        (
            "card_fs_write_unattributed",
            unattributed_violation(),
            informational,
        ),
        (
            "card_net_unproxied",
            unproxied_net_violation(),
            informational,
        ),
        (
            "card_net_policy_denylist",
            denylist_violation(),
            informational,
        ),
        ("card_capability", capability_violation(), informational),
        (
            "card_fs_write_backend_none",
            fs_write_violation(),
            SandboxCardContext {
                backend: None,
                reduced_sandbox: true,
                ..grants
            },
        ),
    ]
}

fn read_fixture(stem: &str) -> Value {
    let path = format!("{FIXTURE_DIR}/{stem}.json");
    let Some((_, text)) = FIXTURES.iter().find(|(name, _)| *name == stem) else {
        panic!("{path}: not listed in FIXTURES");
    };
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// `SANDBOX_WIRE_FIXTURES_UPDATE=1` rewrites the card fixtures from the current payload builder
/// (the relay and desktop suites then pick the change up); the plain run compares.
#[test]
fn card_payloads_match_the_shared_fixtures() {
    let update = std::env::var_os("SANDBOX_WIRE_FIXTURES_UPDATE").is_some();
    for (stem, violation, context) in card_fixtures() {
        let ceiling = Ceiling::for_violation(context.policy, &violation);
        let payload = build_sandbox_violation_payload(context, &violation, &ceiling);
        if update {
            let path = format!("{FIXTURE_DIR}/{stem}.json");
            let mut text = serde_json::to_string_pretty(&payload).unwrap();
            text.push('\n');
            std::fs::write(&path, text).unwrap();
            continue;
        }
        assert_eq!(read_fixture(stem), payload, "{stem}");
    }
}

/// Every card fixture carries the versioned envelope: `schema_version`, a
/// `disposition` the desktop derives nothing from, a `backend` that is never `null`, and no
/// offer on an informational card.
#[test]
fn every_card_fixture_carries_the_versioned_envelope() {
    for (stem, violation, _) in card_fixtures() {
        let card = read_fixture(stem);
        assert_eq!(Some(&json!(1)), card.get("schema_version"), "{stem}");
        assert_eq!(
            Some(&json!(1)),
            card.pointer("/sandbox_violation/schema_version"),
            "{stem}"
        );
        assert!(
            card.get("backend").is_some_and(Value::is_string),
            "{stem}: {:?}",
            card.get("backend")
        );
        assert_eq!(
            Some(&serde_json::to_value(violation.disposition).unwrap()),
            card.get("disposition"),
            "{stem}"
        );
        if violation.is_grantable() {
            assert!(
                card.get("offered_scopes")
                    .and_then(Value::as_array)
                    .is_some_and(|scopes| !scopes.is_empty()),
                "{stem}"
            );
            assert!(card.get("deadline_unix").is_some(), "{stem}");
        } else {
            assert_eq!(Some(&json!([])), card.get("offered_scopes"), "{stem}");
            assert!(card.get("proposed_grant").is_none(), "{stem}");
            assert!(card.get("deadline_unix").is_none(), "{stem}");
        }
    }
    assert_eq!(
        Some(&json!("none")),
        read_fixture("card_fs_write_backend_none").get("backend")
    );
}

/// The reply fixtures, decoded against the card each answers: what the daemon makes of them.
#[test]
fn reply_fixtures_decode_as_documented() {
    let held = net_violation();
    let held_ceiling = grants_allowed_ceiling(&held);
    let host_only = |stem: &str| decode(read_fixture(stem), &held, &held_ceiling);
    let npm_443 = GrantSubject::NetHost {
        host: HostPattern::new("registry.npmjs.org"),
        port: Some(443),
    };
    assert!(
        matches!(
            host_only("reply_net_host_host_only"),
            SandboxAnswer::Allow { ref subject, scope: GrantScope::Session, .. } if *subject == npm_443
        ),
        "{:?}",
        host_only("reply_net_host_host_only")
    );
    assert!(
        matches!(
            host_only("reply_net_host_matching_port"),
            SandboxAnswer::Allow { ref subject, .. } if *subject == npm_443
        ),
        "a trailing :443 equal to the violation's port is tolerated for one release"
    );
    assert!(
        matches!(
            host_only("reply_net_host_other_port"),
            SandboxAnswer::SubjectRefused { ref reason } if reason.contains("8443")
        ),
        "any other port refuses the allow rather than widening it: {:?}",
        host_only("reply_net_host_other_port")
    );
    assert_eq!(
        SandboxAnswer::DecodeFailed,
        host_only("reply_decode_failed"),
        "the renderer could not decode the card: not the user's answer"
    );
    assert_eq!(
        SandboxAnswer::DecodeFailed,
        host_only("reply_decode_failed_nested"),
        "after the relay the reason travels inside the bash_command value"
    );
    let fs = fs_write_violation();
    let fs_ceiling = grants_allowed_ceiling(&fs);
    let caches = build_caches_violation();
    let caches_ceiling = grants_allowed_ceiling(&caches);
    assert_eq!(
        SandboxAnswer::Allow {
            subject: GrantSubject::BuildCaches,
            scope: GrantScope::Workspace {
                root: PathBuf::from(WS),
            },
            expires: Expiry::Ttl {
                seconds: MAX_PERSISTED_TTL_SECONDS,
            },
        },
        decode(
            read_fixture("reply_build_caches_workspace"),
            &caches,
            &caches_ceiling
        ),
        "the family for the workspace, seven days"
    );
    assert!(
        matches!(
            decode(
                read_fixture("reply_build_caches_workspace"),
                &fs,
                &fs_ceiling
            ),
            SandboxAnswer::Allow {
                subject: GrantSubject::FsWriteRoot { .. },
                ..
            }
        ),
        "build_caches on a card that offered a folder falls back to the folder proposal"
    );
    assert_eq!(
        SandboxAnswer::Deny {
            followup: None,
            remember: true
        },
        decode(read_fixture("reply_always_reject"), &fs, &fs_ceiling)
    );
    assert_eq!(
        SandboxAnswer::NoSandboxDecision,
        decode(read_fixture("reply_approve_call"), &fs, &fs_ceiling)
    );
    // A reply on an informational card never yields a grant, whatever it says
    let protected = protected_violation();
    assert_eq!(
        SandboxAnswer::Deny {
            followup: None,
            remember: false
        },
        decode(
            read_fixture("reply_approve_call"),
            &protected,
            &grants_allowed_ceiling(&protected)
        )
    );
}

/// The relay forwards `payload.sandbox_violation` verbatim as the card: it is
/// the whole card, `kind` and `bash_command` included, and nothing but the card.
#[test]
fn nested_card_is_the_top_level_minus_the_pre_run_keys() {
    for violation in [fs_write_violation(), net_violation(), protected_violation()] {
        let ceiling = grants_allowed_ceiling(&violation);
        let payload = build_sandbox_violation_payload(
            ctx(ToolApprovalPolicy::GrantsAllowed),
            &violation,
            &ceiling,
        );
        let top = payload.as_object().unwrap();
        let mut expected = top.clone();
        for key in PRE_RUN_KEYS {
            expected.remove(key);
        }
        let nested = top[SANDBOX_VIOLATION_KIND].as_object().unwrap();
        assert_eq!(&expected, nested, "{violation:?}");
        assert_eq!(json!(SANDBOX_VIOLATION_KIND), nested["kind"]);
        assert_eq!(json!("npm ci"), nested["bash_command"]);
        for key in [
            "tool_call_id",
            "tool_name",
            "description",
            "scope",
            "tool_approval_policy",
        ] {
            assert!(top.contains_key(key), "{key} stays top-level");
        }
    }
}

#[test]
fn card_carries_the_snippet_exit_code_deadline_and_confirm_flag() {
    let violation = fs_write_violation();
    let ceiling = grants_allowed_ceiling(&violation);
    let payload = build_sandbox_violation_payload(
        ctx(ToolApprovalPolicy::GrantsAllowed),
        &violation,
        &ceiling,
    );
    // A file-system target is read from the command's output (coarse decode): confirm
    assert_eq!(Some(&json!(true)), payload.get("proposal_confirm"));
    assert_eq!(
        Some(&json!(violation.stderr_snippet)),
        payload.get("stderr_snippet")
    );
    assert_eq!(Some(&json!(1)), payload.get("exit_code"));
    assert_eq!(Some(&json!(DEADLINE)), payload.get("deadline_unix"));
    assert_eq!(
        Some(&json!("grantable")),
        payload.pointer("/disposition/kind")
    );
    assert_eq!(Some(true), violation.partial_output);
    assert!(
        payload.get("partial_output").is_none(),
        "the card has no partial-output field: nothing reports it"
    );

    // A connection the proxy held is exact: nothing to confirm
    let held = net_violation();
    let payload = build_sandbox_violation_payload(
        ctx(ToolApprovalPolicy::GrantsAllowed),
        &held,
        &grants_allowed_ceiling(&held),
    );
    assert_eq!(Some(&json!(false)), payload.get("proposal_confirm"));

    // No exit code (a signal) and no deadline are omitted, not null
    let mut signalled = fs_write_violation();
    signalled.exit_code = None;
    let payload = build_sandbox_violation_payload(
        SandboxCardContext {
            deadline_unix: None,
            ..ctx(ToolApprovalPolicy::GrantsAllowed)
        },
        &signalled,
        &ceiling,
    );
    assert!(payload.get("exit_code").is_none());
    assert!(payload.get("deadline_unix").is_none());
}

#[test]
fn ceiling_follows_the_tenant_policy() {
    let violation = fs_write_violation();
    let prompt = Ceiling::for_violation(ToolApprovalPolicy::AlwaysPrompt, &violation);
    assert_eq!(vec![OfferedScope::Call], prompt.scopes);
    assert!(prompt.expiries.is_empty());

    let grants = grants_allowed_ceiling(&violation);
    assert_eq!(
        vec![
            OfferedScope::Call,
            OfferedScope::Session,
            OfferedScope::Workspace
        ],
        grants.scopes
    );
    assert!(!grants.expiries.contains(&Expiry::Never));
    assert!(grants.expiries.iter().all(|e| matches!(
        e,
        Expiry::Ttl { seconds } if *seconds <= MAX_PERSISTED_TTL_SECONDS
    )));

    let unattended = Ceiling::for_violation(ToolApprovalPolicy::UnattendedAllowed, &violation);
    assert!(unattended.scopes.contains(&OfferedScope::Global));
    assert!(unattended.expiries.contains(&Expiry::Never));
}

#[test]
fn protected_and_capability_cards_are_informational_deny_only() {
    let protected = protected_violation();
    let ceiling = Ceiling::for_violation(ToolApprovalPolicy::UnattendedAllowed, &protected);
    assert_eq!(Ceiling::deny_only(), ceiling);
    assert!(ceiling.is_deny_only());
    let payload = build_sandbox_violation_payload(
        ctx(ToolApprovalPolicy::UnattendedAllowed),
        &protected,
        &ceiling,
    );
    assert_eq!(Some(&json!([])), payload.get("offered_scopes"));
    assert_eq!(Some(&json!([])), payload.get("offered_expiries"));
    assert_eq!(
        Some(&json!("informational")),
        payload.pointer("/disposition/kind")
    );
    assert!(payload.get("proposed_grant").is_none());
    assert!(
        payload.get("deadline_unix").is_none(),
        "an informational card is not waited for"
    );

    let capability = Violation {
        blocked: Blocked::Capability {
            what: Capability::Ptrace,
        },
        proposed: None,
        disposition: Disposition::informational(InformationalReason::Capability),
        partial_output: None,
        replay: Replay::Rerun,
        exit_code: Some(159),
        stderr_snippet: String::new(),
    };
    let ceiling = Ceiling::for_violation(ToolApprovalPolicy::UnattendedAllowed, &capability);
    assert!(ceiling.scopes.is_empty());
    let payload = build_sandbox_violation_payload(
        ctx(ToolApprovalPolicy::UnattendedAllowed),
        &capability,
        &ceiling,
    );
    assert_eq!(
        Some(&json!({ "kind": "capability", "what": "ptrace" })),
        payload.get("blocked")
    );
    assert_eq!(
        Some(&json!("informational")),
        payload.pointer("/disposition/kind")
    );
    assert_eq!(Some(&json!(false)), payload.get("proposal_confirm"));
}

/// `<base>/cache-\xff`: a path Linux can hold that is not UTF-8.
#[cfg(unix)]
fn not_utf8(base: &str) -> PathBuf {
    let name: &std::ffi::OsStr = std::os::unix::ffi::OsStrExt::from_bytes(b"cache-\xff");
    Path::new(base).join(name)
}

/// A path that is not UTF-8 — blocked or proposed — cannot travel as a JSON string: the card is
/// informational with the renderer's own `decode_failed` words, proposes and offers nothing, is not
/// waited for, and shows the blocked path lossily; an allow on it is a denial. Whatever ceiling it
/// is handed, it never offers a grant for a path it cannot name.
#[cfg(unix)]
#[test]
fn a_path_that_is_not_utf8_gets_an_informational_card_with_a_lossy_path() {
    let bad = not_utf8("/opt/ws-fixture/u/Library/Caches");
    let lossy = "/opt/ws-fixture/u/Library/Caches/cache-\u{fffd}";
    let cases = [
        (
            "a blocked write under its proposal",
            Violation {
                blocked: Blocked::FsWrite {
                    path: bad.join("index"),
                },
                proposed: Some(GrantSubject::FsWriteRoot { root: bad.clone() }),
                ..fs_write_violation()
            },
            json!({ "kind": "fs_write", "path": format!("{lossy}/index") }),
        ),
        (
            "a blocked read",
            Violation {
                blocked: Blocked::FsRead { path: bad.clone() },
                ..fs_read_violation()
            },
            json!({ "kind": "fs_read", "path": lossy }),
        ),
        (
            "a proposal only",
            Violation {
                proposed: Some(GrantSubject::FsWriteRoot { root: bad.clone() }),
                ..fs_write_violation()
            },
            json!({
                "kind": "fs_write",
                "path": "/opt/ws-fixture/u/Library/Caches/npm/_cacache/index",
            }),
        ),
    ];
    for (case, violation, blocked) in cases {
        assert!(!is_wire_expressible(&violation), "{case}");
        let ceiling = Ceiling::for_violation(ToolApprovalPolicy::UnattendedAllowed, &violation);
        assert_eq!(Ceiling::deny_only(), ceiling, "{case}");
        let wide = grants_allowed_ceiling(&fs_write_violation());
        for handed in [&ceiling, &wide] {
            let payload = build_sandbox_violation_payload(
                ctx(ToolApprovalPolicy::UnattendedAllowed),
                &violation,
                handed,
            );
            let nested = payload.get(SANDBOX_VIOLATION_KIND).cloned();
            for card in [Some(payload), nested] {
                let card = card.unwrap_or_default();
                assert_eq!(
                    Some(&json!({ "kind": "informational", "reason": "decode_failed" })),
                    card.get("disposition"),
                    "{case}: {card}"
                );
                assert_eq!(Some(&blocked), card.get("blocked"), "{case}");
                assert!(card.get("proposed_grant").is_none(), "{case}: {card}");
                assert_eq!(Some(&json!([])), card.get("offered_scopes"), "{case}");
                assert_eq!(Some(&json!([])), card.get("offered_expiries"), "{case}");
                assert_eq!(Some(&json!(false)), card.get("proposal_confirm"), "{case}");
                assert!(card.get("deadline_unix").is_none(), "{case}: {card}");
            }
        }
        let answer = decode(
            json!({ "outcome": "always_approve", "duration": { "kind": "global" } }),
            &violation,
            &ceiling,
        );
        assert_eq!(
            SandboxAnswer::Deny {
                followup: None,
                remember: false
            },
            answer,
            "{case}"
        );
    }
    assert!(is_wire_expressible(&fs_write_violation()));
    assert!(is_wire_expressible(&net_violation()));
}

/// Turning a card into JSON never panics: were a field ever to refuse, the renderer gets a sandbox
/// card it cannot decode (it answers `decode_failed`), and the denial stands.
#[cfg(unix)]
#[test]
fn a_card_that_does_not_serialize_becomes_one_the_renderer_rejects() {
    let violation = fs_write_violation();
    let ceiling = grants_allowed_ceiling(&violation);
    let ctx = ctx(ToolApprovalPolicy::GrantsAllowed);
    let unencodable = GrantSubject::FsWriteRoot {
        root: not_utf8("/opt/ws-fixture/u/Library/Caches"),
    };
    let mut card = SandboxCard::new(&ctx, &violation, &ceiling);
    card.proposed_grant = Some(ProposedGrant {
        subject: &unencodable,
    });
    assert_eq!(
        json!({
            "schema_version": SCHEMA_VERSION,
            "kind": SANDBOX_VIOLATION_KIND,
            "bash_command": "npm ci",
        }),
        Value::Object(card.to_json())
    );
}

#[test]
fn unknown_blocked_carries_no_stderr_content_in_blocked() {
    let violation = Violation {
        blocked: Blocked::Unknown {
            stderr_snippet: "token=abc".to_owned(),
        },
        proposed: None,
        disposition: Disposition::Grantable,
        partial_output: None,
        replay: Replay::Rerun,
        exit_code: Some(1),
        stderr_snippet: "token=abc".to_owned(),
    };
    let ceiling = grants_allowed_ceiling(&violation);
    assert!(ceiling.is_deny_only());
    let payload = build_sandbox_violation_payload(
        ctx(ToolApprovalPolicy::GrantsAllowed),
        &violation,
        &ceiling,
    );
    assert_eq!(Some(&json!({ "kind": "unknown" })), payload.get("blocked"));
    assert_eq!(Some(&json!("token=abc")), payload.get("stderr_snippet"));
}

#[test]
fn net_payload_carries_host_port_and_resume() {
    let violation = net_violation();
    let ceiling = grants_allowed_ceiling(&violation);
    let payload = build_sandbox_violation_payload(
        ctx(ToolApprovalPolicy::GrantsAllowed),
        &violation,
        &ceiling,
    );
    assert_eq!(
        Some(&json!({ "kind": "net", "host": "registry.npmjs.org", "port": 443 })),
        payload.get("blocked")
    );
    assert_eq!(Some(&json!("resume")), payload.get("replay"));
    assert_eq!(
        Some(&json!({ "kind": "net_host", "host": "registry.npmjs.org" })),
        payload.pointer("/proposed_grant/subject"),
        "the card proposes the host on any port; the answer keeps the held port"
    );
    assert!(payload.get("exit_code").is_none());

    // A host the kernel masked is absent on the wire, and the proposal is the workspace-wide one
    let masked = Violation {
        blocked: Blocked::Net {
            host: None,
            port: Some(443),
        },
        proposed: Some(GrantSubject::NetHost {
            host: HostPattern::all(),
            port: Some(443),
        }),
        ..net_violation()
    };
    let payload = build_sandbox_violation_payload(
        ctx(ToolApprovalPolicy::GrantsAllowed),
        &masked,
        &grants_allowed_ceiling(&masked),
    );
    assert_eq!(
        Some(&json!({ "kind": "net", "port": 443 })),
        payload.get("blocked"),
        "an unknown host is absent, not null"
    );
    assert_eq!(
        Some(&json!("*")),
        payload.pointer("/proposed_grant/subject/host")
    );
}

/// With no egress proxy a per-host grant would be a no-op: the card must not offer one.
/// The decoder marks such a
/// denial `unproxied_network` — the ceiling reads the disposition, not the proxy state, and a
/// `host: null` connect carries no proposal.
#[test]
fn an_unproxied_connection_is_deny_only_whatever_the_policy() {
    let violation = Violation {
        blocked: Blocked::Net {
            host: None,
            port: Some(443),
        },
        proposed: None,
        disposition: Disposition::informational(InformationalReason::UnproxiedNetwork),
        replay: Replay::Rerun,
        exit_code: Some(7),
        ..net_violation()
    };
    let ceiling = Ceiling::for_violation(ToolApprovalPolicy::UnattendedAllowed, &violation);
    assert_eq!(Ceiling::deny_only(), ceiling);
    let payload = build_sandbox_violation_payload(
        ctx(ToolApprovalPolicy::UnattendedAllowed),
        &violation,
        &ceiling,
    );
    assert_eq!(Some(&json!([])), payload.get("offered_scopes"));
    assert_eq!(
        Some(&json!({ "kind": "informational", "reason": "unproxied_network" })),
        payload.get("disposition")
    );
    assert_eq!(
        Some(&json!({ "kind": "net", "port": 443 })),
        payload.get("blocked")
    );
    assert!(payload.get("proposed_grant").is_none());
    assert_eq!(Some(&json!("rerun")), payload.get("replay"));
    // ... and an allow that arrives anyway is a denial
    assert_eq!(
        SandboxAnswer::Deny {
            followup: None,
            remember: false
        },
        decode(
            json!({ "outcome": "always_approve", "scope": { "kind": "net_all" } }),
            &violation,
            &ceiling
        )
    );
}

#[test]
fn approve_without_a_duration_is_a_call_grant_of_the_proposal() {
    let violation = fs_write_violation();
    let ceiling = grants_allowed_ceiling(&violation);
    let answer = decode(
        json!({
            "outcome": "approve",
            "scope": { "kind": "fs_write_root", "value": "/opt/ws-fixture/u/Library/Caches/npm" }
        }),
        &violation,
        &ceiling,
    );
    assert_eq!(
        SandboxAnswer::Allow {
            subject: GrantSubject::FsWriteRoot {
                root: PathBuf::from("/opt/ws-fixture/u/Library/Caches/npm"),
            },
            scope: GrantScope::Call,
            expires: Expiry::Never,
        },
        answer
    );
}

#[test]
fn workspace_duration_gets_the_asked_ttl_within_the_cap() {
    let violation = fs_write_violation();
    let ceiling = grants_allowed_ceiling(&violation);
    let answer = decode(
        json!({
            "outcome": "approve",
            "scope": { "kind": "fs_write_root" },
            "duration": { "kind": "workspace" },
            "expires": { "kind": "ttl", "seconds": 86400 }
        }),
        &violation,
        &ceiling,
    );
    assert_eq!(
        SandboxAnswer::Allow {
            subject: GrantSubject::FsWriteRoot {
                root: PathBuf::from("/opt/ws-fixture/u/Library/Caches/npm"),
            },
            scope: GrantScope::Workspace {
                root: PathBuf::from(WS)
            },
            expires: Expiry::Ttl { seconds: 86400 },
        },
        answer
    );
}

#[test]
fn durations_and_expiries_outside_the_offered_set_clamp_down_never_up() {
    let violation = fs_write_violation();
    let ceiling = grants_allowed_ceiling(&violation);
    // global is not offered under grants_allowed → workspace; never is not offered → 7 d
    let answer = decode(
        json!({
            "outcome": "approve",
            "scope": { "kind": "fs_write_root" },
            "duration": { "kind": "global" },
            "expires": { "kind": "never" }
        }),
        &violation,
        &ceiling,
    );
    assert!(matches!(
        answer,
        SandboxAnswer::Allow {
            scope: GrantScope::Workspace { .. },
            expires: Expiry::Ttl {
                seconds: MAX_PERSISTED_TTL_SECONDS
            },
            ..
        }
    ));
    // a lifetime already over could only be raised, so the allow is refused
    for expires in [
        json!({ "kind": "ttl", "seconds": 0 }),
        json!({ "kind": "at", "unix": NOW }),
        json!({ "kind": "at", "unix": NOW - 60 }),
    ] {
        let answer = decode(
            json!({
                "outcome": "approve",
                "scope": { "kind": "fs_write_root" },
                "duration": { "kind": "workspace" },
                "expires": expires
            }),
            &violation,
            &ceiling,
        );
        assert!(
            matches!(
                answer,
                SandboxAnswer::Deny {
                    remember: false,
                    ..
                }
            ),
            "{answer:?}"
        );
    }
    // a 30-day TTL, an `at` a year out and a malformed (so absent) expiry all clamp to 7 d
    for expires in [
        json!({ "kind": "ttl", "seconds": 30 * 86400 }),
        json!({ "kind": "at", "unix": NOW + 365 * 86400 }),
        json!({ "kind": "ttl" }),
        json!("tomorrow"),
    ] {
        let answer = decode(
            json!({
                "outcome": "approve",
                "scope": { "kind": "fs_write_root" },
                "duration": { "kind": "workspace" },
                "expires": expires
            }),
            &violation,
            &ceiling,
        );
        assert!(
            matches!(
                answer,
                SandboxAnswer::Allow {
                    expires: Expiry::Ttl {
                        seconds: MAX_PERSISTED_TTL_SECONDS
                    },
                    ..
                }
            ),
            "{answer:?}"
        );
    }
    // an `at` within 7 days is kept
    let answer = decode(
        json!({
            "outcome": "approve",
            "scope": { "kind": "fs_write_root" },
            "duration": { "kind": "workspace" },
            "expires": { "kind": "at", "unix": NOW + 3600 }
        }),
        &violation,
        &ceiling,
    );
    assert!(matches!(
        answer,
        SandboxAnswer::Allow {
            expires: Expiry::At { unix },
            ..
        } if unix == NOW + 3600
    ));
    // under always_prompt even "always_approve" + "session" is a call grant
    let prompt = Ceiling::for_violation(ToolApprovalPolicy::AlwaysPrompt, &violation);
    let answer = decode(
        json!({
            "outcome": "always_approve",
            "scope": { "kind": "fs_write_root" },
            "duration": { "kind": "session" }
        }),
        &violation,
        &prompt,
    );
    assert!(matches!(
        answer,
        SandboxAnswer::Allow {
            scope: GrantScope::Call,
            ..
        }
    ));
    // a malformed duration reads as absent: approve → call
    let answer = decode(
        json!({
            "outcome": "approve",
            "scope": { "kind": "fs_write_root", "value": "/opt/ws-fixture/u/Library/Caches/npm" },
            "duration": "forever"
        }),
        &violation,
        &ceiling,
    );
    assert!(matches!(
        answer,
        SandboxAnswer::Allow {
            scope: GrantScope::Call,
            ..
        }
    ));
}

/// The desktop's wrapper can carry the whole reply: the embedded `outcome` and
/// `followup_message` win over the outer ones, so a nested reject is a denial and its message
/// reaches the model.
#[test]
fn a_nested_reply_brings_its_own_outcome_and_followup() {
    let violation = fs_write_violation();
    let ceiling = grants_allowed_ceiling(&violation);
    let embedded = json!({
        "outcome": "reject",
        "followup_message": "use the repo's own cache"
    })
    .to_string();
    assert_eq!(
        SandboxAnswer::Deny {
            followup: Some("use the repo's own cache".to_owned()),
            remember: false
        },
        decode(
            json!({
                "outcome": "approve",
                "scope": { "kind": "bash_command", "value": embedded }
            }),
            &violation,
            &ceiling
        )
    );
}

/// A client with its own prompts off answers every permission request with a plain `approve`
/// (or a generic card's `always_approve` with a tool or bash scope, a duration or an expiry).
/// None of them names a sandbox subject — a duration or expiry alone is the generic approval
/// vocabulary — so none of them grants anything; a bare `reject` stays a plain denial.
#[test]
fn an_approval_naming_no_sandbox_decision_grants_nothing() {
    let violation = fs_write_violation();
    let ceiling = grants_allowed_ceiling(&violation);
    for reply in [
        json!({ "outcome": "approve" }),
        json!({ "outcome": "approve", "tool_call_id": "tc-0192c1a0" }),
        json!({ "outcome": "always_approve" }),
        json!({ "outcome": "always_approve", "scope": { "kind": "tool_scope" } }),
        json!({ "outcome": "always_approve", "duration": { "kind": "workspace" } }),
        json!({ "outcome": "approve", "expires": { "kind": "ttl", "seconds": 3600 } }),
        json!({ "outcome": 3, "duration": { "kind": "session" }, "expires": { "kind": "never" } }),
        json!({ "outcome": "always_approve", "scope": { "kind": "bash_command", "value": "npm ci" } }),
        json!({ "outcome": "approve", "scope": { "kind": "bash_command", "value": "{}" } }),
        json!({ "outcome": "approve", "scope": { "kind": "cmd_prefix", "value": "npm" } }),
    ] {
        assert_eq!(
            SandboxAnswer::NoSandboxDecision,
            decode(reply.clone(), &violation, &ceiling),
            "{reply}"
        );
    }
    assert_eq!(
        SandboxAnswer::Deny {
            followup: None,
            remember: false
        },
        decode(json!({ "outcome": "reject" }), &violation, &ceiling)
    );
    // The desktop's one-call grant is the bare approve plus the subject and duration it chose
    assert!(matches!(
        decode(
            json!({
                "outcome": "approve",
                "scope": {
                    "kind": "bash_command",
                    "value": "{\"scope\":{\"kind\":\"fs_write_root\",\"value\":\"/opt/ws-fixture/u/Library/Caches/npm\"},\"duration\":{\"kind\":\"call\"}}"
                }
            }),
            &violation,
            &ceiling
        ),
        SandboxAnswer::Allow {
            scope: GrantScope::Call,
            ..
        }
    ));
}

#[test]
fn always_approve_without_a_duration_takes_the_strongest_offered_scope() {
    let violation = fs_write_violation();
    let unattended = Ceiling::for_violation(ToolApprovalPolicy::UnattendedAllowed, &violation);
    let answer = decode(
        json!({
            "outcome": "always_approve",
            "scope": { "kind": "fs_write_root" },
            "expires": { "kind": "never" }
        }),
        &violation,
        &unattended,
    );
    assert!(matches!(
        answer,
        SandboxAnswer::Allow {
            scope: GrantScope::Global,
            expires: Expiry::Never,
            ..
        }
    ));
}

#[test]
fn allow_on_a_deny_only_card_is_a_denial() {
    let protected = protected_violation();
    let ceiling = Ceiling::for_violation(ToolApprovalPolicy::UnattendedAllowed, &protected);
    let answer = decode(
        json!({ "outcome": "always_approve", "duration": { "kind": "global" } }),
        &protected,
        &ceiling,
    );
    assert_eq!(
        SandboxAnswer::Deny {
            followup: None,
            remember: false
        },
        answer
    );
}

#[test]
fn answered_subject_of_the_right_kind_replaces_the_proposal() {
    let violation = fs_write_violation();
    let ceiling = grants_allowed_ceiling(&violation);
    let answer = decode(
        json!({
            "outcome": "approve",
            "scope": { "kind": "fs_write_root", "value": "/opt/ws-fixture/u/Library/Caches/npm/_cacache" }
        }),
        &violation,
        &ceiling,
    );
    assert!(matches!(
        answer,
        SandboxAnswer::Allow {
            subject: GrantSubject::FsWriteRoot { root },
            ..
        } if root == Path::new("/opt/ws-fixture/u/Library/Caches/npm/_cacache")
    ));
    // an unknown kind or a malformed scope names no sandbox decision, whatever the duration
    for scope in [
        json!({ "kind": "cmd_prefix", "value": "npm" }),
        json!({ "kind": "something_new", "value": "/x" }),
        json!("fs_write_root"),
    ] {
        assert_eq!(
            SandboxAnswer::NoSandboxDecision,
            decode(
                json!({ "outcome": "approve", "scope": scope, "duration": { "kind": "call" } }),
                &violation,
                &ceiling,
            ),
            "{scope}"
        );
    }
    // a subject of another kind or a relative path falls back to the proposal
    for scope in [
        json!({ "kind": "net_host", "value": "example.com" }),
        json!({ "kind": "fs_write_root", "value": "relative/dir" }),
        json!({ "kind": "fs_read", "value": "/etc" }),
    ] {
        let answer = decode(
            json!({ "outcome": "approve", "scope": scope, "duration": { "kind": "call" } }),
            &violation,
            &ceiling,
        );
        assert!(
            matches!(
                &answer,
                SandboxAnswer::Allow { subject, .. } if *subject == violation.proposed.clone().unwrap()
            ),
            "{answer:?}"
        );
    }
}

fn typed_folder(value: &str) -> Value {
    json!({
        "outcome": "approve",
        "scope": { "kind": "fs_write_root", "value": value }
    })
}

/// A folder the user types is tied to the violation: it must hold the blocked path and lie at
/// or under the root the card offered. Anything else — an unrelated folder, a parent of the
/// offered root, the home directory, `/`, a top-level system directory — is refused outright:
/// the user allowed what the card showed, and the answer is never widened to the proposal.
#[test]
fn a_typed_folder_untied_to_the_violation_or_too_broad_is_refused() {
    let violation = fs_write_violation();
    let ceiling = grants_allowed_ceiling(&violation);
    let refused = [
        ("/", "too broad"),
        (HOME, "too broad"),
        ("/home", "too broad"),
        ("/usr", "too broad"),
        ("/tmp/x", "too broad"),
        (WS, "does not contain the blocked path"),
        (
            "/opt/ws-fixture/u/Documents",
            "does not contain the blocked path",
        ),
        ("/opt/ws-fixture/u/Library/Caches", "wider than the offered"),
        (
            "/opt/ws-fixture/u/Library/Caches/npm/other",
            "does not contain the blocked path",
        ),
    ];
    for (typed, why) in refused {
        let answer = decode(typed_folder(typed), &violation, &ceiling);
        assert!(
            matches!(&answer, SandboxAnswer::SubjectRefused { reason } if reason.contains(why)),
            "{typed}: {answer:?}"
        );
    }
    // the offered root itself and a folder below it that still holds the blocked path are fine
    for fine in [
        "/opt/ws-fixture/u/Library/Caches/npm",
        "/opt/ws-fixture/u/Library/Caches/npm/_cacache",
    ] {
        let answer = decode(typed_folder(fine), &violation, &ceiling);
        assert!(
            matches!(
                &answer,
                SandboxAnswer::Allow { subject: GrantSubject::FsWriteRoot { root }, .. } if root == Path::new(fine)
            ),
            "{fine}: {answer:?}"
        );
    }
    // a read card ties the same way
    let read = fs_read_violation();
    let read_ceiling = grants_allowed_ceiling(&read);
    let typed_read = |value: &str| json!({ "outcome": "approve", "scope": { "kind": "fs_read", "value": value } });
    assert!(matches!(
        decode(typed_read("/opt/ws-fixture/u/shared"), &read, &read_ceiling),
        SandboxAnswer::SubjectRefused { .. }
    ));
    assert!(matches!(
        decode(
            typed_read("/opt/ws-fixture/u/shared/config"),
            &read,
            &read_ceiling
        ),
        SandboxAnswer::Allow {
            subject: GrantSubject::FsRead { .. },
            ..
        }
    ));
}

/// On the card that offered the build-cache family, a typed folder is a folder grant held to the
/// family's trees: one inside `~/.cargo/registry` that holds the blocked path is fine; `~/.cargo`
/// itself (credentials, binaries) is outside every tree and refused.
#[test]
fn a_typed_folder_on_a_build_caches_card_is_held_to_the_familys_trees() {
    let violation = build_caches_violation();
    let ceiling = grants_allowed_ceiling(&violation);
    let inside = "/opt/ws-fixture/u/.cargo/registry/src";
    let answer = decode(typed_folder(inside), &violation, &ceiling);
    assert!(
        matches!(
            &answer,
            SandboxAnswer::Allow { subject: GrantSubject::FsWriteRoot { root }, .. } if root == Path::new(inside)
        ),
        "{answer:?}"
    );
    let answer = decode(
        typed_folder("/opt/ws-fixture/u/.cargo"),
        &violation,
        &ceiling,
    );
    assert!(
        matches!(&answer, SandboxAnswer::SubjectRefused { reason } if reason.contains("not inside the offered build caches")),
        "{answer:?}"
    );
    // with no home directory to bound the family, a typed folder cannot be tied to it
    let homeless = ProposalBounds {
        user_home: None,
        ..bounds()
    };
    let answer = decode_reply(&typed_folder(inside), &violation, &ceiling, &homeless, NOW);
    assert!(
        matches!(&answer, SandboxAnswer::SubjectRefused { .. }),
        "{answer:?}"
    );
}

/// The typed folder is folded at the reply boundary, never resolved — the breadth cap sees the
/// home directory behind `<ws>/../..`, a folded spelling of a fine folder is tied and stored
/// folded, and a folder through a link is refused, never recorded as where the link leads.
#[test]
fn a_typed_folder_is_folded_not_resolved_before_the_breadth_cap_and_the_store_see_it() {
    let violation = fs_write_violation();
    let ceiling = grants_allowed_ceiling(&violation);
    let dotted_home = format!("{WS}/src/../..");
    assert_eq!(Some(Path::new(HOME)), Path::new(WS).parent());
    let answer = decode(typed_folder(&dotted_home), &violation, &ceiling);
    assert!(
        matches!(&answer, SandboxAnswer::SubjectRefused { reason } if reason.contains("too broad")),
        "a `..` spelling of the home is as too broad as the home: {answer:?}"
    );
    let folded = "/opt/ws-fixture/u/Library/Caches/npm/_cacache";
    let answer = decode(
        typed_folder("/opt/ws-fixture/u/Library/Caches/npm/tmp/../_cacache/./"),
        &violation,
        &ceiling,
    );
    assert!(
        matches!(
            &answer,
            SandboxAnswer::Allow { subject: GrantSubject::FsWriteRoot { root }, .. }
                if root == Path::new(folded)
        ),
        "{answer:?}"
    );

    #[cfg(unix)]
    {
        let tmp = tempfile::tempdir().unwrap();
        let cache = dunce::canonicalize(tmp.path()).unwrap().join("cache");
        let (docs, real) = (cache.with_file_name("docs"), cache.join("real"));
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&docs, cache.join("npm")).unwrap();
        std::os::unix::fs::symlink(&real, cache.join("l")).unwrap();
        // A link leading out of the offered folder, and one staying inside it
        for (offered, typed, path) in [
            (cache.join("npm"), cache.join("npm"), docs.join("x")),
            (cache.clone(), cache.join("l"), real.join("x")),
        ] {
            let read = Violation {
                blocked: Blocked::FsRead { path: path.clone() },
                proposed: Some(GrantSubject::FsRead {
                    root: offered.clone(),
                }),
                ..fs_write_violation()
            };
            let write = Violation {
                blocked: Blocked::FsWrite { path },
                proposed: Some(GrantSubject::FsWriteRoot { root: offered }),
                ..fs_write_violation()
            };
            for (kind, violation) in [("fs_write_root", write), ("fs_read", read)] {
                let reply =
                    json!({ "outcome": "approve", "scope": { "kind": kind, "value": typed } });
                let answer = decode(reply, &violation, &grants_allowed_ceiling(&violation));
                assert!(
                    matches!(&answer, SandboxAnswer::SubjectRefused { .. }),
                    "{kind} {typed:?}: {answer:?}"
                );
            }
        }
    }
}

/// A typed host must be the host the connection was to, and no wider than the pattern the card
/// offered: the exact host is fine and keeps the blocked port; a wildcard covering it, or the
/// all-hosts answer, is wider than the offered host and refused; a host that does not match is
/// refused. A connection whose host the kernel masked offers the all-hosts pattern, so `net_all`
/// (or a typed `*`) is its answer, and a typed host — which nothing ties to the connection — is
/// refused.
#[test]
fn a_typed_host_is_tied_to_the_connection_and_net_all_only_where_it_was_offered() {
    let violation = net_violation();
    let ceiling = grants_allowed_ceiling(&violation);
    let typed_host = |value: &str| json!({ "outcome": "approve", "scope": { "kind": "net_host", "value": value } });
    let net_all = json!({ "outcome": "approve", "scope": { "kind": "net_all" }, "duration": { "kind": "workspace" } });
    let answer = decode(typed_host("registry.npmjs.org"), &violation, &ceiling);
    assert!(
        matches!(
            &answer,
            SandboxAnswer::Allow { subject: GrantSubject::NetHost { host, port: Some(443) }, .. }
                if *host == HostPattern::new("registry.npmjs.org")
        ),
        "{answer:?}"
    );
    for (reply, why) in [
        (typed_host("*.npmjs.org"), "wider than the offered"),
        (net_all.clone(), "wider than the offered"),
        (typed_host("example.com"), "does not match the blocked host"),
        (
            typed_host("*.example.com"),
            "does not match the blocked host",
        ),
    ] {
        let answer = decode(reply.clone(), &violation, &ceiling);
        assert!(
            matches!(&answer, SandboxAnswer::SubjectRefused { reason } if reason.contains(why)),
            "{reply}: {answer:?}"
        );
    }
    // the kernel masked the host: the all-hosts answer is the one offered
    let masked = Violation {
        blocked: Blocked::Net {
            host: None,
            port: Some(443),
        },
        proposed: Some(GrantSubject::NetHost {
            host: HostPattern::all(),
            port: Some(443),
        }),
        ..net_violation()
    };
    let masked_ceiling = grants_allowed_ceiling(&masked);
    let answer = decode(net_all, &masked, &masked_ceiling);
    assert!(matches!(
        answer,
        SandboxAnswer::Allow {
            subject: GrantSubject::NetHost { host, port: None },
            scope: GrantScope::Workspace { .. },
            ..
        } if host.is_all()
    ));
    let answer = decode(typed_host("*"), &masked, &masked_ceiling);
    assert!(matches!(
        answer,
        SandboxAnswer::Allow {
            subject: GrantSubject::NetHost { host, port: Some(443) },
            ..
        } if host.is_all()
    ));
    let answer = decode(typed_host("example.com"), &masked, &masked_ceiling);
    assert!(
        matches!(&answer, SandboxAnswer::SubjectRefused { reason } if reason.contains("host is not known")),
        "{answer:?}"
    );
}

/// A typed host carrying the held port is split where the port starts, never at an IPv6
/// literal's own colons: `[::1]:443` answers a connection to `::1` port 443 as
/// `registry.npmjs.org:443` answers one to that host, and a bracketed literal naming another
/// port is refused like any other.
#[test]
fn a_typed_ipv6_host_with_the_held_port_answers_its_connection() {
    let net_to = |host: &str, port: u16| Violation {
        blocked: Blocked::Net {
            host: Some(host.to_owned()),
            port: Some(port),
        },
        proposed: Some(GrantSubject::NetHost {
            host: HostPattern::new(host),
            port: None,
        }),
        ..net_violation()
    };
    let typed_host = |value: &str| json!({ "outcome": "approve", "scope": { "kind": "net_host", "value": value } });
    for (value, host, port) in [
        ("[::1]:443", "::1", 443),
        ("[2001:db8::1]:8443", "2001:db8::1", 8443),
        ("registry.npmjs.org:443", "registry.npmjs.org", 443),
        ("registry.npmjs.org", "registry.npmjs.org", 443),
    ] {
        let connection = net_to(host, port);
        let answer = decode(
            typed_host(value),
            &connection,
            &grants_allowed_ceiling(&connection),
        );
        assert!(
            matches!(
                &answer,
                SandboxAnswer::Allow { subject: GrantSubject::NetHost { host: granted, port: Some(held) }, .. }
                    if *granted == HostPattern::new(host) && *held == port
            ),
            "{value}: {answer:?}"
        );
    }
    let connection = net_to("::1", 443);
    let answer = decode(
        typed_host("[::1]:80"),
        &connection,
        &grants_allowed_ceiling(&connection),
    );
    assert!(
        matches!(&answer, SandboxAnswer::SubjectRefused { reason } if reason.contains("a port other than the held 443")),
        "{answer:?}"
    );
}

/// The desktop's `sandboxAllow` rides the strict `ToolPermission` proto: `always_approve` with
/// `scope.kind = "bash_command"` whose value is the reply JSON.
#[test]
fn bash_command_value_carrying_the_reply_json_is_unfolded() {
    let violation = fs_write_violation();
    let ceiling = grants_allowed_ceiling(&violation);
    let embedded = json!({
        "scope": { "kind": "fs_write_root", "value": "/opt/ws-fixture/u/Library/Caches/npm/_cacache" },
        "duration": { "kind": "workspace" },
        "expires": { "kind": "ttl", "seconds": 3600 }
    });
    let answer = decode(
        json!({
            "outcome": "always_approve",
            "scope": { "kind": "bash_command", "value": embedded.to_string() }
        }),
        &violation,
        &ceiling,
    );
    assert_eq!(
        SandboxAnswer::Allow {
            subject: GrantSubject::FsWriteRoot {
                root: PathBuf::from("/opt/ws-fixture/u/Library/Caches/npm/_cacache"),
            },
            scope: GrantScope::Workspace {
                root: PathBuf::from(WS)
            },
            expires: Expiry::Ttl { seconds: 3600 },
        },
        answer
    );

    // A real always-allow bash prefix never starts with `{`: it is a generic card's answer, not
    // a sandbox decision, so it grants nothing
    let prefix = decode(
        json!({
            "outcome": "always_approve",
            "scope": { "kind": "bash_command", "value": "npm ci" }
        }),
        &violation,
        &ceiling,
    );
    assert_eq!(SandboxAnswer::NoSandboxDecision, prefix);

    // Broken JSON in the value is never an error: the scope is dropped, and with it the only
    // sandbox decision the reply could have carried
    let broken = decode(
        json!({
            "outcome": "approve",
            "scope": { "kind": "bash_command", "value": "{not json" }
        }),
        &violation,
        &ceiling,
    );
    assert_eq!(SandboxAnswer::NoSandboxDecision, broken);

    // `sandboxDeny` is `reject` plus a followup
    assert_eq!(
        SandboxAnswer::Deny {
            followup: Some("use the workspace dir".to_owned()),
            remember: false
        },
        decode(
            json!({
                "outcome": "reject",
                "scope": { "kind": "bash_command", "value": "{}" },
                "followup_message": "use the workspace dir"
            }),
            &violation,
            &ceiling,
        )
    );
}

#[test]
fn reject_variants_and_cancel_decode() {
    let violation = fs_write_violation();
    let ceiling = grants_allowed_ceiling(&violation);
    let decode = |reply: Value| decode(reply, &violation, &ceiling);
    assert_eq!(
        SandboxAnswer::Deny {
            followup: None,
            remember: false
        },
        decode(json!({ "outcome": "reject" }))
    );
    assert_eq!(
        SandboxAnswer::Deny {
            followup: Some("use the workspace cache".to_owned()),
            remember: false
        },
        decode(json!({ "outcome": "reject", "followup_message": "use the workspace cache" }))
    );
    // the model sees at most `MAX_FOLLOWUP_CHARS` of it, cut on a character boundary
    let long = "é".repeat(MAX_FOLLOWUP_CHARS + 500);
    assert_eq!(
        SandboxAnswer::Deny {
            followup: Some("é".repeat(MAX_FOLLOWUP_CHARS)),
            remember: false
        },
        decode(json!({ "outcome": "reject", "followup_message": long }))
    );
    assert_eq!(
        SandboxAnswer::Deny {
            followup: None,
            remember: true
        },
        decode(json!({ "outcome": "always_reject" }))
    );
    assert_eq!(
        SandboxAnswer::Cancelled,
        decode(json!({ "outcome": "cancelled" }))
    );
    assert_eq!(
        SandboxAnswer::Deny {
            followup: None,
            remember: false
        },
        decode(json!({ "outcome": "something-new" })),
        "an unknown outcome fails closed"
    );
    for not_a_reply in [json!({}), json!(null), json!("approve"), json!(["approve"])] {
        assert_eq!(
            SandboxAnswer::Deny {
                followup: None,
                remember: false
            },
            decode(not_a_reply),
        );
    }
    // numeric outcomes as the legacy renderer sends them
    assert!(matches!(
        decode(json!({
            "outcome": 1,
            "scope": { "kind": "fs_write_root" },
            "duration": { "kind": "call" }
        })),
        SandboxAnswer::Allow { .. }
    ));
    assert_eq!(
        SandboxAnswer::NoSandboxDecision,
        decode(json!({ "outcome": 1 }))
    );
    assert!(matches!(
        decode(json!({ "outcome": 4 })),
        SandboxAnswer::Deny { remember: true, .. }
    ));
    assert!(matches!(
        decode(json!({ "outcome": 9 })),
        SandboxAnswer::Deny {
            remember: false,
            ..
        }
    ));
}
