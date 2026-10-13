use super::{SandboxPath, SettleContext, ViolationSettlement, settle_violation};
use crate::permission::PermissionHookTransport;
use crate::permission::hub_gate::PromptGate;
use crate::permission::sandbox_wire::SANDBOX_VIOLATION_KIND;
use crate::sandbox::metrics;
use crate::sandbox::{
    BackendSource, CallOwner, WorkspaceSandbox, WorkspaceSandboxConfig, WorkspaceSandboxError,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use xai_grok_egress_proxy::EgressProxyOptions;
use xai_grok_sandbox::command::grants::{
    Expiry, FixedClock, Grant, GrantDecision, GrantScope, GrantSubject, HostPattern,
};
use xai_grok_sandbox::command::violation::{
    Blocked, Capability, Disposition, InformationalReason, Replay, Violation,
};
use xai_grok_sandbox::command::{BackendName, CallId, GitConfigEnv, SandboxMode, canonical_path};
use xai_grok_telemetry::events::{SandboxBlockedKind, SandboxCommandOutcome, SandboxSettlement};
use xai_grok_tools::types::tool::{ToolKind, ToolNamespace};
use xai_tool_runtime::ToolApprovalPolicy;
const NOW: i64 = 1_800_000_000;
struct StubTransport {
    reply: Result<Value, String>,
    seen: parking_lot::Mutex<Vec<Value>>,
}
impl StubTransport {
    fn replying(reply: Value) -> Arc<StubTransport> {
        Arc::new(StubTransport {
            reply: Ok(reply),
            seen: parking_lot::Mutex::new(Vec::new()),
        })
    }
    fn failing(message: &str) -> Arc<StubTransport> {
        Arc::new(StubTransport {
            reply: Err(message.to_owned()),
            seen: parking_lot::Mutex::new(Vec::new()),
        })
    }
    fn prompts(&self) -> Vec<Value> {
        self.seen.lock().clone()
    }
    /// An informational card is posted off the tool call; give that task its turns.
    async fn wait_for_prompt(&self) -> Value {
        for _ in 0..100 {
            if let Some(prompt) = self.seen.lock().first().cloned() {
                return prompt;
            }
            tokio::task::yield_now().await;
        }
        panic!("no card was posted");
    }
}
fn dyn_transport(transport: &Arc<StubTransport>) -> Option<Arc<dyn PermissionHookTransport>> {
    Some(transport.clone() as Arc<dyn PermissionHookTransport>)
}
#[async_trait]
impl PermissionHookTransport for StubTransport {
    async fn request_permission(&self, payload: Value) -> Result<Value, String> {
        self.seen.lock().push(payload);
        self.reply.clone()
    }
}
struct Fixture {
    tmp: tempfile::TempDir,
    root: PathBuf,
    sandbox: Arc<WorkspaceSandbox>,
    call: CallId,
}
impl Fixture {
    async fn new() -> Fixture {
        Fixture::with_proxy(false).await
    }
    /// With `proxied`, the folder's own egress proxy is started on a loopback port, as the
    /// daemon's serve does under `observe` and `enforce`.
    async fn with_proxy(proxied: bool) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("ws");
        let grok_home = tmp.path().join("grok-home");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&grok_home).unwrap();
        let layer = crate::sandbox_mode::workspace_config_path(&root);
        std::fs::create_dir_all(layer.parent().unwrap()).unwrap();
        std::fs::write(&layer, "[sandbox]\nmode = \"enforce\"\n").unwrap();
        let sandbox = WorkspaceSandbox::open(WorkspaceSandboxConfig {
            workspace_root: root.clone(),
            control_socket_dir: grok_home.join("daemon"),
            grok_home,
            user_home: Some(tmp.path().join("home")),
            git_env: GitConfigEnv::default(),
            remote: None,
            backend: BackendSource::Fixed(None),
            clock: Arc::new(FixedClock::at(NOW)),
        })
        .await;
        let sandbox = Arc::new(sandbox);
        if proxied {
            sandbox
                .start_network(EgressProxyOptions::default())
                .await
                .unwrap();
        }
        let call = CallId::tool("call-1");
        sandbox.open_settlement(&call).unwrap();
        Fixture {
            tmp,
            root,
            sandbox,
            call,
        }
    }
    fn outside(&self) -> PathBuf {
        let dir = self.tmp.path().join("caches");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn fs_write_violation(&self) -> Violation {
        let root = self.outside();
        Violation {
            blocked: Blocked::FsWrite {
                path: root.join("index"),
            },
            proposed: Some(GrantSubject::FsWriteRoot { root }),
            disposition: Disposition::Grantable,
            partial_output: None,
            replay: Replay::Rerun,
            exit_code: Some(1),
            stderr_snippet: "EACCES".to_owned(),
        }
    }
    /// A connection the proxy held mid-command (the decider's violation): the one grantable
    /// network violation there is.
    fn held_net_violation(&self) -> Violation {
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
            partial_output: None,
            replay: Replay::Resume {
                hold_id: "hold-1".to_owned(),
            },
            exit_code: None,
            stderr_snippet: String::new(),
        }
    }
    /// A network denial the post-run decoder read: the proxy never saw the connection, so it is
    /// informational (`unproxied_network`).
    fn unproxied_net_violation(&self) -> Violation {
        Violation {
            blocked: Blocked::Net {
                host: None,
                port: Some(443),
            },
            proposed: None,
            disposition: Disposition::informational(InformationalReason::UnproxiedNetwork),
            partial_output: None,
            replay: Replay::Rerun,
            exit_code: Some(7),
            stderr_snippet: String::new(),
        }
    }
    fn capability_violation(&self) -> Violation {
        Violation {
            blocked: Blocked::Capability {
                what: Capability::Ptrace,
            },
            proposed: None,
            disposition: Disposition::informational(InformationalReason::Capability),
            partial_output: None,
            replay: Replay::Rerun,
            exit_code: Some(1),
            stderr_snippet: "ptrace: Operation not permitted".to_owned(),
        }
    }
    /// A coarse denial the decoder could not tie to a target: telemetry, never a card.
    fn coarse_unknown_violation(&self) -> Violation {
        Violation {
            blocked: Blocked::Unknown {
                stderr_snippet: "Permission denied (publickey)".to_owned(),
            },
            proposed: None,
            disposition: Disposition::Grantable,
            partial_output: None,
            replay: Replay::Rerun,
            exit_code: Some(128),
            stderr_snippet: "Permission denied (publickey)".to_owned(),
        }
    }
    fn ctx(&self, policy: ToolApprovalPolicy) -> SettleContext<'_> {
        SettleContext {
            sandbox: &self.sandbox,
            epoch: None,
            policy,
            call: &self.call,
            session_id: "sess-1",
            command: "npm install",
            mode: SandboxMode::Enforce,
            backend: Some(BackendName::Seatbelt),
            replayed_under: None,
        }
    }
    fn ctx_after_replay<'a>(
        &'a self,
        policy: ToolApprovalPolicy,
        granted: &'a GrantSubject,
    ) -> SettleContext<'a> {
        SettleContext {
            replayed_under: Some(granted),
            ..self.ctx(policy)
        }
    }
    /// The context of another call in `session_id`, its settlement opened as `finish` would.
    fn ctx_for<'a>(&'a self, call: &'a CallId, session_id: &'a str) -> SettleContext<'a> {
        self.sandbox.open_settlement(call).unwrap();
        SettleContext {
            call,
            session_id,
            ..self.ctx(ToolApprovalPolicy::GrantsAllowed)
        }
    }
}
/// The card's allow as the desktop sends it: a sandbox `scope` (no value, so the proposal
/// stands) and the duration picked.
fn allow_reply(scope: &str, expires: Option<Value>) -> Value {
    let mut reply = serde_json::Map::from_iter([
        ("outcome".to_owned(), json!("approve")),
        ("tool_call_id".to_owned(), json!("call-1")),
        ("scope".to_owned(), json!({ "kind": "fs_write_root" })),
        ("duration".to_owned(), json!({ "kind": scope })),
    ]);
    if let Some(expires) = expires {
        reply.insert("expires".to_owned(), expires);
    }
    Value::Object(reply)
}
#[tokio::test]
async fn without_a_transport_the_denial_stands() {
    let fx = Fixture::new().await;
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.fs_write_violation(),
        None,
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(
        model_text.contains("no channel to the session owner"),
        "{model_text}"
    );
    assert!(fx.sandbox.live_grants().await.is_empty());
}
/// The "second target" case: the replayed run stopped at another target; the model is
/// told the grant applied and that the next run asks about this one. The same target again means
/// the grant did not cover what the command did.
#[tokio::test]
async fn a_second_violation_after_the_replay_is_final_and_never_prompts() {
    let fx = Fixture::new().await;
    let transport = StubTransport::replying(allow_reply("call", None));
    let granted = GrantSubject::FsWriteRoot {
        root: fx.tmp.path().join("elsewhere"),
    };
    let settled = settle_violation(
        fx.ctx_after_replay(ToolApprovalPolicy::GrantsAllowed, &granted),
        fx.fs_write_violation(),
        dyn_transport(&transport),
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(
        model_text.contains("already run again under the user's grant for writes under"),
        "{model_text}"
    );
    assert!(
        model_text.contains("Run it again to be asked about this one."),
        "{model_text}"
    );
    assert!(transport.prompts().is_empty());
    let same = GrantSubject::FsWriteRoot { root: fx.outside() };
    let settled = settle_violation(
        fx.ctx_after_replay(ToolApprovalPolicy::GrantsAllowed, &same),
        fx.fs_write_violation(),
        dyn_transport(&transport),
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(
        model_text.contains("did not unblock it; it is not run a third time"),
        "{model_text}"
    );
    assert!(transport.prompts().is_empty());
}
/// A coarse `Unknown` is recorded and nothing else — no card, no text.
#[tokio::test]
async fn a_coarse_unknown_is_recorded_without_a_card_or_text() {
    let fx = Fixture::new().await;
    let transport = StubTransport::replying(allow_reply("call", None));
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.coarse_unknown_violation(),
        dyn_transport(&transport),
    )
    .await;
    assert!(
        matches!(settled, ViolationSettlement::Recorded),
        "{settled:?}"
    );
    tokio::task::yield_now().await;
    assert!(transport.prompts().is_empty());
    assert!(fx.sandbox.live_grants().await.is_empty());
}
#[tokio::test]
async fn an_allow_for_this_call_records_a_call_grant_and_asks_for_the_replay() {
    let fx = Fixture::new().await;
    let transport = StubTransport::replying(allow_reply("call", None));
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.fs_write_violation(),
        dyn_transport(&transport),
    )
    .await;
    let ViolationSettlement::Replay { grant } = settled else {
        panic!("{settled:?}");
    };
    assert_eq!(GrantScope::Call, grant.scope);
    assert_eq!(Expiry::Never, grant.expires);
    assert_eq!(GrantDecision::Allow, grant.decision);
    assert_eq!("hub:sess-1", grant.granted_by);
    let via = grant.via.expect("provenance");
    assert_eq!("npm install", via.command);
    assert_eq!("call-1", via.tool_call_id);
    assert_eq!(
        GrantSubject::FsWriteRoot { root: fx.outside() },
        grant.subject
    );
    assert!(
        fx.sandbox.live_grants().await.is_empty(),
        "a call grant is consumed by the replay, not stored"
    );
    let prompts = transport.prompts();
    let [card] = prompts.as_slice() else {
        panic!("one card, got {prompts:?}");
    };
    assert_eq!(Some(&json!(SANDBOX_VIOLATION_KIND)), card.get("kind"));
    assert_eq!(Some(&json!("call-1")), card.get("tool_call_id"));
    assert_eq!(Some(&json!("npm install")), card.get("bash_command"));
    assert_eq!(Some(&json!("fs_write")), card.pointer("/blocked/kind"));
    assert_eq!(Some(&json!("seatbelt")), card.get("backend"));
    assert_eq!(Some(&json!(false)), card.get("reduced_sandbox"));
    assert!(
        card.get(SANDBOX_VIOLATION_KIND)
            .is_some_and(Value::is_object),
        "relay copy: {card}"
    );
}
#[tokio::test]
async fn a_workspace_allow_is_persisted_with_the_clamped_expiry() {
    let fx = Fixture::new().await;
    let transport = StubTransport::replying(allow_reply(
        "workspace",
        Some(json!({ "kind": "ttl", "seconds": 999_999_999 })),
    ));
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.fs_write_violation(),
        dyn_transport(&transport),
    )
    .await;
    let ViolationSettlement::Replay { grant } = settled else {
        panic!("{settled:?}");
    };
    assert_eq!(
        GrantScope::Workspace {
            root: fx.root.clone()
        },
        grant.scope
    );
    assert_eq!(
        Expiry::Ttl {
            seconds: crate::permission::sandbox_wire::MAX_PERSISTED_TTL_SECONDS
        },
        grant.expires
    );
    let live = fx.sandbox.live_grants().await;
    let [stored] = live.as_slice() else {
        panic!("one live grant, got {live:?}");
    };
    assert_eq!(grant.id, stored.id);
}
#[tokio::test]
async fn always_prompt_tenants_clamp_a_workspace_ask_down_to_this_call() {
    let fx = Fixture::new().await;
    let transport = StubTransport::replying(allow_reply("workspace", None));
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::AlwaysPrompt),
        fx.fs_write_violation(),
        dyn_transport(&transport),
    )
    .await;
    let ViolationSettlement::Replay { grant } = settled else {
        panic!("{settled:?}");
    };
    assert_eq!(GrantScope::Call, grant.scope);
    assert!(fx.sandbox.live_grants().await.is_empty());
    let card = transport.wait_for_prompt().await;
    assert_eq!(Some(&json!(["call"])), card.get("offered_scopes"));
}
#[tokio::test]
async fn a_deny_keeps_the_denial_and_carries_the_followup_to_the_model() {
    let fx = Fixture::new().await;
    let transport = StubTransport::replying(json!({
        "outcome": "reject",
        "tool_call_id": "call-1",
        "followup_message": "use the vendored copy",
    }));
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.fs_write_violation(),
        dyn_transport(&transport),
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(
        model_text.contains("User says: use the vendored copy"),
        "{model_text}"
    );
    assert!(fx.sandbox.live_grants().await.is_empty());
}
/// A client with its own prompts off answers every permission request with a plain `approve`:
/// on a sandbox card that names no decision, so the denial stands — no grant, no replay, the
/// model told nobody decided, the violation counted as denied. A bare `reject` is a denial too.
#[tokio::test]
async fn a_bare_approve_or_reject_keeps_the_denial() {
    let fx = Fixture::new().await;
    for (reply, told) in [
        (
            json!({ "outcome": "approve", "tool_call_id": "call-1" }),
            "without a sandbox decision",
        ),
        (
            json!({ "outcome": "reject", "tool_call_id": "call-1" }),
            "User kept it blocked",
        ),
    ] {
        let before = metrics::violation_total(
            SandboxMode::Enforce,
            SandboxBlockedKind::FsWrite,
            SandboxSettlement::Denied,
        );
        let transport = StubTransport::replying(reply.clone());
        let settled = settle_violation(
            fx.ctx(ToolApprovalPolicy::GrantsAllowed),
            fx.fs_write_violation(),
            dyn_transport(&transport),
        )
        .await;
        let ViolationSettlement::Denied { model_text } = settled else {
            panic!("{reply}: {settled:?}");
        };
        assert!(model_text.contains(told), "{reply}: {model_text}");
        assert_eq!(1, transport.prompts().len(), "{reply}: the card was posted");
        assert!(fx.sandbox.live_grants().await.is_empty(), "{reply}");
        assert!(
            metrics::violation_total(
                SandboxMode::Enforce,
                SandboxBlockedKind::FsWrite,
                SandboxSettlement::Denied,
            ) > before,
            "{reply}: counted as a denial"
        );
    }
}
type ViolationOf = fn(&Fixture) -> Violation;
/// Every outcome is counted once, when it is settled, under the settlement it names: an uncarded
/// coarse denial as observed, a kept denial as denied, an allow as a replay or, for a held
/// connection, a resume.
#[tokio::test]
async fn each_outcome_is_counted_under_the_settlement_it_names() {
    let cases: [(ViolationOf, bool, SandboxSettlement); 4] = [
        (
            Fixture::coarse_unknown_violation,
            true,
            SandboxSettlement::Observed,
        ),
        (
            Fixture::fs_write_violation,
            false,
            SandboxSettlement::Denied,
        ),
        (Fixture::fs_write_violation, true, SandboxSettlement::Replay),
        (Fixture::held_net_violation, true, SandboxSettlement::Resume),
    ];
    for (violation, answered, counted) in cases {
        let fx = Fixture::with_proxy(true).await;
        let violation = violation(&fx);
        let transport = answered.then(|| StubTransport::replying(allow_reply("session", None)));
        let kind = metrics::violation_settled_event(SandboxMode::Enforce, &violation, counted).kind;
        let before = metrics::violation_total(SandboxMode::Enforce, kind, counted);
        let settled = settle_violation(
            fx.ctx(ToolApprovalPolicy::GrantsAllowed),
            violation.clone(),
            transport.as_ref().and_then(dyn_transport),
        )
        .await;
        assert_eq!(counted, settled.counted_as(&violation), "{settled:?}");
        assert!(
            metrics::violation_total(SandboxMode::Enforce, kind, counted) > before,
            "{counted:?}"
        );
    }
}
/// A settled violation and a finished command are counted and reported in one vocabulary: the
/// series a settlement bumps is labelled with the very values its event carries.
#[tokio::test]
async fn counters_and_events_spell_the_same_values() {
    let fx = Fixture::new().await;
    for (violation, kind) in [
        (fx.fs_write_violation(), "fs_write"),
        (fx.held_net_violation(), "net"),
        (fx.capability_violation(), "capability"),
    ] {
        for settlement in [SandboxSettlement::Replay, SandboxSettlement::Denied] {
            let event =
                metrics::violation_settled_event(SandboxMode::Enforce, &violation, settlement);
            let fields = serde_json::to_value(event).unwrap();
            assert_eq!(Some(&json!(kind)), fields.get("kind"));
            assert_eq!(
                json!(["mode", "kind", "settlement"].map(|field| fields.get(field))),
                json!(metrics::violation_labels(&event)),
                "{kind}"
            );
            let before = metrics::violation_total(SandboxMode::Enforce, event.kind, settlement);
            metrics::violation_settled(SandboxMode::Enforce, &violation, settlement);
            assert!(
                metrics::violation_total(SandboxMode::Enforce, event.kind, settlement) > before,
                "{kind}: the settlement bumps the series its event names"
            );
        }
    }
    let event = metrics::command_ended_event(
        SandboxMode::Enforce,
        Some(BackendName::Seatbelt),
        SandboxCommandOutcome::WrapFailed,
    );
    assert_eq!(
        json!({ "mode": "enforce", "backend": "seatbelt", "outcome": "wrap_failed" }),
        serde_json::to_value(event).unwrap()
    );
    let bare = metrics::command_ended_event(SandboxMode::Observe, None, SandboxCommandOutcome::Ran);
    assert_eq!(
        json!({ "mode": "observe", "outcome": "ran" }),
        serde_json::to_value(bare).unwrap()
    );
}
#[tokio::test]
async fn always_reject_persists_a_workspace_deny_row() {
    let fx = Fixture::new().await;
    let transport = StubTransport::replying(json!({
        "outcome": "always_reject",
        "tool_call_id": "call-1",
    }));
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.fs_write_violation(),
        dyn_transport(&transport),
    )
    .await;
    assert!(
        matches!(settled, ViolationSettlement::Denied { .. }),
        "{settled:?}"
    );
    let live = fx.sandbox.live_grants().await;
    let [denial] = live.as_slice() else {
        panic!("one live grant, got {live:?}");
    };
    assert_eq!(GrantDecision::Deny, denial.decision);
    assert_eq!(
        GrantScope::Workspace {
            root: fx.root.clone()
        },
        denial.scope
    );
    assert_eq!(
        GrantSubject::FsWriteRoot {
            root: canonical_path(&fx.outside())
        },
        denial.subject
    );
    let asked = StubTransport::replying(allow_reply("workspace", None));
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.fs_write_violation(),
        dyn_transport(&asked),
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(
        model_text.contains("chose \"Always reject\""),
        "{model_text}"
    );
    assert!(asked.prompts().is_empty());
    for (scope, root) in [
        (GrantScope::Call, fx.outside()),
        (
            GrantScope::Workspace {
                root: fx.root.clone(),
            },
            fx.tmp.path().to_path_buf(),
        ),
    ] {
        let allow = Grant {
            subject: GrantSubject::FsWriteRoot { root },
            scope,
            decision: GrantDecision::Allow,
            ..denial.clone()
        };
        let refused = fx
            .sandbox
            .record_grant(&fx.call, None, "sess-1", allow)
            .await;
        assert!(
            matches!(refused, Err(WorkspaceSandboxError::DeniedByRow)),
            "{refused:?}"
        );
    }
    assert_eq!(live, fx.sandbox.live_grants().await);
}
/// The store keeps a deny row in its canonical spelling; a proposal spelled through a link to
/// the denied folder is still answered by the row and cannot be allowed past it.
#[cfg(unix)]
#[tokio::test]
async fn always_reject_covers_a_proposal_spelled_through_a_link() {
    let fx = Fixture::new().await;
    let transport = StubTransport::replying(json!({
        "outcome": "always_reject",
        "tool_call_id": "call-1",
    }));
    settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.fs_write_violation(),
        dyn_transport(&transport),
    )
    .await;
    let live = fx.sandbox.live_grants().await;
    let [denial] = live.as_slice() else {
        panic!("one live grant, got {live:?}");
    };
    assert_eq!(
        GrantSubject::FsWriteRoot {
            root: canonical_path(&fx.outside())
        },
        denial.subject
    );
    let alias = fx.tmp.path().join("caches-link");
    std::os::unix::fs::symlink(fx.outside(), &alias).unwrap();
    let violation = Violation {
        blocked: Blocked::FsWrite {
            path: alias.join("index"),
        },
        proposed: Some(GrantSubject::FsWriteRoot {
            root: alias.clone(),
        }),
        ..fx.fs_write_violation()
    };
    let asked = StubTransport::replying(allow_reply("workspace", None));
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        violation,
        dyn_transport(&asked),
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(
        model_text.contains("chose \"Always reject\""),
        "{model_text}"
    );
    assert!(asked.prompts().is_empty());
    for scope in [
        GrantScope::Call,
        GrantScope::Workspace {
            root: fx.root.clone(),
        },
    ] {
        let allow = Grant {
            subject: GrantSubject::FsWriteRoot {
                root: alias.clone(),
            },
            scope,
            decision: GrantDecision::Allow,
            ..denial.clone()
        };
        let refused = fx
            .sandbox
            .record_grant(&fx.call, None, "sess-1", allow)
            .await;
        assert!(
            matches!(refused, Err(WorkspaceSandboxError::DeniedByRow)),
            "{refused:?}"
        );
    }
    assert_eq!(live, fx.sandbox.live_grants().await);
}
/// A filesystem denial of one kind: its `Blocked` for a path and its grant subject for a root.
#[cfg(unix)]
type FsKind = (fn(PathBuf) -> Blocked, fn(PathBuf) -> GrantSubject);
/// `propose` offers nothing for a folder spelled through a symlink, so that violation carries no
/// proposal; the row "Always reject" stored for the folder the link leads to still answers it,
/// for a write and a read alike: no card, the row's refusal, and nothing recorded.
#[cfg(unix)]
#[tokio::test]
async fn always_reject_answers_a_linked_folder_that_proposes_nothing() {
    let kinds: [FsKind; 2] = [
        (
            |path| Blocked::FsWrite { path },
            |root| GrantSubject::FsWriteRoot { root },
        ),
        (
            |path| Blocked::FsRead { path },
            |root| GrantSubject::FsRead { root },
        ),
    ];
    for (blocked, subject) in kinds {
        let fx = Fixture::new().await;
        let rejecting = StubTransport::replying(json!({
            "outcome": "always_reject",
            "tool_call_id": "call-1",
        }));
        let direct = Violation {
            blocked: blocked(fx.outside().join("index")),
            proposed: Some(subject(fx.outside())),
            ..fx.fs_write_violation()
        };
        settle_violation(
            fx.ctx(ToolApprovalPolicy::GrantsAllowed),
            direct,
            dyn_transport(&rejecting),
        )
        .await;
        let live = fx.sandbox.live_grants().await;
        let [denial] = live.as_slice() else {
            panic!("one live grant, got {live:?}");
        };
        assert_eq!(GrantDecision::Deny, denial.decision);
        assert_eq!(subject(canonical_path(&fx.outside())), denial.subject);
        let alias = fx.tmp.path().join("caches-link");
        std::os::unix::fs::symlink(fx.outside(), &alias).unwrap();
        let through_link = Violation {
            blocked: blocked(alias.join("index")),
            proposed: None,
            disposition: Disposition::informational(InformationalReason::ProtectedTarget),
            ..fx.fs_write_violation()
        };
        let asked =
            StubTransport::replying(json!({ "outcome": "reject", "tool_call_id": "call-1" }));
        let settled = settle_violation(
            fx.ctx(ToolApprovalPolicy::GrantsAllowed),
            through_link,
            dyn_transport(&asked),
        )
        .await;
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        assert!(asked.prompts().is_empty(), "{:?}", asked.prompts());
        let ViolationSettlement::Denied { model_text } = settled else {
            panic!("{settled:?}");
        };
        assert!(
            model_text.contains("chose \"Always reject\""),
            "{model_text}"
        );
        assert_eq!(live, fx.sandbox.live_grants().await);
    }
}
/// A session owner who never answers; `waiting` is how many cards it is sitting on right now.
#[derive(Default)]
struct SilentTransport {
    waiting: AtomicUsize,
}
struct Waiting<'a>(&'a AtomicUsize);
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[async_trait]
impl PermissionHookTransport for SilentTransport {
    async fn request_permission(&self, _payload: Value) -> Result<Value, String> {
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let _waiting = Waiting(&self.waiting);
        std::future::pending().await
    }
}
/// Waits (bounded) until `waiting` reads `want`.
async fn waiting_reaches(transport: &SilentTransport, want: usize) -> bool {
    tokio::time::timeout(Duration::from_secs(5), async {
        while transport.waiting.load(Ordering::SeqCst) != want {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok()
}
/// An informational card nobody acknowledges is a task the sandbox owns: it ends with the
/// sandbox instead of waiting on the hub forever.
#[tokio::test]
async fn an_unacknowledged_informational_card_ends_with_the_sandbox() {
    let fx = Fixture::new().await;
    let silent = Arc::new(SilentTransport::default());
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.capability_violation(),
        Some(silent.clone() as Arc<dyn PermissionHookTransport>),
    )
    .await;
    assert!(
        matches!(settled, ViolationSettlement::Denied { .. }),
        "{settled:?}"
    );
    assert!(waiting_reaches(&silent, 1).await, "the card was posted");
    drop(fx);
    assert!(
        waiting_reaches(&silent, 0).await,
        "the acknowledgement ended with the sandbox"
    );
}
#[tokio::test]
async fn a_capability_card_is_informational_and_settled_before_it_is_answered() {
    let fx = Fixture::new().await;
    let transport = StubTransport::replying(allow_reply("global", None));
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::UnattendedAllowed),
        fx.capability_violation(),
        dyn_transport(&transport),
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(
        model_text.contains("can never be allowed from here; the user has been told"),
        "{model_text}"
    );
    assert!(fx.sandbox.live_grants().await.is_empty());
    let card = transport.wait_for_prompt().await;
    assert_eq!(Some(&json!([])), card.get("offered_scopes"));
    assert_eq!(
        Some(&json!("informational")),
        card.pointer("/disposition/kind")
    );
    assert!(card.get("deadline_unix").is_none(), "{card}");
}
/// A protected target reads "protected" to the model, and the card is informational too.
#[tokio::test]
async fn a_protected_target_is_denied_at_once_with_its_own_text() {
    let fx = Fixture::new().await;
    let transport =
        StubTransport::replying(json!({ "outcome": "reject", "tool_call_id": "call-1" }));
    let violation = Violation {
        disposition: Disposition::informational(InformationalReason::ProtectedTarget),
        proposed: None,
        ..fx.fs_write_violation()
    };
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        violation,
        dyn_transport(&transport),
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(
        model_text
            .contains("This target is protected and can never be allowed; the user has been told."),
        "{model_text}"
    );
    let card = transport.wait_for_prompt().await;
    assert_eq!(
        Some(&json!("informational")),
        card.pointer("/disposition/kind")
    );
}
/// A path the user's `sandbox.toml` denies reads as that deny to the model, not as a protected
/// target or a generic refusal, and the card is informational.
#[tokio::test]
async fn a_profile_deny_is_denied_at_once_with_its_own_text() {
    let fx = Fixture::new().await;
    let transport =
        StubTransport::replying(json!({ "outcome": "reject", "tool_call_id": "call-1" }));
    let violation = Violation {
        disposition: Disposition::informational(InformationalReason::ProfileDeny),
        proposed: None,
        ..fx.fs_write_violation()
    };
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        violation,
        dyn_transport(&transport),
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(
        model_text.contains("The user's sandbox.toml denies this path, so no grant can open it"),
        "{model_text}"
    );
    let card = transport.wait_for_prompt().await;
    assert_eq!(
        Some(&json!("informational")),
        card.pointer("/disposition/kind")
    );
    assert_eq!(
        Some(&json!("profile_deny")),
        card.pointer("/disposition/reason")
    );
}
/// A blocked path that is not UTF-8 cannot be named on the card: the denial is settled at once
/// with text that says so, the card is informational and shows the path lossily, and nothing is
/// granted whatever the renderer answers.
#[cfg(unix)]
#[tokio::test]
async fn a_path_that_is_not_utf8_is_denied_at_once_with_an_informational_card() {
    let fx = Fixture::new().await;
    let transport = StubTransport::replying(allow_reply("global", None));
    let name: &std::ffi::OsStr = std::os::unix::ffi::OsStrExt::from_bytes(b"cache-\xff");
    let root = fx.outside().join(name);
    let violation = Violation {
        blocked: Blocked::FsWrite {
            path: root.join("index"),
        },
        proposed: Some(GrantSubject::FsWriteRoot { root }),
        ..fx.fs_write_violation()
    };
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::UnattendedAllowed),
        violation,
        dyn_transport(&transport),
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(
        model_text.contains("The denied path is not valid UTF-8"),
        "{model_text}"
    );
    assert!(fx.sandbox.live_grants().await.is_empty());
    let card = transport.wait_for_prompt().await;
    assert_eq!(
        Some(&json!({ "kind": "informational", "reason": "decode_failed" })),
        card.get("disposition")
    );
    assert_eq!(Some(&json!([])), card.get("offered_scopes"));
    assert!(card.get("proposed_grant").is_none(), "{card}");
    assert!(card.get("deadline_unix").is_none(), "{card}");
    let shown = card
        .pointer("/blocked/path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(shown.ends_with("/cache-\u{fffd}/index"), "{card}");
    assert!(fx.sandbox.live_grants().await.is_empty());
}
/// A network denial the post-run
/// decoder read is informational (`unproxied_network`) — deny-only, and the model is told the
/// posture and the way out, not offered a per-host grant. With no proxy bound the posture is
/// "network off"; with one bound, "this tool did not use the proxy".
#[tokio::test]
async fn an_unproxied_network_violation_is_informational_with_the_recovery_text() {
    let fx = Fixture::new().await;
    let transport = StubTransport::replying(allow_reply("session", None));
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.unproxied_net_violation(),
        dyn_transport(&transport),
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(
        model_text.contains("Network is off for this workspace (the local proxy is not running)"),
        "{model_text}"
    );
    assert!(
        model_text.contains(super::text::RECOVERY_LINE),
        "the way out is named: {model_text}"
    );
    assert!(
        fx.sandbox.live_grants().await.is_empty(),
        "an informational card's answer never becomes a grant"
    );
    let card = transport.wait_for_prompt().await;
    assert_eq!(Some(&json!([])), card.get("offered_scopes"));
    assert_eq!(
        Some(&json!({ "kind": "informational", "reason": "unproxied_network" })),
        card.get("disposition")
    );
    let fx = Fixture::with_proxy(true).await;
    let transport = StubTransport::replying(allow_reply("session", None));
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.unproxied_net_violation(),
        dyn_transport(&transport),
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(
        model_text.contains("This tool did not use the workspace proxy")
            && model_text.contains(super::text::RECOVERY_LINE),
        "{model_text}"
    );
    assert!(fx.sandbox.live_grants().await.is_empty());
}
#[tokio::test]
async fn transport_failure_and_cancellation_keep_the_denial() {
    let fx = Fixture::new().await;
    let failing = StubTransport::failing("hub gone");
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.fs_write_violation(),
        dyn_transport(&failing),
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(model_text.contains("hub gone"), "{model_text}");
    let cancelled = StubTransport::replying(json!({ "outcome": "cancelled" }));
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.fs_write_violation(),
        dyn_transport(&cancelled),
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(model_text.contains("cancelled"), "{model_text}");
    assert!(fx.sandbox.live_grants().await.is_empty());
}
/// A held connection's allow is a real grant: recorded for the session, and the hold resumes
/// (post-run decode never derives a held connection; that path is the proxy's decider).
/// The card offers the host itself; the answer keeps the violation's port.
#[tokio::test]
async fn a_held_connections_allow_records_a_session_grant_and_resumes() {
    let fx = Fixture::with_proxy(true).await;
    let transport = StubTransport::replying(allow_reply("session", None));
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.held_net_violation(),
        dyn_transport(&transport),
    )
    .await;
    let ViolationSettlement::Replay { grant } = settled else {
        panic!("{settled:?}");
    };
    assert_eq!(GrantScope::Session, grant.scope);
    assert_eq!(
        GrantSubject::NetHost {
            host: HostPattern::new("registry.npmjs.org"),
            port: None,
        },
        grant.subject
    );
    let live = fx.sandbox.live_grants().await;
    assert_eq!(1, live.len(), "session rows live in the store's memory");
    let card = transport.wait_for_prompt().await;
    assert_eq!(
        Some(&json!(["call", "session", "workspace"])),
        card.get("offered_scopes")
    );
    assert_eq!(Some(&json!("resume")), card.get("replay"));
    assert_eq!(
        Some(&json!({ "kind": "grantable" })),
        card.get("disposition")
    );
}
/// "Keep blocked" on a held connection records a call-scoped deny row for
/// the host, so the call's later connections are refused without a card; the row is the call's
/// alone (not in the store, gone with the call) and "Always reject" still records the workspace
/// row.
#[tokio::test]
async fn keep_blocked_on_a_held_connection_records_a_call_scoped_deny_row() {
    let fx = Fixture::with_proxy(true).await;
    fx.sandbox
        .bind_call(
            &fx.call,
            CallOwner {
                session_id: "sess-1".to_owned(),
                policy: ToolApprovalPolicy::GrantsAllowed,
                transport: None,
                command: None,
            },
        )
        .unwrap();
    let transport =
        StubTransport::replying(json!({ "outcome": "reject", "tool_call_id": "call-1" }));
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.held_net_violation(),
        dyn_transport(&transport),
    )
    .await;
    assert!(
        matches!(settled, ViolationSettlement::Denied { .. }),
        "{settled:?}"
    );
    assert!(
        fx.sandbox.live_grants().await.is_empty(),
        "nothing persisted"
    );
    let call = xai_grok_sandbox::command::CommandTag::for_call(&CallId::tool("call-1"));
    let rows = fx.sandbox.net_rows_for(Some(&call));
    assert_eq!(1, rows.len(), "{rows:?}");
    let row = rows.first().expect("one call row");
    assert_eq!(
        (GrantScope::Call, GrantDecision::Deny, Expiry::Never),
        (row.scope.clone(), row.decision, row.expires)
    );
    assert_eq!(
        GrantSubject::NetHost {
            host: HostPattern::new("registry.npmjs.org"),
            port: None,
        },
        row.subject
    );
    assert!(
        fx.sandbox
            .net_rows_for(Some(&xai_grok_sandbox::command::CommandTag::for_call(
                &CallId::tool("call-2")
            )))
            .is_empty(),
        "another call's connections still ask"
    );
    let transport =
        StubTransport::replying(json!({ "outcome": "reject", "tool_call_id": "call-1" }));
    let _ = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.fs_write_violation(),
        dyn_transport(&transport),
    )
    .await;
    assert_eq!(1, fx.sandbox.net_rows_for(Some(&call)).len());
    assert!(fx.sandbox.live_grants().await.is_empty());
}
/// The card carries the gate's own deadline (the hub backstop), from the
/// sandbox's clock.
#[tokio::test]
async fn a_grantable_card_carries_the_hub_backstop_deadline() {
    let fx = Fixture::new().await;
    let transport = StubTransport::replying(allow_reply("call", None));
    let _ = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.fs_write_violation(),
        dyn_transport(&transport),
    )
    .await;
    let card = transport.wait_for_prompt().await;
    let deadline = card
        .get("deadline_unix")
        .and_then(Value::as_i64)
        .expect("deadline");
    let backstop = i64::try_from(super::CARD_DEADLINE.as_secs()).unwrap();
    assert_eq!(NOW + backstop, deadline);
    assert_eq!(Some(&json!("grantable")), card.pointer("/disposition/kind"));
}
/// An "allow once" answered after the call's result came in has no spawn to carry it: it is not
/// reported as given, the denial stands, and a held connection is refused rather than resumed.
/// Nothing of it re-creates the call: a settlement opened for the released id is refused.
#[tokio::test]
async fn a_call_grant_answered_after_the_call_finished_keeps_the_denial() {
    let fx = Fixture::new().await;
    let cases = [
        (
            "a stopped command",
            fx.fs_write_violation(),
            "fs_write_root",
        ),
        ("a held connection", fx.held_net_violation(), "net_host"),
    ];
    for (case, violation, kind) in cases {
        fx.sandbox.release_call(&fx.call);
        let mut reply = allow_reply("call", None);
        if let Some(map) = reply.as_object_mut() {
            map.insert("scope".to_owned(), json!({ "kind": kind }));
        }
        let transport = StubTransport::replying(reply);
        let settled = settle_violation(
            fx.ctx(ToolApprovalPolicy::GrantsAllowed),
            violation,
            dyn_transport(&transport),
        )
        .await;
        let ViolationSettlement::Denied { model_text } = settled else {
            panic!("{case}: {settled:?}");
        };
        assert!(
            model_text.contains("the command it was for has already finished"),
            "{case}: {model_text}"
        );
        assert!(fx.sandbox.live_grants().await.is_empty(), "{case}");
        assert!(
            fx.sandbox.open_settlement(&fx.call).is_err(),
            "{case}: a settlement for a finished call re-creates nothing"
        );
        assert_eq!(0, fx.sandbox.open_calls(), "{case}");
    }
}
/// An allow that names a folder the violation does not tie to — one that does not hold the
/// blocked path — grants nothing: the denial stands, the model is told the grant was not
/// recorded, the violation counts as denied, and the card was posted once.
#[tokio::test]
async fn an_allow_for_a_folder_untied_to_the_violation_keeps_the_denial() {
    let fx = Fixture::new().await;
    let elsewhere = fx.tmp.path().join("elsewhere").join("store");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let mut reply = allow_reply("workspace", None);
    if let Some(map) = reply.as_object_mut() {
        map.insert(
            "scope".to_owned(),
            json!({ "kind": "fs_write_root", "value": elsewhere.to_str().unwrap() }),
        );
    }
    let before = metrics::violation_total(
        SandboxMode::Enforce,
        SandboxBlockedKind::FsWrite,
        SandboxSettlement::Denied,
    );
    let transport = StubTransport::replying(reply);
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        fx.fs_write_violation(),
        dyn_transport(&transport),
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(
        model_text.contains("the grant could not be recorded")
            && model_text.contains("does not contain the blocked path"),
        "{model_text}"
    );
    assert!(fx.sandbox.live_grants().await.is_empty());
    assert_eq!(1, transport.prompts().len(), "the card was posted once");
    assert!(
        metrics::violation_total(
            SandboxMode::Enforce,
            SandboxBlockedKind::FsWrite,
            SandboxSettlement::Denied,
        ) > before,
        "counted as a denial"
    );
}
/// What the hub makes of a call, from the tool's kind and namespace and the folder's mode: only a
/// shell whose output the decoder reads, on a folder that enforces, has the sandbox card for its
/// prompt; any other shell keeps the pre-run prompt and still follows the result path; a tool
/// that is no shell never meets the sandbox.
#[test]
fn the_sandbox_path_follows_the_tools_kind_and_namespace_and_the_folders_mode() {
    let execute = |namespace| Some((ToolKind::Execute, namespace));
    let decodable = [
        ToolNamespace::GrokBuild,
        ToolNamespace::GrokBuildConcise,
        ToolNamespace::OpenCode,
    ];
    for namespace in decodable {
        assert_eq!(
            SandboxPath::Gated,
            SandboxPath::for_tool(execute(namespace), Some(SandboxMode::Enforce)),
            "{namespace:?}"
        );
        for mode in [Some(SandboxMode::Observe), Some(SandboxMode::Off), None] {
            assert_eq!(
                SandboxPath::Shell,
                SandboxPath::for_tool(execute(namespace), mode),
                "{namespace:?} {mode:?}: no card where nothing enforces"
            );
        }
    }
    for namespace in [
        ToolNamespace::MCP,
        ToolNamespace::Codex,
        ToolNamespace::GrokBuildHashline,
    ] {
        assert_eq!(
            SandboxPath::Shell,
            SandboxPath::for_tool(execute(namespace), Some(SandboxMode::Enforce)),
            "{namespace:?}: a shell whose output is not decoded keeps the pre-run prompt"
        );
    }
    for kind in [ToolKind::Read, ToolKind::Edit, ToolKind::Search] {
        assert_eq!(
            SandboxPath::None,
            SandboxPath::for_tool(
                Some((kind, ToolNamespace::GrokBuild)),
                Some(SandboxMode::Enforce)
            ),
            "{kind:?}"
        );
    }
    assert_eq!(
        SandboxPath::None,
        SandboxPath::for_tool(None, Some(SandboxMode::Enforce)),
        "a tool the toolset does not know"
    );
    assert_eq!(PromptGate::SandboxCard, SandboxPath::Gated.prompt_gate());
    assert_eq!(PromptGate::PreRun, SandboxPath::Shell.prompt_gate());
    assert_eq!(PromptGate::PreRun, SandboxPath::None.prompt_gate());
    assert!(SandboxPath::Gated.follows_result_path());
    assert!(SandboxPath::Shell.follows_result_path());
    assert!(!SandboxPath::None.follows_result_path());
}
/// A session owner who answers each card only once the test releases its call.
struct HeldTransport {
    seen: parking_lot::Mutex<Vec<Value>>,
    released: tokio::sync::watch::Receiver<Vec<String>>,
}
impl HeldTransport {
    fn new() -> (Arc<HeldTransport>, tokio::sync::watch::Sender<Vec<String>>) {
        let (tx, released) = tokio::sync::watch::channel(Vec::new());
        let transport = Arc::new(HeldTransport {
            seen: parking_lot::Mutex::new(Vec::new()),
            released,
        });
        (transport, tx)
    }
    fn posted(&self) -> Vec<String> {
        self.seen
            .lock()
            .iter()
            .map(|card| {
                card.get("tool_call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }
    /// Yields (bounded) until `count` cards were posted.
    async fn posted_reaches(&self, count: usize) -> bool {
        for _ in 0..1000 {
            if self.seen.lock().len() >= count {
                return true;
            }
            tokio::task::yield_now().await;
        }
        false
    }
}
#[async_trait]
impl PermissionHookTransport for HeldTransport {
    async fn request_permission(&self, payload: Value) -> Result<Value, String> {
        let call = payload
            .get("tool_call_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        self.seen.lock().push(payload);
        let mut released = self.released.clone();
        released
            .wait_for(|released| released.contains(&call))
            .await
            .map_err(|e| e.to_string())?;
        Ok(allow_reply("call", None))
    }
}
/// One grantable card per session at a time: a second violation in the session posts its card
/// only once the first is answered, while another session's card is not held back.
#[tokio::test]
async fn a_sessions_second_grantable_card_waits_for_the_firsts_answer() {
    let fx = Fixture::new().await;
    let (transport, release) = HeldTransport::new();
    let (call_2, call_3) = (CallId::tool("call-2"), CallId::tool("call-3"));
    let settle = |ctx| {
        settle_violation(
            ctx,
            fx.fs_write_violation(),
            Some(transport.clone() as Arc<dyn PermissionHookTransport>),
        )
    };
    let first = settle(fx.ctx_for(&fx.call, "sess-1"));
    let second = settle(fx.ctx_for(&call_2, "sess-1"));
    let other_session = settle(fx.ctx_for(&call_3, "sess-2"));
    let observer = async {
        assert!(
            transport.posted_reaches(2).await,
            "the first card of each session is posted"
        );
        assert_eq!(vec!["call-1", "call-3"], transport.posted());
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            2,
            transport.posted().len(),
            "the session's second card waits for the first's answer"
        );
        release.send_modify(|released| released.push("call-1".to_owned()));
        assert!(
            transport.posted_reaches(3).await,
            "the first's answer lets the second card through"
        );
        assert_eq!(vec!["call-1", "call-3", "call-2"], transport.posted());
        release.send_modify(|released| {
            released.push("call-2".to_owned());
            released.push("call-3".to_owned());
        });
    };
    let (first, second, other_session, ()) = tokio::join!(first, second, other_session, observer);
    for (call, settled) in [
        ("call-1", first),
        ("call-2", second),
        ("call-3", other_session),
    ] {
        assert!(
            matches!(settled, ViolationSettlement::Replay { .. }),
            "{call}: {settled:?}"
        );
    }
}
#[tokio::test]
async fn a_grant_the_store_refuses_keeps_the_denial() {
    let fx = Fixture::new().await;
    let protected = fx.root.join(".git").join("hooks");
    let violation = Violation {
        blocked: Blocked::FsWrite {
            path: protected.join("pre-commit"),
        },
        proposed: Some(GrantSubject::FsWriteRoot { root: protected }),
        disposition: Disposition::Grantable,
        partial_output: None,
        replay: Replay::Rerun,
        exit_code: Some(1),
        stderr_snippet: String::new(),
    };
    let transport = StubTransport::replying(allow_reply("workspace", None));
    let settled = settle_violation(
        fx.ctx(ToolApprovalPolicy::GrantsAllowed),
        violation,
        dyn_transport(&transport),
    )
    .await;
    let ViolationSettlement::Denied { model_text } = settled else {
        panic!("{settled:?}");
    };
    assert!(model_text.contains("protected"), "{model_text}");
    assert!(fx.sandbox.live_grants().await.is_empty());
}
