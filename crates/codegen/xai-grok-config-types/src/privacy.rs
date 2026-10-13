//! The account's privacy mode.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

/// The privacy mode as its protobuf number, kept as-is so a mode added later passes through unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrivacyMode(i32);

impl PrivacyMode {
    pub const UNSPECIFIED: Self = Self(0);
    pub const NO_STORAGE: Self = Self(1);
    pub const NO_TRAINING: Self = Self(2);
    pub const USAGE_DATA_TRAINING_ALLOWED: Self = Self(3);
    pub const USAGE_CODEBASE_TRAINING_ALLOWED: Self = Self(4);
}

impl From<i32> for PrivacyMode {
    fn from(number: i32) -> Self {
        Self(number)
    }
}

impl From<PrivacyMode> for i32 {
    fn from(mode: PrivacyMode) -> Self {
        mode.0
    }
}

/// One privacy mode shared by its clones: one writer sets it, and readers check it right before sending anything.
#[derive(Clone, Debug, Default)]
pub struct SharedPrivacyMode(Arc<AtomicI32>);

impl SharedPrivacyMode {
    pub fn get(&self) -> PrivacyMode {
        PrivacyMode(self.0.load(Ordering::Acquire))
    }

    pub fn set(&self, mode: PrivacyMode) {
        self.0.store(mode.0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_one_mode_that_starts_unspecified() {
        let writer = SharedPrivacyMode::default();
        let reader = writer.clone();
        assert_eq!(PrivacyMode::UNSPECIFIED, reader.get());

        writer.set(PrivacyMode::NO_TRAINING);
        assert_eq!(PrivacyMode::NO_TRAINING, reader.get());
    }
}
