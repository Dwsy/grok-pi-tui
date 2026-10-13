use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProxyOutcome {
    Ok,
    Malformed,
    Unauthenticated,
    PolicyDenied,
    AddressDenied,
    DnsFailed,
    ConnectFailed,
    Timeout,
    Overloaded,
    TlsDenied,
    /// A held request that the decider did not answer before the hold deadline.
    HoldTimeout,
    /// The client hung up while its request was held.
    Abandoned,
}

impl ProxyOutcome {
    /// Every variant, in discriminant order: the counter array is sized from it, so a new
    /// variant left out of the list fails `every_outcome_has_its_counter`, not `record`.
    pub(crate) const ALL: [ProxyOutcome; 12] = [
        ProxyOutcome::Ok,
        ProxyOutcome::Malformed,
        ProxyOutcome::Unauthenticated,
        ProxyOutcome::PolicyDenied,
        ProxyOutcome::AddressDenied,
        ProxyOutcome::DnsFailed,
        ProxyOutcome::ConnectFailed,
        ProxyOutcome::Timeout,
        ProxyOutcome::Overloaded,
        ProxyOutcome::TlsDenied,
        ProxyOutcome::HoldTimeout,
        ProxyOutcome::Abandoned,
    ];
}

const OUTCOME_COUNT: usize = ProxyOutcome::ALL.len();

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProxyMetricsSnapshot {
    pub ok: u64,
    pub malformed: u64,
    pub unauthenticated: u64,
    pub policy_denied: u64,
    pub address_denied: u64,
    pub dns_failed: u64,
    pub connect_failed: u64,
    pub timeout: u64,
    pub overloaded: u64,
    pub tls_denied: u64,
    pub hold_timeout: u64,
    pub abandoned: u64,
}

#[derive(Default)]
pub(crate) struct ProxyMetrics {
    values: [AtomicU64; OUTCOME_COUNT],
}

impl ProxyMetrics {
    pub(crate) fn record(&self, outcome: ProxyOutcome) {
        self.values[outcome as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> ProxyMetricsSnapshot {
        let get = |outcome: ProxyOutcome| self.values[outcome as usize].load(Ordering::Relaxed);
        ProxyMetricsSnapshot {
            ok: get(ProxyOutcome::Ok),
            malformed: get(ProxyOutcome::Malformed),
            unauthenticated: get(ProxyOutcome::Unauthenticated),
            policy_denied: get(ProxyOutcome::PolicyDenied),
            address_denied: get(ProxyOutcome::AddressDenied),
            dns_failed: get(ProxyOutcome::DnsFailed),
            connect_failed: get(ProxyOutcome::ConnectFailed),
            timeout: get(ProxyOutcome::Timeout),
            overloaded: get(ProxyOutcome::Overloaded),
            tls_denied: get(ProxyOutcome::TlsDenied),
            hold_timeout: get(ProxyOutcome::HoldTimeout),
            abandoned: get(ProxyOutcome::Abandoned),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A variant added to `ProxyOutcome` without a slot in `ALL` would index past the counters
    /// on its first `record`. The match below is the guard: a variant it does not name fails
    /// to compile, and every named one is checked to sit in `ALL` at its discriminant.
    #[test]
    fn every_outcome_has_its_counter() {
        for (index, outcome) in ProxyOutcome::ALL.iter().enumerate() {
            let name = match outcome {
                ProxyOutcome::Ok => "ok",
                ProxyOutcome::Malformed => "malformed",
                ProxyOutcome::Unauthenticated => "unauthenticated",
                ProxyOutcome::PolicyDenied => "policy_denied",
                ProxyOutcome::AddressDenied => "address_denied",
                ProxyOutcome::DnsFailed => "dns_failed",
                ProxyOutcome::ConnectFailed => "connect_failed",
                ProxyOutcome::Timeout => "timeout",
                ProxyOutcome::Overloaded => "overloaded",
                ProxyOutcome::TlsDenied => "tls_denied",
                ProxyOutcome::HoldTimeout => "hold_timeout",
                ProxyOutcome::Abandoned => "abandoned",
            };
            assert_eq!(index, *outcome as usize, "{name} is out of order in ALL");
        }
        let metrics = ProxyMetrics::default();
        for outcome in ProxyOutcome::ALL {
            metrics.record(outcome);
        }
        let snapshot = metrics.snapshot();
        assert_eq!(
            ProxyMetricsSnapshot {
                ok: 1,
                malformed: 1,
                unauthenticated: 1,
                policy_denied: 1,
                address_denied: 1,
                dns_failed: 1,
                connect_failed: 1,
                timeout: 1,
                overloaded: 1,
                tls_denied: 1,
                hold_timeout: 1,
                abandoned: 1,
            },
            snapshot
        );
    }
}
