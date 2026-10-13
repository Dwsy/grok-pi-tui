//! Cancellation is checked only before a plan is persisted; after that, apply
//! runs to completion so the scope never holds a half-published batch.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::batch_dream::{BatchDreamError, Result};

#[derive(Debug, Clone, Default)]
pub struct BatchDreamControl {
    cancelled: Arc<AtomicBool>,
    deadline: Option<Instant>,
    #[cfg(test)]
    remaining_checks: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

impl BatchDreamControl {
    #[must_use]
    pub fn with_deadline(deadline: Instant) -> BatchDreamControl {
        BatchDreamControl {
            deadline: Some(deadline),
            ..BatchDreamControl::default()
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn with_check_budget(checks: usize) -> BatchDreamControl {
        BatchDreamControl {
            remaining_checks: Some(Arc::new(std::sync::atomic::AtomicUsize::new(checks))),
            ..BatchDreamControl::default()
        }
    }

    pub(crate) fn check(&self) -> Result<()> {
        #[cfg(test)]
        if let Some(checks) = &self.remaining_checks
            && checks
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_sub(1)
                })
                .is_err()
        {
            return Err(BatchDreamError::Interrupted);
        }
        if self.cancelled.load(Ordering::Acquire)
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(BatchDreamError::Interrupted);
        }
        Ok(())
    }
}
