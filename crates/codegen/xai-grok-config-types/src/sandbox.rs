//! The `[sandbox]` table of the workspace daemon's own settings file, `workspaced.toml`
//! (`<grok_home>/workspaced.toml` for the user layer, `<workspace>/.grok/workspaced.toml` for the
//! folder's own, tighten-only layer). The daemon owns this file outright: nothing here is read
//! from or written to `config.toml`, whose `[sandbox]` table (`profile`, `auto_allow_bash`) stays
//! the `grok` CLI's.

use serde::{Deserialize, Serialize};
pub use xai_grok_config::sandbox_mode::optional_sandbox_mode;
use xai_grok_sandbox::command::SandboxMode;

/// The daemon's settings file name, under the grok home and under a folder's `.grok/`: the one
/// name the loader reads and the floor protects.
pub use xai_grok_sandbox::command::protected::DAEMON_SETTINGS_FILENAME as WORKSPACED_CONFIG_FILENAME;

/// The `[sandbox]` table as one `workspaced.toml` layer carries it. Every key is optional:
/// `None` leaves the decision to the next layer.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct SandboxSettings {
    /// Per-command sandbox rollout mode, in any case (`enforce`, `Enforce`). A value that names
    /// no mode reads as unset, so a typo cannot drop the table's other keys or decide the mode.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_sandbox_mode"
    )]
    pub mode: Option<SandboxMode>,
}

impl SandboxSettings {
    /// Pull `[sandbox]` out of a loaded `workspaced.toml` document. `None` when the root is not a
    /// table or has no `sandbox` table; a `sandbox` table whose keys are wrongly typed is logged
    /// and treated as absent.
    pub fn from_config_document(root: &toml::Value) -> Option<SandboxSettings> {
        let table = root.as_table()?.get("sandbox")?;
        SandboxSettings::deserialize(table.clone())
            .inspect_err(|error| {
                tracing::warn!(%error, "[sandbox] did not deserialize; treating the layer as unset");
            })
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::SandboxSettings;
    use xai_grok_sandbox::command::SandboxMode;

    #[test]
    fn mode_is_read_from_the_sandbox_table() {
        let root: toml::Value = toml::from_str("[sandbox]\nmode = \"enforce\"\n").unwrap();
        assert_eq!(
            Some(SandboxSettings {
                mode: Some(SandboxMode::Enforce),
            }),
            SandboxSettings::from_config_document(&root)
        );
    }

    #[test]
    fn absent_table_and_absent_key_are_none() {
        let no_table: toml::Value = toml::from_str("[proxy]\nport = 3128\n").unwrap();
        assert_eq!(None, SandboxSettings::from_config_document(&no_table));
        let no_key: toml::Value = toml::from_str("[sandbox]\nfuture_key = 1\n").unwrap();
        assert_eq!(
            Some(SandboxSettings::default()),
            SandboxSettings::from_config_document(&no_key)
        );
    }

    /// The CLI's `config.toml` keys are not this file's; a copy-paste of that table is read for
    /// `mode` alone and nothing else is declared unknown.
    #[test]
    fn the_shell_keys_are_ignored_beside_mode() {
        let root: toml::Value = toml::from_str(
            "[sandbox]\nprofile = \"workspace\"\nauto_allow_bash = true\nmode = \"off\"\n",
        )
        .unwrap();
        assert_eq!(
            Some(SandboxSettings {
                mode: Some(SandboxMode::Off),
            }),
            SandboxSettings::from_config_document(&root)
        );
    }

    /// The file accepts what `GROK_SANDBOX_MODE` accepts: any case, surrounding whitespace.
    #[test]
    fn mode_is_read_in_any_case() {
        for (text, mode) in [
            ("[sandbox]\nmode = \"Enforce\"\n", SandboxMode::Enforce),
            ("[sandbox]\nmode = \"ENFORCE\"\n", SandboxMode::Enforce),
            ("[sandbox]\nmode = \" Observe \"\n", SandboxMode::Observe),
            ("[sandbox]\nmode = \"OFF\"\n", SandboxMode::Off),
        ] {
            let root: toml::Value = toml::from_str(text).unwrap();
            assert_eq!(
                Some(SandboxSettings { mode: Some(mode) }),
                SandboxSettings::from_config_document(&root),
                "{text}"
            );
        }
    }

    /// A `$VAR` reference is text like any other: nothing expands it, so it names no mode.
    #[test]
    fn wrongly_typed_or_misspelt_mode_is_unset() {
        for text in [
            "[sandbox]\nmode = 3\n",
            "[sandbox]\nmode = \"enforcee\"\n",
            "[sandbox]\nmode = \"${GROK_SANDBOX_MODE_UNSET_VAR:-enforce}\"\n",
            "[sandbox]\nmode = [\"enforce\"]\n",
        ] {
            let root: toml::Value = toml::from_str(text).unwrap();
            assert_eq!(
                Some(SandboxSettings::default()),
                SandboxSettings::from_config_document(&root),
                "{text}"
            );
        }
    }

    #[test]
    fn settings_round_trip_through_toml_without_empty_keys() {
        let settings = SandboxSettings {
            mode: Some(SandboxMode::Observe),
        };
        let text = toml::to_string(&settings).unwrap();
        assert_eq!("mode = \"observe\"\n", text);
        assert_eq!(settings, toml::from_str(&text).unwrap());
        assert_eq!("", toml::to_string(&SandboxSettings::default()).unwrap());
    }
}
