use super::load::load_config_from_toml;
use super::mcp::{Config, user_config_path};
use anyhow::Result;
use toml::Value as TomlValue;
use toml::map::Map as TomlMap;
use xai_grok_agent::prompt::skills::SkillsConfig;
/// Process-wide write lock for `~/.grok/config.toml`.
/// Serializes the read-modify-write in `save_config` so two rapid settings toggles can't interleave and clobber each other.
static SAVE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// Blank (first-run 0-byte file) is an empty table; other unparseable TOML is an error so a silent fallback cannot drop unmodeled sections.
pub(crate) fn parse_existing_config_toml(s: &str) -> Result<TomlValue, toml::de::Error> {
    if s.trim().is_empty() {
        return Ok(TomlValue::Table(TomlMap::new()));
    }
    toml::from_str(s)
}
/// Settings-save body; caller must hold the full [`ConfigWriteGuard`] — [`SAVE_LOCK`] alone races
/// flock-only writers and the last rename drops their edit.
async fn save_config_locked(config: &Config) -> Result<()> {
    let path = user_config_path();
    let mut root: TomlValue = match tokio::fs::read_to_string(&path).await {
        Ok(s) => match parse_existing_config_toml(&s) {
            Ok(v) => v,
            Err(parse_err) => {
                return Err(anyhow::anyhow!(
                    "refusing to overwrite unparseable {}: {}; save a backup \
                         and fix the syntax error before retrying",
                    path.display(),
                    parse_err,
                ));
            }
        },
        Err(_) => TomlValue::Table(TomlMap::new()),
    };
    if !matches!(root, TomlValue::Table(_)) {
        root = TomlValue::Table(TomlMap::new());
    }
    let table = root.as_table_mut().expect("root must be a table");
    merge_section(table, "cli", &config.cli);
    merge_section(table, "models", &config.models);
    merge_section(table, "ui", &config.ui);
    merge_section(table, "harness", &config.harness);
    merge_section(table, "session", &config.session);
    merge_ask_user_question_section(table, &config.ask_user_question);
    if config.privacy == super::mcp::PrivacyConfig::default() {
        table.remove("privacy");
    } else {
        merge_section(table, "privacy", &config.privacy);
    }
    if config.consent == super::consent::ConsentConfig::default() {
        table.remove("consent");
    } else {
        if let Some(TomlValue::Table(section)) = table.get_mut("consent") {
            section.remove("answers");
        }
        merge_section(table, "consent", &config.consent);
    }
    if config.skills == SkillsConfig::default() {
        table.remove("skills");
    } else {
        merge_section(table, "skills", &config.skills);
    }
    merge_section(table, "telemetry", &config.telemetry);
    merge_section(table, "features", &config.features);
    let toml_str = toml::to_string_pretty(&root)?;
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    #[cfg(unix)]
    let prior_mode: Option<u32> = match tokio::fs::metadata(&path).await {
        Ok(m) => {
            use std::os::unix::fs::PermissionsExt;
            Some(m.permissions().mode())
        }
        Err(_) => None,
    };
    #[cfg(not(unix))]
    let prior_mode: Option<u32> = None;
    let suffix = {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("toml.tmp.{}.{}", std::process::id(), nanos)
    };
    let tmp = path.with_extension(suffix);
    tokio::fs::write(&tmp, toml_str).await?;
    #[cfg(unix)]
    if let Some(mode) = prior_mode {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode)).await;
    }
    let _ = prior_mode;
    tokio::fs::rename(&tmp, &path).await?;
    Ok(())
}
/// Guard for a user `config.toml` read-modify-write: [`SAVE_LOCK`] plus the config-init flock —
/// without the flock leg, a SAVE_LOCK writer and a flock writer silently drop each other's edits.
pub(crate) struct ConfigWriteGuard {
    _save: tokio::sync::MutexGuard<'static, ()>,
    _flock: std::fs::File,
}
/// Acquire the user `config.toml` write guard (SAVE_LOCK ⊃ init flock) on the blocking pool;
/// fails closed — callers must not fall back to an unguarded write.
pub(crate) async fn lock_config_writes() -> std::io::Result<ConfigWriteGuard> {
    let save = SAVE_LOCK.lock().await;
    let grok_home = crate::util::grok_home::grok_home();
    let flock = tokio::task::spawn_blocking(move || acquire_init_lock(&grok_home))
        .await
        .map_err(|e| std::io::Error::other(format!("config lock task failed: {e}")))??;
    Ok(ConfigWriteGuard {
        _save: save,
        _flock: flock,
    })
}
/// Exclusive advisory `flock` on `<grok_home>/.config-init.lock`, retried briefly, serializing
/// `config.toml` read-modify-writes; only `WouldBlock` retries, and the file is never removed.
pub fn acquire_init_lock(grok_home: &std::path::Path) -> std::io::Result<std::fs::File> {
    use fs2::FileExt;
    let _ = std::fs::create_dir_all(grok_home);
    let lock_path = grok_home.join(".config-init.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    for _ in 0..50 {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        format!("timed out waiting for {} after 1s", lock_path.display()),
    ))
}
/// Read a file, treating only `NotFound` as empty.
/// Hard read errors (EACCES, EIO) propagate so callers don't clobber an unreadable file on the next write.
pub fn read_to_string_or_empty(path: &std::path::Path) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}
/// Atomic write via temp file then `rename` (mirrors [`save_config`]) so a crash mid-write can't truncate `config.toml`.
/// Preserves the dest mode on unix.
pub fn atomic_write_string(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    #[cfg(unix)]
    let prior_mode: Option<u32> = match std::fs::metadata(path) {
        Ok(m) => {
            use std::os::unix::fs::PermissionsExt;
            Some(m.permissions().mode())
        }
        Err(_) => None,
    };
    #[cfg(not(unix))]
    let prior_mode: Option<u32> = None;
    let suffix = {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("toml.tmp.{}.{}", std::process::id(), nanos)
    };
    let tmp = path.with_extension(suffix);
    std::fs::write(&tmp, content)?;
    #[cfg(unix)]
    if let Some(mode) = prior_mode {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode));
    }
    let _ = prior_mode;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}
/// Merge `[toolset.ask_user_question]` into the root table.
/// `[toolset]` is deliberately NOT merged wholesale, so only this settings-writable sub-table round-trips.
/// It carries runtime-only structs (`web_search` sampler etc.) whose serialized defaults must never land in the user file.
fn merge_ask_user_question_section(
    table: &mut TomlMap<String, TomlValue>,
    ask: &crate::tools::config::AskUserQuestionToolConfig,
) {
    if ask.timeout_enabled.is_none() && ask.timeout_secs.is_none() {
        return;
    }
    let toolset = table
        .entry("toolset".to_string())
        .or_insert_with(|| TomlValue::Table(TomlMap::new()));
    if !matches!(toolset, TomlValue::Table(_)) {
        *toolset = TomlValue::Table(TomlMap::new());
    }
    if let TomlValue::Table(toolset_table) = toolset {
        merge_section(toolset_table, "ask_user_question", ask);
    }
}
/// Merge serialized fields of `value` into `table[key]`, preserving any existing keys not present in the serialized output.
/// This prevents `save_config` round-trips from silently dropping unmodeled fields (e.g. pager-written `show_timestamps`, `auto_dark_theme`).
/// Deep-merge `incoming` into `existing`: nested tables recurse; scalars replace.
fn merge_toml_tables(
    existing: &mut TomlMap<String, TomlValue>,
    incoming: TomlMap<String, TomlValue>,
) {
    for (field_key, field_val) in incoming {
        match (existing.get_mut(&field_key), field_val) {
            (Some(TomlValue::Table(dst)), TomlValue::Table(src)) => {
                merge_toml_tables(dst, src);
            }
            (_, v) => {
                existing.insert(field_key, v);
            }
        }
    }
}
fn merge_section<T: serde::Serialize>(
    table: &mut TomlMap<String, TomlValue>,
    key: &str,
    value: &T,
) {
    match TomlValue::try_from(value) {
        Ok(TomlValue::Table(new_fields)) if !new_fields.is_empty() => {
            let section = table
                .entry(key.to_string())
                .or_insert_with(|| TomlValue::Table(TomlMap::new()));
            if let TomlValue::Table(existing) = section {
                merge_toml_tables(existing, new_fields);
            } else {
                *section = TomlValue::Table(new_fields);
            }
        }
        Ok(TomlValue::Table(_)) => {}
        Ok(_) | Err(_) => {
            table.remove(key);
        }
    }
}
/// Update settings with a read-modify-write, preserving unrelated fields.
pub async fn update_config<F>(f: F) -> Result<()>
where
    F: FnOnce(&mut Config),
{
    let _guard = lock_config_writes().await?;
    let root: TomlValue =
        crate::config::load_from_disk().unwrap_or_else(|_| TomlValue::Table(TomlMap::new()));
    let mut cfg = load_config_from_toml(&root);
    f(&mut cfg);
    save_config_locked(&cfg).await
}
#[cfg(test)]
#[path = "persist_tests.rs"]
mod tests;
