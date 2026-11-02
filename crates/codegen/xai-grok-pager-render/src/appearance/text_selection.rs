//! The `keep_text_selection` user setting (`flash` | `hold` | `word_select`).
//!
//! This is the single, unified control for scrollback text-selection behavior.
//! It governs how long an in-app selection highlight stays on screen and how text multi-clicks behave. Foldable cards use a separate single-click fold gesture.
//!
//! The compile-time default is `flash`.
//! A remote `keep_text_selection_default` soft-default (`flash` | `hold` | `word_select`) can override it at pager startup.
//! See `apply_remote_keep_text_selection_default`.

/// Scrollback text-selection behavior: highlight lifetime and text multi-click action.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TextSelection {
    /// Brief highlight on mouse-up, then clear; single-click toggles tool/summary cards, double-click toggles foldable conversation messages. Default.
    #[default]
    Flash,
    /// Stay visible until Esc/click/scroll; single-click toggles tool/summary cards, double-click toggles foldable conversation messages.
    Hold,
    /// Stay visible until dismissed; double-click selects and copies a word, triple-click a paragraph on non-card text. Foldable cards still toggle on a single click.
    /// Implies [`TextSelection::holds`].
    WordSelect,
}

impl TextSelection {
    /// Canonical persisted string (matches the settings-registry choices).
    pub const fn as_canonical(self) -> &'static str {
        match self {
            Self::Flash => "flash",
            Self::Hold => "hold",
            Self::WordSelect => "word_select",
        }
    }

    /// Parse a canonical string, returning `None` for unrecognized input.
    pub fn from_canonical(value: &str) -> Option<Self> {
        match value {
            "flash" => Some(Self::Flash),
            "hold" => Some(Self::Hold),
            "word_select" => Some(Self::WordSelect),
            _ => None,
        }
    }

    /// Whether the highlight stays until dismissed instead of clearing on a timer (`hold` or `word_select`).
    pub const fn holds(self) -> bool {
        matches!(self, Self::Hold | Self::WordSelect)
    }

    /// Whether double-click selects and copies a word (and triple-click a paragraph) on non-card text hits (`word_select` only).
    pub const fn selects_word(self) -> bool {
        matches!(self, Self::WordSelect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_round_trips() {
        for kind in [
            TextSelection::Flash,
            TextSelection::Hold,
            TextSelection::WordSelect,
        ] {
            assert_eq!(
                TextSelection::from_canonical(kind.as_canonical()),
                Some(kind)
            );
        }
    }

    #[test]
    fn default_is_flash() {
        assert_eq!(TextSelection::default(), TextSelection::Flash);
        assert_eq!(TextSelection::default().as_canonical(), "flash");
    }

    /// `word_select` always implies `holds()` (persistent highlight).
    /// It is the only mode that turns on double-click word select.
    #[test]
    fn word_select_implies_hold_and_word_select() {
        assert!(TextSelection::WordSelect.holds());
        assert!(TextSelection::WordSelect.selects_word());
        // Hold persists but leaves foldable-card clicks as fold toggles.
        assert!(TextSelection::Hold.holds());
        assert!(!TextSelection::Hold.selects_word());
        // Flash neither persists nor word-selects.
        assert!(!TextSelection::Flash.holds());
        assert!(!TextSelection::Flash.selects_word());
    }

    #[test]
    fn unknown_canonical_is_none() {
        assert_eq!(TextSelection::from_canonical("yes"), None);
        assert_eq!(TextSelection::from_canonical(""), None);
        assert_eq!(TextSelection::from_canonical("Flash"), None);
        assert_eq!(TextSelection::from_canonical("true"), None);
    }
}
