#![cfg(unix)]

use pretty_assertions::assert_eq;

use super::*;
use crate::test_support::{handler_name, spec};

fn store(dir: &Path, boot_id: &str) -> RegistrationStore {
    RegistrationStore::new(dir.to_path_buf(), BootId::from(boot_id))
}

#[test]
fn load_keeps_this_boot_and_cleans_up_what_is_not_a_live_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let current = store(dir.path(), "boot-a");
    current
        .save_sync(&handler_name("keep"), &spec(&["/bin/keep"], Some(200)))
        .expect("save");
    store(dir.path(), "boot-b")
        .save_sync(&handler_name("foreign"), &spec(&["/bin/foreign"], None))
        .expect("save");
    std::fs::write(dir.path().join("corrupt.json"), b"{").expect("write");
    std::fs::write(dir.path().join(".tmp-crashed"), b"partial").expect("write");
    std::fs::write(dir.path().join("README"), b"not ours").expect("write");

    let loaded = current.load_sync();

    assert_eq!(
        BTreeMap::from([(handler_name("keep"), spec(&["/bin/keep"], Some(200)))]),
        loaded
    );
    let mut remaining: Vec<String> = std::fs::read_dir(dir.path())
        .expect("read dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    remaining.sort();
    assert_eq!(vec!["README".to_owned(), "keep.json".to_owned()], remaining);
}

#[test]
fn load_trims_records_past_the_registered_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let current = store(dir.path(), "boot-a");
    for index in 0..MAX_REGISTERED_HANDLERS + 2 {
        current
            .save_sync(
                &handler_name(&format!("h{index:02}")),
                &spec(&["/bin/true"], None),
            )
            .expect("save");
    }

    let loaded = current.load_sync();

    assert_eq!(MAX_REGISTERED_HANDLERS, loaded.len());
    assert!(
        !dir.path()
            .join(format!("h{:02}.json", MAX_REGISTERED_HANDLERS))
            .exists()
    );
}

#[test]
fn remove_of_an_absent_record_succeeds() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(
        store(dir.path(), "boot-a")
            .remove_sync(&handler_name("absent"))
            .is_ok()
    );
}
