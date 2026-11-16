//! MCP-qualified tool name syntax is shared wire metadata, not a transport.

use crate::MCP_TOOL_NAME_DELIMITER;

/// Parse a non-empty `server__tool` ID with one overlap-aware delimiter and valid [`xai_tool_protocol::ToolId`] syntax.
pub fn parse_mcp_qualified_name(name: &str) -> Option<(xai_tool_protocol::ToolId, &str, &str)> {
    let delimiter = MCP_TOOL_NAME_DELIMITER.as_bytes();
    // Byte windows preserve both overlapping `__` boundaries in `___`.
    let mut boundaries = name
        .as_bytes()
        .windows(delimiter.len())
        .enumerate()
        .filter_map(|(index, window)| (window == delimiter).then_some(index));
    let boundary = boundaries.next()?;
    if boundaries.next().is_some() {
        return None;
    }
    let (server, tool_with_delimiter) = name.split_at(boundary);
    let tool = &tool_with_delimiter[MCP_TOOL_NAME_DELIMITER.len()..];
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((xai_tool_protocol::ToolId::new(name).ok()?, server, tool))
}

/// Parse an MCP tool name in `server__tool` format into owned segments.
pub fn parse_mcp_tool_name(name: &str) -> Option<(String, String)> {
    parse_mcp_qualified_name(name).map(|(_, server, tool)| (server.to_owned(), tool.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_qualified_ids_match_stock_wire_contract() {
        for (raw, server, tool) in [
            ("linear__list_issues", "linear", "list_issues"),
            ("test-server__read_file", "test-server", "read_file"),
        ] {
            let (id, actual_server, actual_tool) =
                parse_mcp_qualified_name(raw).expect("valid qualified tool id");
            assert_eq!(id.as_str(), raw);
            assert_eq!((actual_server, actual_tool), (server, tool));
            assert_eq!(
                parse_mcp_tool_name(raw),
                Some((server.to_owned(), tool.to_owned()))
            );
        }
    }

    #[test]
    fn overlapping_and_malformed_names_fail_closed() {
        for raw in [
            "server__part__tool",
            "server__tool__part",
            "foo___bar",
            "foo____bar",
            "__tool",
            "server__",
            "server",
            "",
            "server__bad.tool",
        ] {
            assert!(parse_mcp_qualified_name(raw).is_none(), "{raw:?}");
        }
    }
}
