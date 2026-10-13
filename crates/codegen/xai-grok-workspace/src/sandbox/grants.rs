//! The grant side of a [`WorkspaceSandbox`]: the live-row snapshot the policy and the proxy's
//! decider read, recording what the user allowed, listing and revoking rows, and the JSON the
//! daemon's `sandbox.*` control verbs answer with. The store on disk is
//! `xai_grok_sandbox::command::GrantStore`; this file is the daemon's view of it.

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};
use xai_grok_sandbox::command::backend::CommandTag;
use xai_grok_sandbox::command::grants::{
    Expiry, Grant, GrantDecision, GrantScope, GrantSubject, SystemClock,
};
use xai_grok_sandbox::command::{
    CallId, GrantError, GrantStore, ObserveSummary, allows_not_denied, canonical_subject,
};

use super::{Engaged, GrantId, LiveRows, WorkspaceSandbox, WorkspaceSandboxError, metrics};

impl Engaged {
    /// The one writer of the live-row snapshot; called under the store lock after every change
    /// and reload.
    fn refresh_live(&self, store: &GrantStore) {
        *self.live.write() = LiveRows {
            shared: store.live_shared(),
            sessions: store.live_session_rows(),
        };
    }
}

impl WorkspaceSandbox {
    /// The live rows of the snapshot a call run for `session` sees; none before the folder
    /// engaged.
    fn rows_for(&self, session: Option<&str>) -> Vec<Grant> {
        self.engaged
            .get()
            .map(|engaged| engaged.live.read().rows_for(session))
            .unwrap_or_default()
    }

    /// The live rows a call for hub session `session` runs under (the shared rows and that
    /// session's own), re-judged against the clock: the allows no live deny covers, plus every
    /// live deny (one that cuts no allow is ignored by the policy build).
    pub(super) fn live_allows_now(&self, session: Option<&str>) -> Vec<Grant> {
        let now = self.clock.now_unix();
        let live: Vec<Grant> = self
            .rows_for(session)
            .into_iter()
            .filter(|grant| grant.is_live(now))
            .collect();
        let mut rows = allows_not_denied(&live);
        rows.retain(|grant| grant.decision == GrantDecision::Allow);
        rows.extend(
            live.into_iter()
                .filter(|grant| grant.decision == GrantDecision::Deny),
        );
        rows
    }

    /// Whether a live deny row a call for hub session `session` runs under covers `subject`, so
    /// that [`allows_not_denied`] would drop an allow for it: deny rows win. Rows are stored
    /// canonical, so `subject` is judged as typed and as a canonical copy; either match counts.
    pub(crate) fn deny_row_covers(&self, session: Option<&str>, subject: &GrantSubject) -> bool {
        let mut denies = self.live_allows_now(session);
        denies.retain(|grant| grant.decision == GrantDecision::Deny);
        [subject.clone(), canonical_subject(subject.clone())]
            .into_iter()
            .any(|candidate| {
                let mut rows = denies.clone();
                rows.push(Grant {
                    id: GrantId::new(String::new()),
                    subject: candidate,
                    scope: GrantScope::Call,
                    expires: Expiry::Never,
                    decision: GrantDecision::Allow,
                    granted_at: 0,
                    granted_by: String::new(),
                    via: None,
                });
                !allows_not_denied(&rows)
                    .iter()
                    .any(|grant| grant.decision == GrantDecision::Allow)
            })
    }

    /// The live `NetHost` rows, allow and deny alike, a call run for `session` is decided under:
    /// the shared rows and that session's own, never another session's.
    pub fn net_rows(&self, session: Option<&str>) -> Vec<Grant> {
        let now = self.clock.now_unix();
        self.rows_for(session)
            .into_iter()
            .filter(|grant| {
                grant.is_live(now) && matches!(grant.subject, GrantSubject::NetHost { .. })
            })
            .collect()
    }

    /// [`WorkspaceSandbox::net_rows`] for the session `call` runs for, plus the call-scoped
    /// `NetHost` rows in effect for it: inherited by its running spawn or given while it runs (a
    /// connection allowed "once" covers the command's next ones too). The `GrantView` the decider
    /// reads.
    pub fn net_rows_for(&self, call: Option<&CommandTag>) -> Vec<Grant> {
        let Some(call) = call.and_then(CommandTag::call_id) else {
            return self.net_rows(None);
        };
        let (session, once) = {
            let calls = self.calls.lock();
            (calls.session_of(&call), calls.once_rows(&call))
        };
        let mut rows = self.net_rows(session.as_deref());
        rows.extend(
            once.into_iter()
                .filter(|grant| matches!(grant.subject, GrantSubject::NetHost { .. })),
        );
        rows
    }

    /// Record the grant the user gave on the card for `call_id`, answered from hub session
    /// `session_id`. Call-scoped grants apply to the connections of the spawn of `call_id` that
    /// is running and to its next spawn, and go with the call's result or, after a background
    /// start, its child's exit (a late one is refused: the call is gone, or `epoch` — the call's
    /// as the card was raised — moved on since); session grants live with `session_id`; the
    /// rest go to their files.
    ///
    /// # Errors
    /// The store's error for a protected subject or an unwritable file; the grant is then not in
    /// effect. [`WorkspaceSandboxError::DeniedByRow`] for an allow a live deny row covers.
    pub async fn record_grant(
        &self,
        call: &CallId,
        epoch: Option<u64>,
        session_id: &str,
        grant: Grant,
    ) -> Result<GrantId, WorkspaceSandboxError> {
        if let GrantSubject::FsWriteRoot { root } | GrantSubject::FsRead { root } = &grant.subject
            && root.to_str().is_none()
        {
            return Err(WorkspaceSandboxError::SubjectNotUtf8 {
                path: root.to_string_lossy().into_owned(),
            });
        }
        let engaged = self.engage().await;
        let mut store = engaged.store.lock().await;
        if grant.decision == GrantDecision::Allow
            && self.deny_row_covers(Some(session_id), &grant.subject)
        {
            return Err(WorkspaceSandboxError::DeniedByRow);
        }
        let recorded = grant.clone();
        let id = match grant.scope {
            GrantScope::Call => {
                // SECURITY: the floor applies to a one-shot grant too; it never enters the store,
                // so the store's own check in `add` does not run for it
                store.check_subject(&grant.subject)?;
                if !self.calls.lock().stash_once(call, epoch, grant) {
                    return Err(WorkspaceSandboxError::CallFinished);
                }
                recorded.id.clone()
            }
            GrantScope::Session => store.add_session(session_id, grant)?,
            GrantScope::Workspace { .. } | GrantScope::Global => store.add(grant).await?,
        };
        engaged.refresh_live(&store);
        metrics::grant(&recorded);
        xai_grok_telemetry::log_event(metrics::grant_recorded_event(&recorded));
        Ok(id)
    }

    /// A hub session ended: its "for this conversation" rows are gone, and so is what the table
    /// kept for the calls it left running in the background, their credentials revoked and
    /// their holds refused with it.
    pub async fn end_session(&self, session_id: &str) {
        let (ended, enforced) = {
            let mut calls = self.calls.lock();
            let ended = calls.end_session(session_id);
            self.revoke_released(&ended);
            (ended, calls.any_runs_enforced())
        };
        self.release_ended_holds(ended);
        self.resync_unless_enforced(enforced);
        let Some(engaged) = self.engaged.get() else {
            return;
        };
        let mut store = engaged.store.lock().await;
        let dropped = store.clear_session(session_id);
        if dropped > 0 {
            tracing::info!(session_id, dropped, "session sandbox grants dropped");
            engaged.refresh_live(&store);
        }
    }

    /// Every live row, session ∪ workspace ∪ global, after picking up file changes made elsewhere
    /// (a hold a row now covers is released with its verdict).
    pub async fn live_grants(&self) -> Vec<Grant> {
        let engaged = self.engage().await;
        let (live, reloaded) = {
            let mut store = engaged.store.lock().await;
            let reloaded = store.reload_if_changed().await;
            if reloaded {
                engaged.refresh_live(&store);
            }
            (store.live(), reloaded)
        };
        if reloaded {
            self.release_holds_now_covered().await;
        }
        live
    }

    /// # Errors
    /// `NotFound` for an unknown id, or the persist error.
    pub async fn revoke_grant(&self, id: &GrantId) -> Result<(), WorkspaceSandboxError> {
        let engaged = self.engage().await;
        let mut store = engaged.store.lock().await;
        revoke_in(&mut store, id).await?;
        engaged.refresh_live(&store);
        Ok(())
    }

    /// Re-read the grant files when another writer (the desktop's revoke, another daemon) changed
    /// them, so the next `prepare` sees the change and a hold a row now covers is released with
    /// its verdict. Nothing before the folder engaged.
    pub async fn refresh_grants(&self) {
        let Some(engaged) = self.engaged.get() else {
            return;
        };
        let reloaded = {
            let mut store = engaged.store.lock().await;
            let reloaded = store.reload_if_changed().await;
            if reloaded {
                engaged.refresh_live(&store);
            }
            reloaded
        };
        if reloaded {
            self.release_holds_now_covered().await;
        }
    }

    /// `sandbox.status`: what the desktop shows in Settings. `backend` is the `BackendName`
    /// spelling or `"none"` (the desktop decodes a string); `network` is `"proxy"` while an
    /// egress proxy is bound for the folder, `"off"` otherwise, and `proxy` is where it listens
    /// or `null`; `degraded` is the mode's `ModeDegradation` marker, or null.
    pub fn status_json(&self) -> Value {
        let resolved = self.resolved_mode();
        let proxy = self.proxy();
        json!({
            "mode": resolved.mode,
            "mode_source": resolved.source,
            "degraded": resolved.degraded,
            "backend": self.backend_name().map_or("none", <&str>::from),
            "reduced_sandbox": self.reduced_sandbox(),
            "network": if proxy.is_some() { "proxy" } else { "off" },
            "proxy": proxy.map(|proxy| json!({ "port": proxy.port })),
            "open_calls": self.open_calls(),
            "session_dir_unsafe": self.session_dir_unsafe(),
        })
    }

    /// `sandbox.grants.list`: rows plus `expires_at` (unix seconds or null).
    pub async fn grants_json(&self) -> Value {
        json!({ "grants": grants_to_json(&self.live_grants().await) })
    }

    /// `sandbox.observe.summary`: the would-block table, most recent first.
    pub fn observe_summary_json(&self) -> Value {
        observe_summary_to_json(&self.observe_summary())
    }
}

/// The live rows of `<grok_home>/sandbox_grants.toml` alone (`sandbox.grants.list` with no
/// folder), read fresh. Session and workspace rows belong to a served folder's sandbox.
pub async fn global_grants(grok_home: &Path) -> Vec<Grant> {
    let store = GrantStore::open_in(
        grok_home,
        grok_home,
        Vec::new(),
        xai_dirs::home_dir().as_deref(),
        Arc::new(SystemClock),
    )
    .await;
    store
        .live()
        .into_iter()
        .filter(|grant| grant.scope == GrantScope::Global)
        .collect()
}

/// Revoke a global row through `<grok_home>/sandbox_grants.toml` directly, for a
/// `sandbox.grants.revoke` that arrives while no folder is served (a served folder's sandbox
/// revokes global rows itself, and the file's mtime tells every other store to reload).
///
/// # Errors
/// `GrantError::NotFound` for an unknown id, or the persist error.
pub async fn revoke_global_grant(grok_home: &Path, id: &GrantId) -> Result<(), GrantError> {
    let mut store = GrantStore::open_in(
        grok_home,
        grok_home,
        Vec::new(),
        xai_dirs::home_dir().as_deref(),
        Arc::new(SystemClock),
    )
    .await;
    revoke_in(&mut store, id).await
}

/// Revoke `id` in `store` and report the scope of the row it was.
async fn revoke_in(store: &mut GrantStore, id: &GrantId) -> Result<(), GrantError> {
    let scope = store
        .live()
        .into_iter()
        .find(|g| &g.id == id)
        .map(|g| g.scope);
    store.revoke(id).await?;
    if let Some(scope) = scope {
        xai_grok_telemetry::log_event(metrics::grant_revoked_event(&scope));
    }
    Ok(())
}

/// The JSON rows with `expires_at` for the Settings list. A row that does not serialize is left
/// out and logged, never sent as `null`, which the list's decoder would reject whole.
pub fn grants_to_json(grants: &[Grant]) -> Vec<Value> {
    grants
        .iter()
        .filter_map(|grant| {
            let mut row = serde_json::to_value(grant)
                .inspect_err(|error| {
                    tracing::warn!(id = %grant.id, %error, "sandbox grant row left out of the list");
                })
                .ok()?;
            if let Some(map) = row.as_object_mut() {
                map.insert("expires_at".to_owned(), json!(grant.expires_at()));
            }
            Some(row)
        })
        .collect()
}

/// The observe table on the wire: rows keyed by the proposal root the card would offer (the host
/// for a connection), each with the verdict `enforce` would have reached (`ask` / `deny_row` /
/// `policy_denylist`) and how many observations it stands for, plus how many
/// rows were evicted once the table was full ("and N more").
pub fn observe_summary_to_json(summary: &ObserveSummary) -> Value {
    let entries: Vec<Value> = summary
        .would_block
        .iter()
        .map(|row| {
            json!({
                "kind": row.kind,
                "target": row.target,
                "verdict": row.verdict,
                "count": row.count,
                "last_unix": row.last_at,
            })
        })
        .collect();
    json!({ "entries": entries, "evicted": summary.evicted })
}
