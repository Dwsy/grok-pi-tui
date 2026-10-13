//! This module caches no settings.
//! [`crate::settings_cache`] keeps settings on disk between launches.

use tokio::sync::watch;
use xai_grok_login::GrokAuth;

/// Shares one live settings fetch between concurrent mid-session refreshes.
#[derive(Default)]
pub struct SettingsRefresh {
    state: std::cell::RefCell<RefreshState>,
}

impl SettingsRefresh {
    const MAX_DROPPED_LEADER_REATTEMPTS: u32 = 3;

    /// A caller that joins a running fetch for `auth` never calls its own `leader`.
    /// Returns `None` when leaders keep dropping before they publish a result.
    pub async fn refresh<F, Fut>(&self, auth: &GrokAuth, leader: F) -> Option<crate::SettingsFetch>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = crate::SettingsFetch>,
    {
        let identity = CredentialIdentity::from(auth);
        let mut leader = Some(leader);
        let mut dropped_leader_reattempts = 0u32;

        loop {
            let (plan, my_epoch) = self.plan_attempt(&identity);

            match plan {
                RefreshPlan::Join(mut rx) => {
                    let published = rx
                        .wait_for(Option::is_some)
                        .await
                        .ok()
                        .and_then(|outcome| outcome.clone());
                    match published {
                        Some(outcome) => return Some(outcome),
                        None => {
                            dropped_leader_reattempts += 1;
                            if dropped_leader_reattempts > Self::MAX_DROPPED_LEADER_REATTEMPTS {
                                return None;
                            }
                        }
                    }
                }
                RefreshPlan::Lead(tx) => {
                    let mut guard = RefreshLeaderGuard {
                        refresh: self,
                        epoch: my_epoch,
                        published: false,
                    };
                    let run = leader.take().expect("leader closure consumed at most once");
                    let outcome = run().await;

                    self.clear_running_fetch_if_current(my_epoch);
                    guard.published = true;
                    let _ = tx.send(Some(outcome.clone()));
                    return Some(outcome);
                }
            }
        }
    }

    fn plan_attempt(&self, identity: &CredentialIdentity) -> (RefreshPlan, u64) {
        let mut st = self.state.borrow_mut();
        if st.identity.as_ref() != Some(identity) {
            st.reset_to(identity.clone());
        }

        let my_epoch = st.epoch;
        let plan = if let Some(rx) = st.running_fetch.as_ref() {
            RefreshPlan::Join(rx.clone())
        } else {
            let (tx, rx) = watch::channel(None);
            st.running_fetch = Some(rx);
            RefreshPlan::Lead(tx)
        };
        (plan, my_epoch)
    }

    fn clear_running_fetch_if_current(&self, epoch: u64) {
        let mut st = self.state.borrow_mut();
        if st.epoch == epoch {
            st.running_fetch = None;
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct CredentialIdentity {
    user_id: String,
    key: String,
}

impl From<&GrokAuth> for CredentialIdentity {
    fn from(auth: &GrokAuth) -> Self {
        Self {
            user_id: auth.user_id.clone(),
            key: auth.key.clone(),
        }
    }
}

#[derive(Default)]
struct RefreshState {
    /// Increases on every credential switch and never resets.
    /// A leader from before a switch back to its credential cannot clear the newer `running_fetch`.
    epoch: u64,
    identity: Option<CredentialIdentity>,
    running_fetch: Option<watch::Receiver<Option<crate::SettingsFetch>>>,
}

impl RefreshState {
    fn reset_to(&mut self, identity: CredentialIdentity) {
        self.epoch += 1;
        self.identity = Some(identity);
        self.running_fetch = None;
    }
}

enum RefreshPlan {
    Join(watch::Receiver<Option<crate::SettingsFetch>>),
    Lead(watch::Sender<Option<crate::SettingsFetch>>),
}

struct RefreshLeaderGuard<'a> {
    refresh: &'a SettingsRefresh,
    epoch: u64,
    published: bool,
}

impl Drop for RefreshLeaderGuard<'_> {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        self.refresh.clear_running_fetch_if_current(self.epoch);
    }
}

#[cfg(test)]
mod tests {
    use super::SettingsRefresh;
    use crate::SettingsFetch;

    #[tokio::test(flavor = "current_thread")]
    async fn refresh_coalesces_concurrent_callers() {
        let refresh = SettingsRefresh::default();
        let calls = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let leader = || {
            let calls = calls.clone();
            move || async move {
                calls.set(calls.get() + 1);
                tokio::task::yield_now().await;
                SettingsFetch::Fetched(Box::default())
            }
        };
        let auth = xai_grok_login::GrokAuth::test_default();

        let cluster = (0..5).map(|_| refresh.refresh(&auth, leader()));
        let outcomes = futures::future::join_all(cluster).await;

        assert!(
            outcomes
                .iter()
                .all(|o| matches!(o, Some(SettingsFetch::Fetched(_)))),
            "every coalesced caller receives the shared success"
        );
        assert_eq!(calls.get(), 1, "concurrent callers share one leader fetch");

        let _ = refresh.refresh(&auth, leader()).await;
        assert_eq!(
            calls.get(),
            2,
            "a sequential trigger re-fetches; no stale replay"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn follower_refetches_after_leader_is_dropped() {
        let refresh = SettingsRefresh::default();
        let auth = xai_grok_login::GrokAuth::test_default();

        let mut leader = Box::pin(refresh.refresh(&auth, std::future::pending::<SettingsFetch>));
        tokio::select! {
            biased;
            _ = &mut leader => unreachable!("a pending leader cannot complete"),
            _ = tokio::task::yield_now() => {}
        }

        let calls = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let calls_follower = calls.clone();
        let mut follower = Box::pin(refresh.refresh(&auth, move || async move {
            calls_follower.set(calls_follower.get() + 1);
            SettingsFetch::Fetched(Box::default())
        }));
        tokio::select! {
            biased;
            _ = &mut follower => unreachable!("the follower must park on the in-flight leader"),
            _ = tokio::task::yield_now() => {}
        }
        assert_eq!(calls.get(), 0, "a joining follower must not fetch");

        drop(leader);

        assert!(
            matches!(follower.await, Some(SettingsFetch::Fetched(_))),
            "a cancelled leader drives a real re-fetch, not a gate-opening Retry"
        );
        assert_eq!(
            calls.get(),
            1,
            "the follower re-plans and leads exactly one fresh fetch"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn refresh_returns_none_after_repeated_leader_drops() {
        let refresh = SettingsRefresh::default();
        let auth = xai_grok_login::GrokAuth::test_default();

        macro_rules! new_fetch {
            () => {
                Box::pin(refresh.refresh(&auth, std::future::pending::<SettingsFetch>))
            };
        }

        macro_rules! assert_pending {
            ($fut:expr, $msg:expr) => {
                tokio::select! {
                    biased;
                    _ = &mut $fut => unreachable!($msg),
                    _ = tokio::task::yield_now() => {}
                }
            };
        }

        let mut leader = new_fetch!();
        assert_pending!(leader, "the first leader parks on its pending fetch");
        let mut follower = new_fetch!();
        assert_pending!(follower, "the follower joins the first in-flight leader");

        for _ in 0..3 {
            let mut next = new_fetch!();
            drop(leader);
            assert_pending!(next, "a fresh leader grabs the freed lead slot");
            assert_pending!(
                follower,
                "the follower re-joins the fresh leader after a drop"
            );
            leader = next;
        }

        drop(leader);

        assert!(
            follower.await.is_none(),
            "past the reattempt budget the follower yields None, not a gate-opening Retry"
        );
    }
}
