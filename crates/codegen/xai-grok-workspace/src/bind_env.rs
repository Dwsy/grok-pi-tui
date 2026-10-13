//! Shell environment for a hub session bound with a `session_root`
//! (`/workspace/<conversation_id>`): its own token file, tmp tree, and
//! default port.

use std::collections::HashMap;
use std::path::Path;

/// Tools that refresh their token from a file read this path ahead of the
/// default token file.
pub(crate) const TERMINAL_JWT_FILE: &str = "TERMINAL_JWT_FILE";

/// `PORT` is advisory: a deterministic per-session default. It is not
/// reserved and nothing stops a tool from binding another port.
pub(crate) const PORT_RANGE_START: u32 = 20_000;
pub(crate) const PORT_RANGE_LEN: u32 = 10_000;

/// Env for the session bound at `real_root`. `None` when the root has no
/// last path segment. Tmp-derived entries are skipped when the session tmp
/// dir cannot be created.
pub(crate) fn bind_session_env(
    real_root: &str,
    hub_session_id: &str,
) -> Option<HashMap<String, String>> {
    let conversation_id = Path::new(real_root).file_name()?.to_str()?;
    let tmp_dir = match crate::session::tool_config::ensure_private_session_tmp_dir(hub_session_id)
    {
        Ok(dir) => Some(dir),
        Err(e) => {
            tracing::warn!(
                session_id = %hub_session_id,
                error = %e,
                "session.bind: could not create the per-session tmp dir; TMPDIR stays global"
            );
            None
        }
    };
    Some(conversation_session_env(
        conversation_id,
        hub_session_id,
        tmp_dir.as_deref(),
    ))
}

pub(crate) fn conversation_session_env(
    conversation_id: &str,
    hub_session_id: &str,
    tmp_dir: Option<&Path>,
) -> HashMap<String, String> {
    let mut env = HashMap::from([
        (
            TERMINAL_JWT_FILE.to_owned(),
            xai_grok_workspace_types::grok_files_conversation_jwt_path(conversation_id),
        ),
        ("PORT".to_owned(), advisory_port(hub_session_id).to_string()),
    ]);
    if let Some(tmp) = tmp_dir {
        let tmp = tmp.to_string_lossy();
        env.extend([
            ("TMPDIR".to_owned(), tmp.to_string()),
            ("XDG_CACHE_HOME".to_owned(), format!("{tmp}/cache")),
            ("XDG_STATE_HOME".to_owned(), format!("{tmp}/state")),
            ("npm_config_cache".to_owned(), format!("{tmp}/npm-cache")),
        ]);
    }
    env
}

pub(crate) fn advisory_port(hub_session_id: &str) -> u32 {
    PORT_RANGE_START + (fnv1a64(hub_session_id.as_bytes()) % u64::from(PORT_RANGE_LEN)) as u32
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_is_keyed_by_conversation_and_session() {
        let env = conversation_session_env(
            "conv-abc",
            "hub-sess-1",
            Some(Path::new("/tmp/sessions/hub-sess-1")),
        );
        assert_eq!(
            env.get(TERMINAL_JWT_FILE).map(String::as_str),
            Some("/etc/secrets/terminal.conv-abc.jwt")
        );
        assert_eq!(
            env.get("TMPDIR").map(String::as_str),
            Some("/tmp/sessions/hub-sess-1")
        );
        assert_eq!(
            env.get("XDG_CACHE_HOME").map(String::as_str),
            Some("/tmp/sessions/hub-sess-1/cache")
        );
        assert_eq!(
            env.get("XDG_STATE_HOME").map(String::as_str),
            Some("/tmp/sessions/hub-sess-1/state")
        );
        assert_eq!(
            env.get("npm_config_cache").map(String::as_str),
            Some("/tmp/sessions/hub-sess-1/npm-cache")
        );
        assert_eq!(
            env.get("PORT").map(String::as_str),
            Some(advisory_port("hub-sess-1").to_string().as_str())
        );
        assert_eq!(env.len(), 6);
    }

    #[test]
    fn tmp_entries_are_skipped_without_a_tmp_dir() {
        let env = conversation_session_env("conv-abc", "hub-sess-1", None);
        assert_eq!(
            env.keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            [TERMINAL_JWT_FILE, "PORT"].into_iter().collect()
        );
    }

    #[test]
    fn advisory_port_is_deterministic_and_in_range() {
        assert_eq!(advisory_port("hub-sess-1"), advisory_port("hub-sess-1"));
        assert_ne!(advisory_port("hub-sess-1"), advisory_port("hub-sess-2"));
        for id in ["", "a", "hub-sess-1", "0123456789abcdef0123456789abcdef"] {
            let port = advisory_port(id);
            assert!(
                (PORT_RANGE_START..PORT_RANGE_START + PORT_RANGE_LEN).contains(&port),
                "{id:?} -> {port}"
            );
        }
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn bind_session_env_takes_the_last_root_segment() {
        let env = bind_session_env("/workspace/conv-xyz", "hub-sess-env").expect("env");
        assert_eq!(
            env.get(TERMINAL_JWT_FILE).map(String::as_str),
            Some("/etc/secrets/terminal.conv-xyz.jwt")
        );
        assert_eq!(bind_session_env("/", "hub-sess-env"), None);
    }
}
