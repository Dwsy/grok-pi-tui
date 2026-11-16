//! Wire-safe terminal task metadata shared by agent tools and terminal hosts.
//!
//! Do not depend on tool implementations to deserialize background-task
//! manifests. The stock tool backend re-exports this canonical type.

/// Distinguishes regular background bash commands from monitor tasks.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    #[default]
    Bash,
    Monitor,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_kind_wire_names_and_default_are_stable() {
        assert_eq!(TaskKind::default(), TaskKind::Bash);
        assert_eq!(serde_json::to_string(&TaskKind::Bash).unwrap(), "\"bash\"");
        assert_eq!(
            serde_json::to_string(&TaskKind::Monitor).unwrap(),
            "\"monitor\""
        );
        assert_eq!(
            serde_json::from_str::<TaskKind>("\"monitor\"").unwrap(),
            TaskKind::Monitor
        );
    }
}
