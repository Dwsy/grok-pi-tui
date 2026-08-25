use anyhow::{Context, Result};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// Materialized Pi auth extension bundle. `_source_dir` must stay alive for
/// the Pi process lifetime so relative imports resolve.
pub(super) struct AuthExtension {
    _source_dir: TempDir,
    source_path: PathBuf,
}

impl AuthExtension {
    pub(super) fn source_path(&self) -> &Path {
        &self.source_path
    }
}

fn write_source_file(dir: &Path, name: &str, source: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    let mut file =
        File::create(&path).with_context(|| format!("create Pi auth extension module {name}"))?;
    file.write_all(source.as_bytes())
        .with_context(|| format!("write Pi auth extension module {name}"))?;
    file.flush()
        .with_context(|| format!("flush Pi auth extension module {name}"))?;
    file.sync_all().ok();
    Ok(path)
}

/// Materialize default-on Pi auth commands (`/login` / `/logout`).
///
/// Requires system Pi >= 0.84.3 (`modelRuntime.login` + Remote TUI). Every
/// authored module must be materialized here because this injector owns the
/// transitive closure of `index.ts`'s relative imports.
pub(super) fn write_auth_extension() -> Result<AuthExtension> {
    let source_dir = tempfile::Builder::new()
        .prefix("pi-grok-auth-")
        .tempdir()
        .context("create Pi auth extension source directory")?;
    let source_path = write_source_file(
        source_dir.path(),
        "index.ts",
        include_str!("../../../../../../extensions/pi-grok-auth/index.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "shared.ts",
        include_str!("../../../../../../extensions/pi-grok-auth/shared.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "runtime.ts",
        include_str!("../../../../../../extensions/pi-grok-auth/runtime.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "providers.ts",
        include_str!("../../../../../../extensions/pi-grok-auth/providers.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "login.ts",
        include_str!("../../../../../../extensions/pi-grok-auth/login.ts"),
    )?;
    write_source_file(
        source_dir.path(),
        "logout.ts",
        include_str!("../../../../../../extensions/pi-grok-auth/logout.ts"),
    )?;
    Ok(AuthExtension {
        _source_dir: source_dir,
        source_path,
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
