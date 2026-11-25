use anyhow::Result;
use std::path::{Path, PathBuf};

/// Materialized Pi auth extension bundle in a content-addressed cache
/// directory so `jiti`'s `fsCache` hits across launches.
pub(super) struct AuthExtension {
    source_path: PathBuf,
}

impl AuthExtension {
    pub(super) fn source_path(&self) -> &Path {
        &self.source_path
    }
}

/// Materialize default-on Pi auth commands (`/login` / `/logout`).
///
/// Requires system Pi >= 0.99.0 (`modelRuntime.login` + Remote TUI). Every
/// authored module must be materialized here because this injector owns the
/// transitive closure of `index.ts`'s relative imports.
pub(super) fn write_auth_extension() -> Result<AuthExtension> {
    let root = super::tui_kit::materialize_cached_bundle(
        "pi-grok-auth",
        &[
            (
                "index.ts",
                include_str!("../../../../../../extensions/pi-grok-auth/index.ts"),
            ),
            (
                "shared.ts",
                include_str!("../../../../../../extensions/pi-grok-auth/shared.ts"),
            ),
            (
                "runtime.ts",
                include_str!("../../../../../../extensions/pi-grok-auth/runtime.ts"),
            ),
            (
                "providers.ts",
                include_str!("../../../../../../extensions/pi-grok-auth/providers.ts"),
            ),
            (
                "login.ts",
                include_str!("../../../../../../extensions/pi-grok-auth/login.ts"),
            ),
            (
                "logout.ts",
                include_str!("../../../../../../extensions/pi-grok-auth/logout.ts"),
            ),
        ],
    )?;
    Ok(AuthExtension {
        source_path: root.join("index.ts"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Bundle {
        index: String,
        shared: String,
        runtime: String,
        providers: String,
        login: String,
        logout: String,
    }

    fn write_bundle() -> (AuthExtension, Bundle) {
        let extension = write_auth_extension().expect("write extension");
        let dir = extension.source_path().parent().expect("source dir");
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"))
        };
        let bundle = Bundle {
            index: read("index.ts"),
            shared: read("shared.ts"),
            runtime: read("runtime.ts"),
            providers: read("providers.ts"),
            login: read("login.ts"),
            logout: read("logout.ts"),
        };
        (extension, bundle)
    }

    #[test]
    fn auth_extension_materializes_every_module_of_the_entry_closure() {
        let (extension, bundle) = write_bundle();
        assert!(bundle.index.contains("from \"./login.ts\""));
        assert!(bundle.index.contains("from \"./logout.ts\""));
        assert!(bundle.login.contains("from \"./providers.ts\""));
        assert!(bundle.login.contains("from \"./runtime.ts\""));
        assert!(bundle.login.contains("from \"./shared.ts\""));
        assert!(bundle.logout.contains("from \"./providers.ts\""));
        assert!(bundle.logout.contains("from \"./runtime.ts\""));
        assert!(bundle.logout.contains("from \"./shared.ts\""));
        assert!(bundle.providers.contains("from \"./shared.ts\""));
        assert!(bundle.runtime.contains("from \"./shared.ts\""));
        assert!(bundle.shared.contains("ModelRuntimeLike"));
        assert!(bundle.runtime.contains("loadComponents"));
        assert!(bundle.providers.contains("loginProviders"));
        assert!(bundle.login.contains("registerCommand(\"login\""));
        assert!(bundle.logout.contains("registerCommand(\"logout\""));
        assert_eq!(
            extension
                .source_path()
                .extension()
                .and_then(|value| value.to_str()),
            Some("ts")
        );
    }

    #[test]
    fn auth_extension_source_preserves_remote_tui_login_contract() {
        let extension = write_auth_extension().expect("write extension");
        let source = std::fs::read_to_string(extension.source_path()).expect("read extension");
        assert!(source.contains("registerLoginCommand"));
        assert!(source.contains("registerLogoutCommand"));
    }
}
