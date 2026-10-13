#![cfg(unix)]

use std::path::Path;
use std::sync::mpsc;

use pretty_assertions::assert_eq;

use super::*;
use crate::persist::BootId;
use crate::test_support::{BOOT_A, handler_name, spec};

fn store(dir: &Path) -> RegistrationStore {
    RegistrationStore::new(dir.to_path_buf(), BootId::from(BOOT_A))
}

/// A registry that stops waiting for its record writes at once.
fn impatient_registry(dir: &Path) -> HandlerRegistry {
    HandlerRegistry {
        persist_timeout: Duration::ZERO,
        ..HandlerRegistry::load_sync(Some(store(dir)))
    }
}

/// A runtime whose only blocking thread is parked until the returned sender fires, so every record write the
/// registry starts waits behind it and outlives its caller's timeout.
fn gated_runtime() -> (tokio::runtime::Runtime, mpsc::Sender<()>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .max_blocking_threads(1)
        .build()
        .expect("runtime");
    let (open, gate) = mpsc::channel::<()>();
    let (parked, is_parked) = mpsc::channel();
    runtime.spawn_blocking(move || {
        parked.send(()).expect("test alive");
        gate.recv().expect("gate opened");
    });
    is_parked.recv().expect("blocking thread parked");
    (runtime, open)
}

fn assert_timed_out(result: Result<(), RegistryError>) {
    match result {
        Err(RegistryError::Persist(e)) if e.kind() == io::ErrorKind::TimedOut => {}
        other => panic!("expected a persist timeout, got {other:?}"),
    }
}

#[test]
fn put_that_outlives_its_timeout_lands_in_the_store_and_the_table_together() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (runtime, open_gate) = gated_runtime();
    let registry = impatient_registry(dir.path());

    runtime.block_on(async {
        assert_timed_out(
            registry
                .put(handler_name("late"), spec(&["/bin/true"], None))
                .await,
        );
        open_gate.send(()).expect("gate");
        let _drained = registry.writes.lock().await;
    });

    let expected = BTreeMap::from([(handler_name("late"), spec(&["/bin/true"], None))]);
    assert_eq!(expected, store(dir.path()).load_sync());
    assert_eq!(
        expected,
        registry.snapshot().into_iter().collect::<BTreeMap<_, _>>()
    );
}

#[test]
fn delete_after_a_timed_out_put_waits_for_it_and_removes_it_everywhere() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (runtime, open_gate) = gated_runtime();
    let mut registry = impatient_registry(dir.path());
    runtime.block_on(async {
        assert_timed_out(
            registry
                .put(handler_name("late"), spec(&["/bin/true"], None))
                .await,
        );
    });
    registry.persist_timeout = Duration::from_secs(30);

    let late = handler_name("late");
    runtime.block_on(async {
        let delete = registry.delete(&late);
        open_gate.send(()).expect("gate");
        delete.await.expect("delete");
    });

    assert_eq!(BTreeMap::new(), store(dir.path()).load_sync());
    assert_eq!(Vec::<(HandlerName, ExecSpec)>::new(), registry.snapshot());
}
