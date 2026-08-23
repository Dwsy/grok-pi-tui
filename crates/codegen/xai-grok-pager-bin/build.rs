use std::{fs, path::PathBuf, process::Command};

fn main() {
    for path in ["HEAD", "refs/tags"] {
        if let Some(path) = git_path(path) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!("cargo:rerun-if-env-changed=GROK_PI_VERSION");
    generate_host_ui_catalog();

    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Product version for `grok-pi --version` and update checks.
    // Prefer release env (set by CI from the v* tag), then git describe,
    // never the upstream workspace CARGO_PKG_VERSION (0.1.220-alpha.*).
    let version = product_version();

    println!("cargo:rustc-env=GROK_PI_VERSION={version}");
    println!("cargo:rustc-env=VERSION_WITH_COMMIT={version} ({commit})");
}

fn generate_host_ui_catalog() {
    let extensions = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"))
        .join("../../../extensions");
    let mut sources = Vec::new();
    for entry in fs::read_dir(&extensions).expect("read extensions directory") {
        let entry = entry.expect("read extension entry");
        if !entry.file_type().expect("extension file type").is_dir() {
            continue;
        }
        let manifest = entry.path().join("grok-pi.json");
        if !manifest.is_file() {
            continue;
        }
        println!("cargo:rerun-if-changed={}", manifest.display());
        let name = entry.file_name().to_string_lossy().into_owned();
        let source = format!("extensions/{name}/grok-pi.json");
        let json = fs::read_to_string(&manifest).expect("read extension grok-pi.json");
        sources.push((source, json));
    }
    sources.sort_by(|a, b| a.0.cmp(&b.0));

    let mut generated = String::from("pub const BUNDLED_HOST_UI_SOURCES: &[(&str, &str)] = &[\n");
    for (source, json) in sources {
        generated.push_str(&format!("    ({source:?}, {json:?}),\n"));
    }
    generated.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("host_ui_catalog.rs");
    fs::write(out, generated).expect("write host UI catalog");
}

fn git_path(path: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-path", path])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    String::from_utf8(output.stdout)
        .ok()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
}

fn product_version() -> String {
    if let Ok(v) = std::env::var("GROK_PI_VERSION") {
        let v = v.trim().trim_start_matches('v').to_string();
        if !v.is_empty() {
            return v;
        }
    }

    // Local / non-release builds: nearest annotated or lightweight v* tag.
    if let Some(tag) = git_describe_version() {
        return tag;
    }

    "0.0.0-dev".to_string()
}

fn git_describe_version() -> Option<String> {
    let output = Command::new("git")
        .args([
            "describe",
            "--tags",
            "--match",
            "v*",
            "--abbrev=0",
            // Use build metadata (`+dirty`), not a prerelease (`-dirty`).
            // Semver ranks prereleases below the base release, which made a
            // dirty tree of vX.Y.Z look older than the published X.Y.Z tag.
            "--dirty=+dirty",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let tag = String::from_utf8(output.stdout).ok()?;
    let tag = tag.trim().trim_start_matches('v');
    if tag.is_empty() {
        None
    } else {
        Some(tag.to_string())
    }
}
