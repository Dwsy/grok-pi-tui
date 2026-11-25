use anyhow::Result;

/// Materialize the Rust TUI bridge extension before user extensions load.
///
/// This thin Pi-side bridge executes component factories and sends pre-rendered
/// frames to the Rust Pager via RPC notifications, replacing the TS remote-tui's
/// temp-file key transport.
pub(super) fn write_rust_tui_bridge_extension() -> Result<super::tui_kit::CachedExtensionFile> {
    const SOURCE: &str =
        include_str!("../../../../../../extensions/pi-grok-rust-tui-bridge/index.ts");
    super::tui_kit::materialize_cached_file("pi-grok-rust-tui-bridge", SOURCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_tui_bridge_extension_executes_factories_via_rpc() {
        let file = write_rust_tui_bridge_extension().expect("write extension");
        let source = std::fs::read_to_string(file.path()).expect("read extension");
        assert!(source.contains("pi/ui/remote_tui"));
        assert!(source.contains("component.render"));
        assert!(source.contains("handleInput"));
        assert!(source.contains("overlay_push"));
        assert!(source.contains("overlay_pop"));
        // Must NOT use temp-file key transport (literal path helpers / fs.watch).
        assert!(!source.contains("keyfile"));
        assert!(!source.contains("watch("));
    }
}
