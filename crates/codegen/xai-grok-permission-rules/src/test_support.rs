/// Crate-wide lock serializing every test that mutates the process-global environment (`GROK_HOME`, `HOME`, …).
/// nextest isolates each test in its own process, but `cargo test --lib` shares ONE process across threads.
/// A per-module lock can't stop a peer test in another module clobbering `GROK_HOME` mid-test, so every env-mutating test module uses this one.
pub(crate) static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Crate-shared RAII guard for a single process env var in tests: sets (or unsets) it on construction and restores the prior value on drop.
/// Hold it together with [`ENV_TEST_LOCK`] for the test's lifetime, acquiring the lock FIRST so it drops LAST.
/// The env restore (this guard) then runs before the lock releases, so no peer test observes the temporary value.
pub(crate) struct TestEnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl TestEnvGuard {
    /// Set `key` to `val`, restoring the prior value on drop.
    pub(crate) fn set(key: &'static str, val: &std::path::Path) -> Self {
        let prev = std::env::var_os(key);
        unsafe { std::env::set_var(key, val) };
        Self { key, prev }
    }

    /// Unset `key`, restoring the prior value on drop.
    pub(crate) fn unset(key: &'static str) -> Self {
        let prev = std::env::var_os(key);
        unsafe { std::env::remove_var(key) };
        Self { key, prev }
    }
}

impl Drop for TestEnvGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(prev) => unsafe { std::env::set_var(self.key, prev) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

/// Test sink that accumulates `tracing` output into a shared buffer.
#[derive(Clone)]
struct VecWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for VecWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Serializes capture tests: `rebuild_interest_cache` is process-global, so
/// two concurrent captures can drop each other's warns.
static CAPTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run `f` while capturing WARN-level logs on this thread. `f` must be pure:
/// it runs once un-captured first to register its warn callsites.
pub(crate) fn capturing_warn_logs<T>(f: impl Fn() -> T) -> (T, String) {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Without a global sink a racing thread caches the warn callsite as never-enabled
    // and the thread-local capture misses it.
    {
        static GLOBAL_SINK: std::sync::Once = std::sync::Once::new();
        GLOBAL_SINK.call_once(|| {
            let _ = tracing::subscriber::set_global_default(
                tracing_subscriber::fmt()
                    .with_max_level(tracing::Level::WARN)
                    .with_writer(std::io::sink)
                    .finish(),
            );
        });
    }
    // Registers the warn callsites before the capture starts.
    f();
    let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let writer_buf = buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || VecWriter(writer_buf.clone()))
        .finish();
    let value = tracing::subscriber::with_default(subscriber, || {
        // Callsite interest cached before the dispatcher swap would silently
        // drop the warn from the capture.
        tracing::callsite::rebuild_interest_cache();
        f()
    });
    let logs = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    (value, logs)
}
