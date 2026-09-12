use std::path::Path;

use tempfile::TempDir;
use xai_grok_config_types::MemoryMode;

use crate::storage::{MemoryStorage, SaveRememberNoteError};
use crate::v2::{MAX_MANUAL_OBSERVATION_BYTES, V2StorageError};

fn make_v2_storage(temp: &TempDir) -> MemoryStorage {
    MemoryStorage::new_for_mode(
        Path::new("/home/user/project"),
        Some(&temp.path().join("memory-v2")),
        MemoryMode::V2,
    )
}

#[test]
fn v2_sources_follow_global_and_workspace_scope() {
    let temp = TempDir::new().unwrap();
    let storage = make_v2_storage(&temp);

    assert_eq!(
        storage.classify_source(&storage.global_dir().join("topics/preference.md")),
        "global"
    );
    assert_eq!(
        storage.classify_source(
            &storage
                .global_dir()
                .join("observations/_inbox/preference.md")
        ),
        "global"
    );
    assert_eq!(
        storage.classify_source(&storage.workspace_dir().join("topics/project.md")),
        "workspace"
    );
    assert_eq!(
        storage.classify_source(
            &storage
                .workspace_dir()
                .join("observations/_inbox/project.md")
        ),
        "workspace"
    );
}

#[test]
fn legacy_non_manifest_workspace_files_remain_session_sources() {
    let temp = TempDir::new().unwrap();
    let global = temp.path().join("memory");
    let workspace = global.join("workspace");
    let storage = MemoryStorage::with_paths(global, workspace.clone());

    assert_eq!(
        storage.classify_source(&workspace.join("sessions/log.md")),
        "session"
    );
    assert_eq!(
        storage.classify_source(&workspace.join("MEMORY.md")),
        "workspace"
    );
}

#[test]
fn remember_notes_are_published_as_distinct_global_v2_observations() {
    let temp = TempDir::new().unwrap();
    let storage = make_v2_storage(&temp);

    storage
        .save_remember_note("Prefer concise explanations")
        .unwrap();
    storage
        .save_remember_note("Deployments\nRequire the staging flag")
        .unwrap();

    let inbox = storage.global_dir().join("observations/_inbox");
    let mut observations: Vec<_> = std::fs::read_dir(&inbox)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
        .collect();
    observations.sort();
    assert_eq!(observations.len(), 2);
    assert!(
        observations.iter().all(|path| !path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with('.')),
        "published observations must be visible while temporary files stay hidden"
    );
    let contents: Vec<_> = observations
        .iter()
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect();
    assert!(contents.contains(&"## Prefer concise explanations".to_string()));
    assert!(contents.contains(&"## Deployments\n\nRequire the staging flag".to_string()));
    assert!(
        !temp.path().join("memory/MEMORY.md").exists(),
        "v2 remember must not fall through to legacy storage"
    );

    let listed = storage.list_memory_files().unwrap();
    assert!(observations.iter().all(|path| listed.contains(path)));
    storage.ensure_initialized().unwrap();
    let manifest = std::fs::read_to_string(storage.global_memory_file()).unwrap();
    assert!(manifest.contains("Prefer concise explanations"));
    assert!(manifest.contains("Deployments"));
}

#[test]
fn oversized_remember_note_is_rejected_with_typed_error() {
    let temp = TempDir::new().unwrap();
    let storage = make_v2_storage(&temp);
    let oversized = "x".repeat(MAX_MANUAL_OBSERVATION_BYTES + 1);

    let error = storage.save_remember_note(&oversized).unwrap_err();
    assert!(matches!(
        error,
        SaveRememberNoteError::V2(V2StorageError::ObservationTooLarge {
            actual_bytes,
            limit_bytes: MAX_MANUAL_OBSERVATION_BYTES,
        }) if actual_bytes > MAX_MANUAL_OBSERVATION_BYTES
    ));
    assert!(
        !storage.global_dir().join("observations/_inbox").exists(),
        "oversized notes must fail before initializing or writing v2 storage"
    );
}

#[test]
fn legacy_remember_notes_keep_their_existing_uncapped_behavior() {
    let temp = TempDir::new().unwrap();
    let global = temp.path().join("memory");
    let storage = MemoryStorage::with_paths(global.clone(), global.join("workspace"));
    let large_note = "x".repeat(MAX_MANUAL_OBSERVATION_BYTES + 1);

    storage.save_remember_note(&large_note).unwrap();

    assert!(
        std::fs::read_to_string(global.join("MEMORY.md"))
            .unwrap()
            .len()
            > large_note.len()
    );
}

#[cfg(unix)]
#[test]
fn v2_listing_rejects_symlinked_source_directories() {
    use std::os::unix::fs::symlink;

    for relative in ["topics", "observations/_inbox"] {
        let temp = TempDir::new().unwrap();
        let storage = make_v2_storage(&temp);
        storage.ensure_initialized().unwrap();

        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("escaped.md"), "# Outside").unwrap();
        let directory = storage.workspace_dir().join(relative);
        std::fs::remove_dir(&directory).unwrap();
        symlink(&outside, &directory).unwrap();

        assert_eq!(
            storage.list_memory_files().unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied,
            "{relative} symlink must be rejected"
        );
    }
}

#[cfg(unix)]
#[test]
fn v2_listing_rejects_symlinked_markdown_files() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let storage = make_v2_storage(&temp);
    storage.ensure_initialized().unwrap();

    let outside = temp.path().join("outside.md");
    std::fs::write(&outside, "# Outside").unwrap();
    symlink(&outside, storage.workspace_dir().join("topics/escaped.md")).unwrap();

    assert_eq!(
        storage.list_memory_files().unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
}

#[cfg(unix)]
#[test]
fn v2_read_rejects_symlinked_workspace_root() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let storage = make_v2_storage(&temp);
    storage.ensure_initialized().unwrap();

    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let secret = outside.join("secret.md");
    std::fs::write(&secret, "# Outside").unwrap();
    std::fs::remove_dir_all(storage.workspace_dir()).unwrap();
    symlink(&outside, storage.workspace_dir()).unwrap();
    let escaped = storage.workspace_dir().join("secret.md");

    assert_eq!(
        storage.read_file(&escaped, None, None).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
}
