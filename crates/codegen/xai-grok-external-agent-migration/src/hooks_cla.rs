use std::path::Path;

use tracing::{debug, info, warn};

use super::config_values::ImportableItem;
use super::error::MigrationError;

pub(super) fn extract_hooks_from_settings_file(path: &Path) -> Vec<ImportableItem> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return Vec::new();
    };
    let Some(hooks_obj) = value.get("hooks").and_then(|v| v.as_object()) else {
        return Vec::new();
    };

    let mut items = Vec::new();
    for (event, groups_val) in hooks_obj {
        let Some(groups) = groups_val.as_array() else {
            continue;
        };
        for group in groups {
            let matcher = group
                .get("matcher")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let Some(handlers) = group.get("hooks").and_then(|v| v.as_array()) else {
                continue;
            };
            for handler in handlers {
                let handler_type = handler.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if handler_type != "command" {
                    debug!(
                        path = %path.display(),
                        event = %event,
                        handler_type = %handler_type,
                        "Skipping non-command hook handler during import"
                    );
                    continue;
                }
                let Some(command) = handler.get("command").and_then(|v| v.as_str()) else {
                    continue;
                };
                let timeout = handler.get("timeout").and_then(serde_json::Value::as_u64);
                items.push(ImportableItem::Hook {
                    event: event.clone(),
                    matcher: matcher.clone(),
                    command: command.to_string(),
                    timeout,
                });
            }
        }
    }
    items
}

// imported-from-claude.json is rewritten even when only a hook timeout changes
pub(super) fn apply_hooks_to_dir(
    hooks_dir: &Path,
    items: &[ImportableItem],
) -> Result<usize, MigrationError> {
    let new_hooks: Vec<&ImportableItem> = items
        .iter()
        .filter(|item| matches!(item, ImportableItem::Hook { .. }))
        .collect();
    if new_hooks.is_empty() {
        return Ok(0);
    }

    let target = hooks_dir.join("imported-from-claude.json");
    let mut root = read_hooks_root(&target)?;
    let root_obj = root
        .as_object_mut()
        .ok_or_else(|| MigrationError::NotAJsonObject {
            place: format!("{}: root", target.display()),
        })?;
    let hooks_obj = root_obj
        .entry("hooks".to_string())
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| MigrationError::NotAJsonObject {
            place: format!("{}: hooks", target.display()),
        })?;

    let mut count = 0usize;
    let mut dirty = false;
    for item in new_hooks {
        let ImportableItem::Hook {
            event,
            matcher,
            command,
            timeout,
        } = item
        else {
            continue;
        };

        let groups = hooks_obj
            .entry(event.clone())
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or_else(|| MigrationError::NotAJsonArray {
                path: target.clone(),
                event: event.clone(),
            })?;

        if refresh_existing_hook_timeout(groups, matcher.as_deref(), command, *timeout) {
            debug!(
                event = %event,
                matcher = ?matcher,
                command = %command,
                timeout = ?timeout,
                "Hook already present; refreshed timeout in place"
            );
            dirty = true;
            continue;
        }

        let mut handler = serde_json::Map::new();
        handler.insert("type".to_string(), serde_json::json!("command"));
        handler.insert("command".to_string(), serde_json::json!(command));
        if let Some(timeout) = timeout {
            handler.insert("timeout".to_string(), serde_json::json!(timeout));
        }

        let mut group = serde_json::Map::new();
        group.insert(
            "hooks".to_string(),
            serde_json::Value::Array(vec![serde_json::Value::Object(handler)]),
        );
        if let Some(matcher) = matcher {
            group.insert("matcher".to_string(), serde_json::json!(matcher));
        }
        groups.push(serde_json::Value::Object(group));
        count += 1;
    }

    if count > 0 || dirty {
        std::fs::create_dir_all(hooks_dir)
            .map_err(|source| MigrationError::io("create directory", hooks_dir, source))?;
        let json_str = serde_json::to_string_pretty(&root).map_err(|source| {
            MigrationError::JsonSerialize {
                path: target.clone(),
                source,
            }
        })?;
        let tmp = target.with_extension("json.tmp");
        std::fs::write(&tmp, &json_str)
            .map_err(|source| MigrationError::io("write", &tmp, source))?;
        std::fs::rename(&tmp, &target)
            .map_err(|source| MigrationError::io("rename", &target, source))?;
        info!(
            path = %target.display(),
            count,
            "Wrote imported hooks to .grok/hooks/imported-from-claude.json"
        );
    }

    Ok(count)
}

fn read_hooks_root(target: &Path) -> Result<serde_json::Value, MigrationError> {
    match std::fs::read_to_string(target) {
        Ok(contents) => Ok(serde_json::from_str(&contents).unwrap_or_else(|error| {
            warn!(
                path = %target.display(),
                error = %error,
                "Existing imported-from-claude.json is malformed; replacing with fresh content. \
                 Manual edits in the malformed file will be lost."
            );
            serde_json::json!({})
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(source) => Err(MigrationError::io("read", target, source)),
    }
}

fn refresh_existing_hook_timeout(
    groups: &mut [serde_json::Value],
    matcher: Option<&str>,
    command: &str,
    timeout: Option<u64>,
) -> bool {
    for group in groups.iter_mut() {
        if group.get("matcher").and_then(|v| v.as_str()) != matcher {
            continue;
        }
        let Some(handlers) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) else {
            continue;
        };
        for handler in handlers.iter_mut() {
            let cmd_match = handler.get("type").and_then(|v| v.as_str()) == Some("command")
                && handler.get("command").and_then(|v| v.as_str()) == Some(command);
            if !cmd_match {
                continue;
            }
            if let Some(handler_obj) = handler.as_object_mut() {
                match timeout {
                    Some(timeout) => {
                        handler_obj.insert("timeout".to_string(), serde_json::json!(timeout));
                    }
                    None => {
                        handler_obj.remove("timeout");
                    }
                }
            }
            return true;
        }
    }
    false
}

#[cfg(test)]
#[path = "hooks_cla_tests.rs"]
mod tests;
