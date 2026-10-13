use super::error::MigrationError;

use toml::Value as TomlValue;
use toml::map::Map as TomlMap;
use xai_grok_config::McpServerConfig;

pub(super) fn merge_mcp_servers(
    table: &mut TomlMap<String, TomlValue>,
    servers: &[(&str, &McpServerConfig)],
) -> Result<usize, MigrationError> {
    let mcp = table
        .entry("mcp_servers")
        .or_insert_with(|| TomlValue::Table(TomlMap::new()));
    let mcp_table = mcp.as_table_mut().ok_or(MigrationError::NotATable {
        section: "[mcp_servers]",
    })?;

    let mut count = 0;
    for (name, config) in servers {
        if !mcp_table.contains_key(*name) {
            let serialized =
                toml::Value::try_from(*config).map_err(|source| MigrationError::McpSerialize {
                    name: (*name).to_string(),
                    source,
                })?;
            mcp_table.insert((*name).to_string(), serialized);
            count += 1;
        }
    }
    Ok(count)
}
