use anyhow::{Context, Result};
use std::{fs::File, io::Write};
use tempfile::NamedTempFile;

/// Materialize the built-in reply-emphasis extension.
///
/// The extension injects the `reply_style` system-prompt section that teaches the agent to write
/// `[[Done]]` pills. The pills are drawn by the Pager's own renderer (`xai-grok-markdown::pill`),
/// which is why this has to be a host extension: Pi runs in `rpc` mode here and its markdown
/// transformers never fire.
pub(super) fn write_emphasis_extension() -> Result<NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix("pi-grok-emphasis-")
        .suffix(".ts")
        .tempfile()
        .context("create Pi emphasis extension tempfile")?;
    const SOURCE: &str = include_str!("../../../../../../extensions/pi-grok-emphasis/index.ts");
    file.write_all(SOURCE.as_bytes())
        .context("write Pi emphasis extension source")?;
    file.flush()
        .context("flush Pi emphasis extension source")?;
    File::open(file.path())
        .and_then(|source| source.sync_all())
        .ok();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The extension is source, not a compiled artifact: assert the shape the host needs.
    #[test]
    fn emphasis_extension_is_a_prompt_section_without_a_tui_gate() {
        let file = write_emphasis_extension().expect("write extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        // The rules travel as a system-prompt section, set where Pi still runs them in rpc mode.
        assert!(source.contains("before_agent_start"));
        assert!(source.contains("reply_style"));
        // A `tui` gate would silence the rules under grok-pi's rpc host. The extension's own
        // comment explains that, so the check is for the guard in code, not the prose about it.
        for line in source.lines() {
            let code = line.trim_start();
            if code.starts_with("//") || code.starts_with('*') {
                continue;
            }
            assert!(
                !code.contains("ctx.mode"),
                "the extension must not gate on the run mode: {line}"
            );
        }
        // Turning the rules off must be possible without removing the extension.
        assert!(source.contains("PI_GROK_EMPHASIS_GUIDANCE"));
    }

    /// Every tone's word list exists in the source, so no rule can promise a pill with no colour.
    ///
    /// The words are emitted at runtime from these tables, so the assertion is on the tables.
    #[test]
    fn every_tone_has_a_taught_word_list() {
        let file = write_emphasis_extension().expect("write extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        for tone in [
            "error", "warning", "success", "danger", "mode", "owner",
        ] {
            assert!(
                source.contains(&format!("  {tone}: {{")),
                "the {tone} tone needs a word list"
            );
        }
        // The tables are read by the guidance, so the rules and the draw cannot drift.
        assert!(source.contains("v.error.core"));
        assert!(source.contains("v.warning.core"));
        assert!(source.contains("v.success.core"));
        assert!(source.contains("v.danger.core"));
        assert!(source.contains("v.mode.core"));
        assert!(source.contains("v.owner.core"));
        // Context words have no tone but are still taught.
        assert!(source.contains("CONTEXT_WORDS"));
    }
}
