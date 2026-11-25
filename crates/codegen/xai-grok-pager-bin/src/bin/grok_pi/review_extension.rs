use anyhow::Result;
use std::path::{Path, PathBuf};

/// Materialized `pi-grok-review` extension bundle in a content-addressed cache
/// directory so `jiti`'s `fsCache` hits across launches.
pub(super) struct ReviewExtension {
    source_path: PathBuf,
}

impl ReviewExtension {
    pub(super) fn path(&self) -> &Path {
        &self.source_path
    }
}

/// Materialize the standalone TS `/review-session` and `/review-message`
/// extension along with its shared `pi-grok-tui-kit` primitives and
/// `pi-grok-mentions/file-data.ts` preview helper.
pub(super) fn write_review_extension() -> Result<ReviewExtension> {
    let root = super::tui_kit::materialize_cached_tui_bundle(
        "pi-grok-review",
        &[
            (
                "pi-grok-mentions/file-data.ts",
                include_str!("../../../../../../extensions/pi-grok-mentions/file-data.ts"),
            ),
            (
                "pi-grok-mentions/session-data.ts",
                include_str!("../../../../../../extensions/pi-grok-mentions/session-data.ts"),
            ),
            (
                "pi-grok-mentions/session-lite-db.ts",
                include_str!("../../../../../../extensions/pi-grok-mentions/session-lite-db.ts"),
            ),
            (
                "pi-grok-mentions/skill-data.ts",
                include_str!("../../../../../../extensions/pi-grok-mentions/skill-data.ts"),
            ),
            (
                "pi-grok-review/index.ts",
                include_str!("../../../../../../extensions/pi-grok-review/index.ts"),
            ),
            (
                "pi-grok-review/review-data.ts",
                include_str!("../../../../../../extensions/pi-grok-review/review-data.ts"),
            ),
            (
                "pi-grok-review/review-tui.ts",
                include_str!("../../../../../../extensions/pi-grok-review/review-tui.ts"),
            ),
            (
                "pi-grok-review/trace-data.ts",
                include_str!("../../../../../../extensions/pi-grok-review/trace-data.ts"),
            ),
            (
                "pi-grok-review/trace-tui.ts",
                include_str!("../../../../../../extensions/pi-grok-review/trace-tui.ts"),
            ),
            (
                "pi-grok-review/tool-render-tui.ts",
                include_str!("../../../../../../extensions/pi-grok-review/tool-render-tui.ts"),
            ),
        ],
    )?;

    Ok(ReviewExtension {
        source_path: root.join("pi-grok-review/index.ts"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn review_extension_materializes_with_shared_file_lib() {
        let ext = write_review_extension().expect("write");
        let root = ext.path().parent().unwrap().parent().unwrap();
        let index = fs::read_to_string(ext.path()).expect("read index.ts");
        let shared = fs::read_to_string(root.join("pi-grok-mentions/file-data.ts"))
            .expect("read shared file-data.ts");
        let session_data = fs::read_to_string(root.join("pi-grok-mentions/session-data.ts"))
            .expect("read shared session-data.ts");
        let skill_data = fs::read_to_string(root.join("pi-grok-mentions/skill-data.ts"))
            .expect("read shared skill-data.ts");
        let kit = fs::read_to_string(root.join("pi-grok-tui-kit/frame.ts"))
            .expect("read pi-grok-tui-kit/frame.ts");
        let trace_data = fs::read_to_string(root.join("pi-grok-review/trace-data.ts"))
            .expect("read pi-grok-review/trace-data.ts");
        let trace_tui = fs::read_to_string(root.join("pi-grok-review/trace-tui.ts"))
            .expect("read pi-grok-review/trace-tui.ts");
        let tool_render_tui = fs::read_to_string(root.join("pi-grok-review/tool-render-tui.ts"))
            .expect("read pi-grok-review/tool-render-tui.ts");
        assert!(index.contains("__pi_review_session"));
        assert!(index.contains("__pi_review_message"));
        assert!(index.contains("__pi_trace"));
        assert!(index.contains("__pi_tool_render"));
        assert!(trace_data.contains("parseSessionTraceJsonl"));
        assert!(trace_tui.contains("createTraceTui"));
        assert!(tool_render_tui.contains("createToolRenderTui"));
        assert!(shared.contains("loadFileOrDirPreview"));
        assert!(session_data.contains("formatTimestamp"));
        assert!(skill_data.contains("formatFileSize"));
        assert!(kit.contains("renderSplitOverlayFrame"));
    }
}
