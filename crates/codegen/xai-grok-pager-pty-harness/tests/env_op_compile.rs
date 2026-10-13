use std::ffi::OsStr;

use xai_grok_pager_pty_harness::EnvOp;

#[test]
fn set_and_remove_operations_have_one_typed_surface() {
    let key = OsStr::new("FEATURE_FLAG");
    let value = OsStr::new("enabled");
    let operations: [EnvOp<'_>; 4] = [
        EnvOp::set("FEATURE_FLAG", "enabled"),
        EnvOp::remove("XAI_API_KEY"),
        EnvOp::set_os(key, value),
        EnvOp::remove_os(key),
    ];

    assert!(matches!(operations[0], EnvOp::Set(_, _)));
    assert!(matches!(operations[1], EnvOp::Remove(_)));
    assert!(matches!(operations[2], EnvOp::Set(_, _)));
    assert!(matches!(operations[3], EnvOp::Remove(_)));
}
