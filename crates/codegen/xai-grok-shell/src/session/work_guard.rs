//! Small busy-work counter guard shared by Stock SessionActor and Workflow.
pub(crate) struct WorkGuard(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl WorkGuard {
    pub(crate) fn new(active_work: std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Self {
        active_work.fetch_add(1, std::sync::atomic::Ordering::Release);
        Self(active_work)
    }
}

impl Drop for WorkGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Release);
    }
}
