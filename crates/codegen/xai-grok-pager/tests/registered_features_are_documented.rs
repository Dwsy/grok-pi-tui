//! `FEATURES` is the source of truth and the operator tables are hand-maintained mirrors with no compile-time check of their own.
//! This test is that check.
//!
//! The operator tables are upstream-internal docs that this fork does not ship,
//! so they are read at run time and the check runs only when they are present
//! (for example after an upstream sync restores them).

use xai_grok_shell::agent::config::FEATURES;

/// Read an operator table, or `None` when this fork does not carry it.
fn read_if_present(relative: &str) -> Option<String> {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), relative);
    std::fs::read_to_string(path).ok()
}

#[test]
fn every_registered_feature_reaches_the_operator() {
    let enterprise = read_if_present("docs/internal/25-enterprise.md");
    let env_vars = read_if_present("docs/internal/22-environment-variables.md");
    let (Some(enterprise), Some(env_vars)) = (enterprise, env_vars) else {
        eprintln!("operator tables are absent from this fork; skipping the feature documentation check");
        return;
    };
    for spec in FEATURES {
        assert!(
            enterprise.contains(&format!("`{}`", spec.key)),
            "{} has no row in the 25-enterprise.md pinning table",
            spec.key,
        );
        assert!(
            env_vars.contains(&format!("`{}`", spec.env)),
            "{} is undocumented in 22-environment-variables.md",
            spec.env,
        );
    }
}