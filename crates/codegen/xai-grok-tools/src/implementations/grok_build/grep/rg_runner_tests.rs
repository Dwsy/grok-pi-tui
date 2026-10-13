use std::fs;
use std::time::Duration;

use tempfile::TempDir;

use super::*;
use crate::implementations::grok_build::grep::offer::GrepSource;
use crate::implementations::grok_build::grep::tests::make_grep_input;
use crate::implementations::grok_build::grep::{GrepStep, prepare_grep};
use crate::types::resources::{Cwd, Resources};
use crate::types::tool_metadata::test_ctx;

/// A cancelled tool future drops the `Child` before any wait/kill path
/// runs; the spawn config must kill rg on drop.
#[cfg(unix)]
#[tokio::test]
async fn dropping_spawned_grep_child_kills_rg() {
    let tmp = TempDir::new().unwrap();
    // Overflow the stdout pipe so rg blocks on write and stays alive until killed.
    let line = format!("needle {}\n", "x".repeat(120));
    fs::write(tmp.path().join("big.txt"), line.repeat(20_000)).unwrap();

    let mut resources = Resources::new();
    resources.insert(Cwd(tmp.path().to_path_buf()));
    let ctx = test_ctx(resources.into_shared());

    let step = prepare_grep(&ctx, &make_grep_input("needle"))
        .await
        .expect("prepare_grep");
    let ready = match step {
        GrepStep::Ready(r) => r,
        GrepStep::Early(out) => panic!("expected spawned rg, got early output: {out:?}"),
    };
    let GrepSource::Rg(rg) = ready.source else {
        panic!("expected spawned rg without a file system to offer the search");
    };
    let pid = rg.child.id().expect("child pid");

    // Hold the read end open (no EPIPE death) and drop the child mid-run.
    let RgRunner {
        child,
        stdout: stdout_pipe,
        ..
    } = rg;
    drop(child);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !xai_tty_utils::process_not_running(pid) {
        assert!(
            std::time::Instant::now() < deadline,
            "rg (pid {pid}) still running 5s after its Child was dropped — leaked"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    drop(stdout_pipe);
}
