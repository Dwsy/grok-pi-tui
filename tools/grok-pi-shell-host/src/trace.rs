use bstr::ByteSlice;
use omp_shell::bash::event::NodeKind;
use omp_shell::bash::trace::{
    Node as OmpNode, Snapshot as OmpSnapshot, State as OmpState, Unit as OmpUnit,
};

use crate::protocol::{
    MAX_NODE_DISPLAY_CHARS, MAX_TRACE_JSON_BYTES, MAX_TRACE_NODES, ShellTraceNode,
    ShellTraceNodeState, ShellTraceSummary, TRACE_SCHEMA_VERSION,
};

/// Project an OMP `RunTrace::snapshot()` into a bounded, redacted `ShellTraceSummary`.
///
/// Security / redaction invariants:
/// - Never reads `Node::env` (environment assignment values)
/// - Never reads `Node::argv` (expanded argument values)
/// - Never reads `Node::pid` (host PIDs)
/// - Never reads `Snapshot::history` (unbounded output/event journal)
pub fn project_trace(snapshot: &OmpSnapshot, final_exit_code: Option<i32>) -> ShellTraceSummary {
    let total_duration_ms = snapshot.t_ns / 1_000_000;
    let mut nodes: Vec<ShellTraceNode> = Vec::new();
    let mut truncated = false;
    let mut next_id: u32 = 0;

    'units: for unit in &snapshot.units {
        let unit_src = resolve_unit_source(snapshot.script.as_slice(), unit);
        // Map local unit node index -> global ShellTraceNode id
        let mut local_to_global: Vec<Option<u32>> = vec![None; unit.nodes.len()];

        for (idx, omp_node) in unit.nodes.iter().enumerate() {
            if nodes.len() >= MAX_TRACE_NODES {
                truncated = true;
                break 'units;
            }

            let command = extract_command_label(unit_src, omp_node.lo, omp_node.hi);
            let parent_id = omp_node
                .p
                .and_then(|p| local_to_global.get(p as usize).copied().flatten());
            let depth = compute_depth(&unit.nodes, idx);

            let id = next_id;
            next_id = next_id.saturating_add(1);
            local_to_global[idx] = Some(id);

            nodes.push(ShellTraceNode {
                id,
                parent_id,
                depth,
                kind: node_kind_str(omp_node.k).to_string(),
                command,
                state: map_state(omp_node.state),
                exit_code: omp_node.st,
                duration_ms: omp_node.ns / 1_000_000,
                runs: omp_node.runs.max(1),
                background: omp_node.bg,
            });
        }
    }

    let mut summary = ShellTraceSummary {
        version: TRACE_SCHEMA_VERSION,
        total_duration_ms,
        exit_code: final_exit_code.or(snapshot.status),
        truncated,
        nodes,
    };

    enforce_json_byte_budget(&mut summary, MAX_TRACE_JSON_BYTES);
    summary
}

fn resolve_unit_source<'a>(script: &'a [u8], unit: &'a OmpUnit) -> &'a [u8] {
    if let Some(src) = &unit.source {
        return src.as_slice();
    }
    if let Some(at) = unit.at {
        let start = (at as usize).min(script.len());
        let end = start.saturating_add(unit.bytes as usize).min(script.len());
        return &script[start..end];
    }
    script
}

fn extract_command_label(unit_src: &[u8], lo: u32, hi: u32) -> String {
    let start = (lo as usize).min(unit_src.len());
    let end = (hi as usize).max(start).min(unit_src.len());
    let slice = &unit_src[start..end];
    let lossy = slice.to_str_lossy();
    let normalized = collapse_whitespace(lossy.trim());
    truncate_chars(&normalized, MAX_NODE_DISPLAY_CHARS)
}

fn collapse_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len().min(MAX_NODE_DISPLAY_CHARS + 8));
    let mut prev_ws = false;
    for ch in input.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    out
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let mut count = 0usize;
    for (byte_idx, _) in input.char_indices() {
        if count == max_chars.saturating_sub(3) {
            if input.chars().count() > max_chars {
                let mut out = input[..byte_idx].to_string();
                out.push_str("...");
                return out;
            }
            return input.to_string();
        }
        count += 1;
    }
    input.to_string()
}

fn compute_depth(nodes: &[OmpNode], idx: usize) -> u8 {
    let mut depth: u8 = 0;
    let mut cur = nodes.get(idx).and_then(|n| n.p);
    let mut hops = 0usize;
    while let Some(parent_idx) = cur {
        depth = depth.saturating_add(1);
        hops += 1;
        if hops >= nodes.len() {
            break;
        }
        cur = nodes.get(parent_idx as usize).and_then(|n| n.p);
    }
    depth
}

fn map_state(state: OmpState) -> ShellTraceNodeState {
    match state {
        OmpState::Pending => ShellTraceNodeState::Pending,
        OmpState::Running => ShellTraceNodeState::Running,
        OmpState::Ok => ShellTraceNodeState::Ok,
        OmpState::Fail => ShellTraceNodeState::Fail,
        OmpState::Skipped => ShellTraceNodeState::Skipped,
    }
}

const fn node_kind_str(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::AndOr => "and_or",
        NodeKind::Pipeline => "pipeline",
        NodeKind::Simple => "simple",
        NodeKind::Brace => "brace",
        NodeKind::Subshell => "subshell",
        NodeKind::If => "if",
        NodeKind::While => "while",
        NodeKind::Until => "until",
        NodeKind::For => "for",
        NodeKind::ArithFor => "arith_for",
        NodeKind::Select => "select",
        NodeKind::Case => "case",
        NodeKind::Arith => "arith",
        NodeKind::Cond => "cond",
        NodeKind::FuncDef => "func_def",
        NodeKind::Coproc => "coproc",
        NodeKind::Assign => "assign",
        NodeKind::Word => "word",
    }
}

fn enforce_json_byte_budget(summary: &mut ShellTraceSummary, max_bytes: usize) {
    while let Ok(encoded) = serde_json::to_vec(summary) {
        if encoded.len() <= max_bytes || summary.nodes.is_empty() {
            break;
        }
        summary.nodes.pop();
        summary.truncated = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bstr::BString;
    use omp_shell::bash::trace::Diag;

    fn dummy_node(
        k: NodeKind,
        lo: u32,
        hi: u32,
        p: Option<u32>,
        state: OmpState,
        ns: u64,
    ) -> OmpNode {
        let mut node = OmpNode {
            k,
            lo,
            hi,
            p,
            state,
            runs: 1,
            live: 0,
            st: if matches!(state, OmpState::Fail) {
                Some(1)
            } else {
                Some(0)
            },
            fails: u32::from(matches!(state, OmpState::Fail)),
            ns,
            at: None,
            argv: vec![BString::from("SECRET_ARG_VALUE")],
            argc: Some(1),
            env: vec![(
                BString::from("API_KEY"),
                BString::from("super_secret_token"),
            )],
            res: None,
            redirs: Vec::new(),
            pipe: Vec::new(),
            iter: None,
            iters: None,
            taken: None,
            bg: false,
            pid: Some(99999),
            end_ns: None,
            end_seq: None,
        };
        if matches!(state, OmpState::Skipped) {
            node.st = None;
            node.runs = 0;
        }
        node
    }

    #[test]
    fn project_trace_redacts_secrets_and_preserves_hierarchy() {
        let script = BString::from("echo hi && false");
        let snapshot = OmpSnapshot {
            v: 1,
            seq: 3,
            status: Some(1),
            root_vpid: None,
            t_ns: 15_000_000,
            script: script.clone(),
            units: vec![OmpUnit {
                at: Some(0),
                line: 1,
                bytes: script.len() as u32,
                source: None,
                nodes: vec![
                    dummy_node(NodeKind::AndOr, 0, 16, None, OmpState::Fail, 15_000_000),
                    dummy_node(NodeKind::Simple, 0, 7, Some(0), OmpState::Ok, 4_000_000),
                    dummy_node(
                        NodeKind::Simple,
                        11,
                        16,
                        Some(0),
                        OmpState::Fail,
                        11_000_000,
                    ),
                ],
                syntax: None,
                projection: 0,
                callout: false,
            }],
            diags: Vec::<Diag>::new(),
            history: Vec::new(),
            history_truncated: false,
        };

        let summary = project_trace(&snapshot, Some(1));
        assert_eq!(summary.version, TRACE_SCHEMA_VERSION);
        assert_eq!(summary.total_duration_ms, 15);
        assert_eq!(summary.exit_code, Some(1));
        assert!(!summary.truncated);
        assert_eq!(summary.nodes.len(), 3);
        assert_eq!(summary.nodes[0].depth, 0);
        assert_eq!(summary.nodes[1].depth, 1);
        assert_eq!(summary.nodes[1].parent_id, Some(0));
        assert_eq!(summary.nodes[1].command, "echo hi");
        assert_eq!(summary.nodes[2].state, ShellTraceNodeState::Fail);

        let json = serde_json::to_string(&summary).unwrap();
        assert!(!json.contains("super_secret_token"));
        assert!(!json.contains("SECRET_ARG_VALUE"));
        assert!(!json.contains("99999"));
    }

    #[test]
    fn project_trace_truncates_over_node_limit() {
        let script = BString::from("echo ok");
        let mut nodes = Vec::new();
        for _ in 0..(MAX_TRACE_NODES + 10) {
            nodes.push(dummy_node(
                NodeKind::Simple,
                0,
                7,
                None,
                OmpState::Ok,
                1_000_000,
            ));
        }
        let snapshot = OmpSnapshot {
            v: 1,
            seq: 1,
            status: Some(0),
            root_vpid: None,
            t_ns: 1_000_000,
            script,
            units: vec![OmpUnit {
                at: Some(0),
                line: 1,
                bytes: 7,
                source: None,
                nodes,
                syntax: None,
                projection: 0,
                callout: false,
            }],
            diags: Vec::new(),
            history: Vec::new(),
            history_truncated: false,
        };

        let summary = project_trace(&snapshot, Some(0));
        assert!(summary.truncated);
        assert_eq!(summary.nodes.len(), MAX_TRACE_NODES);
    }
}
