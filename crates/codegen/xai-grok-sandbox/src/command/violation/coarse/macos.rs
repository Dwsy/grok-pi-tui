//! macOS shapes for the coarse channel: Seatbelt's `deny(1)` report line, `Sandbox: <proc>(<pid>)
//! deny(1) <operation> <target>` (the unified-log shape). The
//! operation says what was refused, so the marker carries it; the bare word `sandbox` is never a marker (a
//! failing `cargo test` printing its target directory, Chromium's `--no-sandbox` banner and
//! Deno's `(deno sandbox)` note are ordinary output). The wrapper's own diagnostics
//! (`sandbox-exec: …: Operation not permitted`) carry the shared errno marker already.

use crate::command::violation::coarse::Marker;

const DENY_REPORT: &str = "deny(1) ";

/// The `deny(1)` report on `lower` (an ASCII-lowercased line): the marker's byte offset and what
/// the operation says about the access. `file-write-*` is a write, `file-read-*` a read, any other
/// filesystem operation (`file-ioctl`, `file-link`) is decided by the policy. A `network-*` or
/// `system-*` operation names no path and is read by the network and capability rules instead.
pub(super) fn deny_report(lower: &str) -> Option<(usize, Marker)> {
    let at = lower.find(DENY_REPORT)?;
    let operation = lower
        .get(at + DENY_REPORT.len()..)?
        .split_whitespace()
        .next()?;
    let marker = if operation.starts_with("file-write") {
        Marker::Write
    } else if operation.starts_with("file-read") {
        Marker::Read
    } else if operation.starts_with("file-") {
        Marker::Denied
    } else {
        return None;
    };
    Some((at, marker))
}

/// `deny(1) network-outbound remote:*:<port>` (the address is masked in the report, only the port
/// is exact) or `deny(1) network-outbound /private/var/run/mDNSResponder` (the resolver's socket:
/// a lookup, no port). The host is never in the line.
pub(super) fn network_report(line: &str) -> Option<Option<u16>> {
    let at = line.find(DENY_REPORT)?;
    let mut words = line.get(at + DENY_REPORT.len()..)?.split_whitespace();
    if words.next()? != "network-outbound" {
        return None;
    }
    let port = words
        .next()
        .and_then(|target| target.strip_prefix("remote:"))
        .and_then(|rest| rest.rsplit(':').next())
        .and_then(|port| port.parse().ok());
    Some(port)
}
