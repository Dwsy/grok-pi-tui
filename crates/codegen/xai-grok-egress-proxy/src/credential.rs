//! Per-call proxy credentials: the wrapper points the child at
//! `http://grok:<call-token>@127.0.0.1:<port>`, clients send the token as `Proxy-Authorization`,
//! and the proxy maps it back to the [`CommandTag`] it was minted for (revoked tag → 407; the
//! session-wide bearer token has no call attached). Only the password is the secret.
//!
//! This is attribution, not isolation: the token sits in the child's environment, so any
//! process of the same uid can read and present it; what holds is that it dies with its call.

use std::io;
use std::net::SocketAddr;
use std::sync::Mutex;

use base64::Engine;
use rand::TryRngCore;
use rand::rngs::OsRng;
use subtle::ConstantTimeEq;
use xai_grok_sandbox::command::CommandTag;

use crate::error::ProxyError;

/// The user name in the proxy URL. Cosmetic: authentication compares the password only.
pub const PROXY_USERNAME: &str = "grok";

/// The secret half of a proxy credential (the session bearer or one call's password). `Debug`
/// prints `ProxyToken(***)`, equality is constant-time, and the bytes leave only through
/// [`ProxyToken::expose`].
#[derive(Clone)]
pub struct ProxyToken(String);

impl ProxyToken {
    pub(crate) fn generate() -> io::Result<ProxyToken> {
        let mut bytes = [0u8; 32];
        OsRng.try_fill_bytes(&mut bytes).map_err(io::Error::other)?;
        Ok(ProxyToken(
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes),
        ))
    }

    /// The secret itself, for the one place that writes it into a URL or a header.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    fn matches(&self, presented: &str) -> bool {
        presented.as_bytes().ct_eq(self.0.as_bytes()).unwrap_u8() == 1
    }
}

impl std::fmt::Debug for ProxyToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProxyToken(***)")
    }
}

impl PartialEq for ProxyToken {
    fn eq(&self, other: &ProxyToken) -> bool {
        self.matches(&other.0)
    }
}

impl Eq for ProxyToken {}

/// A short-lived credential minted for one command; drop it into the child's `HTTP_PROXY`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyCredential {
    tag: CommandTag,
    token: ProxyToken,
}

impl ProxyCredential {
    pub fn tag(&self) -> &CommandTag {
        &self.tag
    }

    /// `http://grok:<token>@<address>`, the value for `HTTP_PROXY`/`HTTPS_PROXY`.
    pub fn proxy_url(&self, address: SocketAddr) -> String {
        format!("http://{PROXY_USERNAME}:{}@{address}", self.token.expose())
    }

    /// The header a client that cannot read userinfo must send.
    pub fn proxy_authorization(&self) -> String {
        format!("Basic {}", basic_credentials(self.token.expose()))
    }
}

fn basic_credentials(token: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("{PROXY_USERNAME}:{token}"))
}

pub(crate) struct CredentialRegistry {
    session_token: ProxyToken,
    calls: Mutex<Vec<ProxyCredential>>,
}

impl CredentialRegistry {
    pub(crate) fn new(session_token: ProxyToken) -> CredentialRegistry {
        CredentialRegistry {
            session_token,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn session_authorization(&self) -> String {
        format!("Bearer {}", self.session_token.expose())
    }

    /// Minting again for a live tag replaces its credential, so a stale copy stops working.
    pub(crate) fn mint(&self, tag: &CommandTag) -> io::Result<ProxyCredential> {
        let credential = ProxyCredential {
            tag: tag.clone(),
            token: ProxyToken::generate()?,
        };
        let mut calls = self
            .calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        calls.retain(|existing| existing.tag != *tag);
        calls.push(credential.clone());
        Ok(credential)
    }

    pub(crate) fn revoke(&self, tag: &CommandTag) -> bool {
        let mut calls = self
            .calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let before = calls.len();
        calls.retain(|existing| existing.tag != *tag);
        calls.len() != before
    }

    /// Revokes `credential` only while it is its tag's live one: a later mint for the tag
    /// replaced it already, and that one stays.
    pub(crate) fn revoke_exact(&self, credential: &ProxyCredential) -> bool {
        let mut calls = self
            .calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let before = calls.len();
        calls.retain(|existing| existing != credential);
        calls.len() != before
    }

    /// `Ok(None)` is the session token; `Ok(Some(tag))` a live call credential.
    pub(crate) fn authenticate(&self, header: &str) -> Result<Option<CommandTag>, ProxyError> {
        let token = presented_token(header).ok_or(ProxyError::Authentication)?;
        if self.session_token.matches(&token) {
            return Ok(None);
        }
        let calls = self
            .calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        calls
            .iter()
            .find(|credential| credential.token.matches(&token))
            .map(|credential| Some(credential.tag.clone()))
            .ok_or(ProxyError::Authentication)
    }
}

/// Accepts `Bearer <token>` and `Basic base64(<user>:<token>)`; anything else is unauthenticated.
fn presented_token(header: &str) -> Option<String> {
    let (scheme, value) = header.trim().split_once(' ')?;
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if scheme.eq_ignore_ascii_case("bearer") {
        return Some(value.to_owned());
    }
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (_, password) = decoded.split_once(':')?;
    (!password.is_empty()).then(|| password.to_owned())
}
