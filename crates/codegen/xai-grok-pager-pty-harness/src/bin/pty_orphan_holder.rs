//! Test fixture for the orphan-reap regression test (`tests/orphan_reap.rs`).
//!
//! Spawns one long-lived child under a [`PtyController`] — exactly the way
//! every PTY e2e test spawns the pager — prints the child PID, then blocks
//! forever. The regression test kills *this process* ungracefully
//! (SIGKILL/SIGTERM, so no Drop runs) and asserts the PTY child does not
//! survive it. `main` (a thread that lives as long as the process) does the
//! spawning, satisfying the pdeathsig spawning-thread requirement.

use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use portable_pty::PtySize;
use xai_grok_pager_pty_harness::PtyController;
use xai_grok_test_support::TestSandbox;

fn main() -> anyhow::Result<()> {
    let sandbox = TestSandbox::new();
    // Ignore SIGHUP so a well-behaved child does not mask the leak; only pdeathsig can reap it.
    // `exec` keeps one PID (SIG_IGN survives) so the liveness probe targets the process that must die with the holder.
    let controller = PtyController::spawn_in_sandbox(
        Path::new("/bin/sh"),
        PtySize {
            rows: 8,
            cols: 40,
            pixel_width: 0,
            pixel_height: 0,
        },
        &["-c", "trap '' HUP; exec sleep 600"],
        &sandbox,
        &[],
        None,
    )?;
    let pid = controller
        .child_pid()
        .ok_or_else(|| anyhow::anyhow!("PTY child has no pid"))?;
    println!("CHILD_PID={pid}");
    std::io::stdout().flush()?;

    // Hold the controller (and its child) until the test kills this process.
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}
