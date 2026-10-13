//! Expiring allow-list rows. Persisted rows live in `sandbox_grants.toml` beside `permission.toml`
//! (workspace scope) or under the grok home (global scope); `Call` and `Session` rows never touch
//! disk. Liveness is a function of an injected [`Clock`], so expiry is testable and the "advance
//! the clock" real test is a one-liner.

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};

use serde::{Deserialize, Serialize};

/// UUIDv7, so rows sort by creation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GrantId(String);

impl GrantId {
    pub fn new(id: impl Into<String>) -> GrantId {
        GrantId(id.into())
    }
}

impl std::fmt::Display for GrantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for GrantId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// `"registry.npmjs.org"`, `"*.npmjs.org"`, or `"*"` (all network).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostPattern(String);

impl HostPattern {
    pub fn new(pattern: impl Into<String>) -> HostPattern {
        HostPattern(pattern.into())
    }

    pub fn all() -> HostPattern {
        HostPattern("*".to_owned())
    }

    pub fn is_all(&self) -> bool {
        self.0 == "*"
    }

    /// Whether `host` is covered: `*` covers every host, `*.suffix` covers proper subdomains of
    /// `suffix` (never `suffix` itself), anything else is an exact case-insensitive match.
    /// Trailing dots are ignored. An IP address is compared as a parsed address, so every
    /// spelling of it is one host and no `*.` pattern covers it. The egress proxy's decider and
    /// the store's deny-shadows-allow rule share this one definition.
    pub fn matches(&self, host: &str) -> bool {
        if self.is_all() {
            return true;
        }
        let host = normalized_host(host);
        let pattern = normalized_host(&self.0);
        if let Some(address) = ip_literal(&host) {
            return ip_literal(&pattern)
                .is_some_and(|p| p.to_canonical() == address.to_canonical());
        }
        match pattern.strip_prefix("*.") {
            Some(suffix) if !suffix.is_empty() => {
                host.len() > suffix.len() + 1
                    && host.ends_with(suffix)
                    && host.as_bytes().get(host.len() - suffix.len() - 1) == Some(&b'.')
            }
            Some(_) => false,
            None => host == pattern,
        }
    }

    /// Whether every host this pattern covers is also covered by `other` (`other` is at least as
    /// broad): the deny-row shadowing rule for two patterns.
    pub fn is_covered_by(&self, other: &HostPattern) -> bool {
        if other.is_all() {
            return true;
        }
        if self.is_all() {
            return false;
        }
        match self.0.strip_prefix("*.") {
            // `*.a.example.com` is within `*.example.com`: the suffix itself is a proper subdomain.
            // An exact host never covers a wildcard, not even the wildcard's own apex
            Some(suffix) => {
                normalized_host(&other.0) == normalized_host(&self.0)
                    || (other.0.starts_with("*.") && other.matches(suffix))
            }
            None => other.matches(&self.0),
        }
    }
}

/// The spelling host comparisons use: lower-case, trailing dots dropped.
fn normalized_host(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// `host` as an IP address, an IPv6 one in brackets or not; `None` for a name.
pub(crate) fn ip_literal(host: &str) -> Option<IpAddr> {
    let bare = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    bare.parse().ok()
}

/// `value` split into its host and the port written after it: `host:443`, `[::1]:443` (the host
/// comes back without its brackets), a bare `host` or `[::1]`, and an unbracketed value with more
/// than one `:`, which is an IPv6 literal with no port. The port is as written, for the caller to
/// parse; a value that does not split this way is a host as written.
pub fn split_host_port(value: &str) -> (&str, Option<&str>) {
    if let Some(rest) = value.strip_prefix('[') {
        return match rest.split_once(']') {
            Some((host, "")) => (host, None),
            Some((host, tail)) => match tail.strip_prefix(':') {
                Some(port) => (host, Some(port)),
                None => (value, None),
            },
            None => (value, None),
        };
    }
    match value.split_once(':') {
        Some((host, port)) if !port.contains(':') => (host, Some(port)),
        _ => (value, None),
    }
}

impl std::fmt::Display for HostPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for HostPattern {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GrantSubject {
    FsWriteRoot {
        root: PathBuf,
    },
    FsRead {
        root: PathBuf,
    },
    NetHost {
        host: HostPattern,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
    },
    /// The curated build-cache trees ([`crate::command::policy::BUILD_CACHE_TREES`]) as one
    /// grant for the workspace — what a toolchain unpacks or runs without re-verifying, so a
    /// poisoned file there would execute in the user's unsandboxed toolchain.
    /// The card names no path: the family is the daemon's table, never the user's pick.
    /// The checksum-verified subpaths are writable without it.
    BuildCaches,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GrantScope {
    Call,
    Session,
    Workspace { root: PathBuf },
    Global,
}

impl GrantScope {
    pub fn is_persisted(&self) -> bool {
        matches!(self, GrantScope::Workspace { .. } | GrantScope::Global)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expiry {
    Ttl { seconds: u64 },
    At { unix: i64 },
    Never,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantDecision {
    #[default]
    Allow,
    /// Deny rows win over allow rows, as the `disallowed_*` sets do.
    Deny,
}

/// Where a grant came from, for the Settings row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// The command text, capped at [`PROVENANCE_COMMAND_MAX_CHARS`] by the writer.
    pub command: String,
    pub tool_call_id: String,
}

pub const PROVENANCE_COMMAND_MAX_CHARS: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub id: GrantId,
    pub subject: GrantSubject,
    pub scope: GrantScope,
    pub expires: Expiry,
    #[serde(default)]
    pub decision: GrantDecision,
    /// Whole unix seconds.
    pub granted_at: i64,
    /// `"desktop:<device_id>"`, `"cli"`, or `"hub:<session_id>"`.
    pub granted_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<Provenance>,
}

impl Grant {
    /// `None` for `Expiry::Never`.
    pub fn expires_at(&self) -> Option<i64> {
        match self.expires {
            Expiry::Ttl { seconds } => Some(
                self.granted_at
                    .saturating_add(i64::try_from(seconds).unwrap_or(i64::MAX)),
            ),
            Expiry::At { unix } => Some(unix),
            Expiry::Never => None,
        }
    }

    pub fn is_live(&self, now_unix: i64) -> bool {
        self.expires_at().is_none_or(|at| now_unix < at)
    }
}

/// The clock every liveness check reads. Production uses [`SystemClock`]; tests and the
/// "advance the clock" real test use [`FixedClock`].
pub trait Clock: Send + Sync {
    fn now_unix(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
    }
}

pub struct FixedClock(AtomicI64);

impl FixedClock {
    pub fn at(unix: i64) -> FixedClock {
        FixedClock(AtomicI64::new(unix))
    }

    pub fn advance(&self, seconds: i64) {
        self.0.fetch_add(seconds, Ordering::SeqCst);
    }
}

impl Clock for FixedClock {
    fn now_unix(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
#[path = "grants_tests.rs"]
mod tests;
