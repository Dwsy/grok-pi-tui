use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn git_stdout(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

fn main() {
    println!("cargo:rerun-if-env-changed=GROK_VERSION");

    // Watch the git files that change on commit/checkout so the version stamp refreshes
    // Never emit a missing path: cargo treats it as always dirty and rebuilds this crate every build
    let mut watch_paths = Vec::new();
    watch_paths.extend(git_stdout(&["rev-parse", "--git-path", "HEAD"]));
    watch_paths.extend(git_stdout(&["rev-parse", "--git-path", "logs/HEAD"]));
    watch_paths.extend(git_stdout(&["rev-parse", "--git-path", "refs/tags"]));
    println!("cargo:rerun-if-env-changed=GROK_PI_VERSION");
    if let Some(head_ref) = git_stdout(&["symbolic-ref", "-q", "HEAD"]) {
        watch_paths.extend(git_stdout(&["rev-parse", "--git-path", &head_ref]));
    }
    for path in watch_paths.iter().filter(|p| Path::new(p).exists()) {
        println!("cargo:rerun-if-changed={path}");
    }

    let commit = git_stdout(&["rev-parse", "HEAD"])
        .map(|s| s.chars().take(12).collect::<String>())
        .filter(|s| s.len() == 12)
        .unwrap_or_else(|| "unknown".to_string());

    // Product version for `grok-pi --version` and update checks.
    // Prefer release env (set by CI from the v* tag), then git describe,
    // never the upstream workspace CARGO_PKG_VERSION (0.1.220-alpha.*).
    let version = product_version();

    println!("cargo:rustc-env=GROK_PI_VERSION={version}");
    println!("cargo:rustc-env=VERSION_WITH_COMMIT={version} ({commit})");
    generate_host_ui_catalog();
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

fn product_version() -> String {
    if let Ok(v) = std::env::var("GROK_PI_VERSION") {
        let v = v.trim().trim_start_matches('v').to_string();
        if !v.is_empty() {
            return v;
        }
    }
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
