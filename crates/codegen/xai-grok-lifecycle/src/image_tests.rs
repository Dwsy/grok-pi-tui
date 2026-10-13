#![cfg(unix)]

use pretty_assertions::assert_eq;

use super::*;
use crate::test_support::{handler_name, spec};

fn write(dir: &Path, file_name: &str, contents: &[u8]) {
    std::fs::write(dir.join(file_name), contents).expect("write manifest");
}

#[test]
fn valid_manifests_load_in_name_order_and_invalid_ones_are_skipped() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        "b-flush.json",
        br#"{"v":1,"argv":["/bin/b"],"timeout_ms":500,"extra":true}"#,
    );
    write(
        dir.path(),
        "a-sync.json",
        br#"{"v":1,"argv":["/bin/a","now"]}"#,
    );
    write(dir.path(), "bad-json.json", b"{not json");
    write(
        dir.path(),
        "bad-version.json",
        br#"{"v":2,"argv":["/bin/c"]}"#,
    );
    write(dir.path(), "relative.json", br#"{"v":1,"argv":["bin/c"]}"#);
    write(dir.path(), "Bad_Name.json", br#"{"v":1,"argv":["/bin/c"]}"#);
    write(dir.path(), "notes.txt", b"not a manifest");
    let oversized = format!(r#"{{"v":1,"argv":["/bin/c","{}"]}}"#, "x".repeat(4096));
    write(dir.path(), "oversized.json", oversized.as_bytes());

    let loaded: Vec<(HandlerName, ExecSpec)> = load_image_handlers_sync(dir.path())
        .into_iter()
        .map(|image| (image.name, image.spec))
        .collect();

    assert_eq!(
        vec![
            (handler_name("a-sync"), spec(&["/bin/a", "now"], None)),
            (handler_name("b-flush"), spec(&["/bin/b"], Some(500))),
        ],
        loaded
    );
}

#[test]
fn missing_dir_means_no_image_handlers() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(load_image_handlers_sync(&dir.path().join("absent")).is_empty());
}

#[test]
fn image_handlers_are_capped() {
    let dir = tempfile::tempdir().expect("tempdir");
    for index in 0..MAX_IMAGE_HANDLERS + 3 {
        write(
            dir.path(),
            &format!("h{index:03}.json"),
            br#"{"v":1,"argv":["/bin/true"]}"#,
        );
    }
    let loaded = load_image_handlers_sync(dir.path());
    assert_eq!(MAX_IMAGE_HANDLERS, loaded.len());
    assert_eq!(
        Some(&handler_name("h000")),
        loaded.first().map(|image| &image.name)
    );
}

#[test]
fn manifests_past_the_candidate_cap_are_the_last_by_name_whatever_the_directory_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    for index in 0..MAX_IMAGE_CANDIDATES + 44 {
        write(
            dir.path(),
            &format!("h{index:03}.json"),
            br#"{"v":1,"argv":["/bin/true"]}"#,
        );
    }

    let names: Vec<HandlerName> = load_image_handlers_sync(dir.path())
        .into_iter()
        .map(|image| image.name)
        .collect();

    let expected: Vec<HandlerName> = (0..MAX_IMAGE_HANDLERS)
        .map(|index| handler_name(&format!("h{index:03}")))
        .collect();
    assert_eq!(expected, names);
}

#[tokio::test]
async fn any_manifest_file_reserves_its_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(dir.path(), "broken.json", b"{not json");
    let until = Instant::now() + std::time::Duration::from_secs(5);
    assert!(is_image_name(dir.path(), &handler_name("broken"), until).await);
    assert!(!is_image_name(dir.path(), &handler_name("other"), until).await);
}
