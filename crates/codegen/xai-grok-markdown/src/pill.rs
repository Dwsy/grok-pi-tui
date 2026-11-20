//! Status pills for assistant replies: `[[Done]]` reads as a rounded chip.
//!
//! A pill is written as `[[Word]]` beside the thing it describes. Its word decides the tone, the
//! icon and, for a known word, the canonical spelling: `[[Completed]]` reads as `Done`.
//!
//! This module is the single source of truth shared by the renderer and by the reply-writing rules
//! given to the agent (see `grok-pi`'s emphasis extension). Keeping the tables here means a word
//! that a rule teaches always has a tone to draw.
//!
//! # Why the brackets are hidden rather than removed
//!
//! The renderer's `source_map` and the streaming checkpoint both index into the source text, and
//! `Transform` substitutions must preserve byte length. Rather than shorten the text, this module
//! hides the two-character `[[` / `]]` pairs with an `Effects::HIDDEN` highlight at their exact
//! byte ranges. Source offsets stay valid, so streaming tail re-renders and selection copy keep
//! working unchanged.

use anstyle::Style;

use crate::style::MarkdownStyle;

/// The colour group a pill word belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillTone {
    /// Something is wrong.
    Error,
    /// Look here: an ask, a decision, or an unchecked claim.
    Warning,
    /// Settled.
    Success,
    /// A consequence, shown louder than the panel tones.
    Danger,
    /// The kind of work the reply did.
    Mode,
    /// Who owns the thing described.
    Owner,
    /// Any other label.
    Neutral,
}

impl PillTone {
    /// Every tone, in the order the guidance lists them.
    pub const ALL: [Self; 7] = [
        Self::Danger,
        Self::Error,
        Self::Warning,
        Self::Success,
        Self::Mode,
        Self::Owner,
        Self::Neutral,
    ];

    /// The tone's style name, used by tests and by the guidance generator.
    pub fn key(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Success => "success",
            Self::Danger => "danger",
            Self::Mode => "mode",
            Self::Owner => "owner",
            Self::Neutral => "neutral",
        }
    }

    /// The style used to draw a pill of this tone.
    pub fn style(self, ms: &MarkdownStyle) -> Style {
        match self {
            Self::Error => ms.pill_error,
            Self::Warning => ms.pill_warning,
            Self::Success => ms.pill_success,
            Self::Danger => ms.pill_danger,
            Self::Mode => ms.pill_mode,
            Self::Owner => ms.pill_owner,
            Self::Neutral => ms.pill_neutral,
        }
    }
}

/// The words agents are told to write (`core`) and the words accepted without being taught (`also`).
pub struct Vocabulary {
    pub core: &'static [&'static str],
    pub also: &'static [&'static str],
}

/// Each tone's words. `core` are taught to the agent; `also` are accepted and coloured the same.
pub const VOCABULARY: [(PillTone, Vocabulary); 6] = [
    (
        PillTone::Error,
        Vocabulary {
            core: &["Failed", "Blocked", "Broken", "Unsafe"],
            also: &["fail", "failing", "error", "rejected", "down"],
        },
    ),
    (
        PillTone::Warning,
        Vocabulary {
            core: &[
                "Needs you",
                "Decision",
                "Unverified",
                "Partial",
                "Skipped",
                "In progress",
                "Pending",
                "At risk",
            ],
            also: &[
                "needs review",
                "waiting",
                "review",
                "todo",
                "stale",
                "draft",
            ],
        },
    ),
    (
        PillTone::Success,
        Vocabulary {
            core: &[
                "Done",
                "Passed",
                "Observed",
                "Fixed",
                "Live",
                "Recommended",
                "Keep",
            ],
            also: &[
                "pass",
                "ready",
                "merged",
                "safe",
                "verified",
                "shipped",
                "ok",
                "approved",
                "complete",
                "completed",
                "finished",
                "healthy",
            ],
        },
    ),
    (
        PillTone::Danger,
        Vocabulary {
            core: &["Costs money", "Destructive", "Irreversible"],
            also: &[],
        },
    ),
    (
        PillTone::Mode,
        Vocabulary {
            core: &["Brainstorm", "Investigate", "Preview", "Implement", "Qualify"],
            also: &[],
        },
    ),
    (
        PillTone::Owner,
        Vocabulary {
            core: &["Owned", "Shared", "Foreign"],
            also: &[],
        },
    ),
];

/// Words that are soft blue like any unknown label, but have an icon and a place in the guidance.
pub const CONTEXT_WORDS: &[&str] = &["Inferred", "New", "Changed", "Optional"];

/// Nerd Font glyphs (Font Awesome set). One cell wide in every terminal.
const CHECK: &str = "\u{f00c}";
const CROSS: &str = "\u{f00d}";
const CLOCK: &str = "\u{f017}";

/// The icon each known pill word carries. Words without an entry stay text-only.
pub fn icon_of(key: &str) -> Option<&'static str> {
    Some(match key {
        "done" | "passed" | "pass" | "complete" | "completed" | "ok" | "verified" | "ready" => CHECK,
        "observed" => "\u{f06e}",
        "recommended" => "\u{f005}",
        "fixed" | "implement" => "\u{f0ad}",
        "live" | "shipped" => "\u{f135}",
        "approved" => "\u{f164}",
        "failed" | "fail" | "failing" | "error" | "rejected" => CROSS,
        "blocked" => "\u{f05e}",
        "broken" => "\u{f127}",
        "unsafe" | "safe" | "guard" => "\u{f132}",
        "down" => "\u{f063}",
        "needs you" | "owned" => "\u{f007}",
        "decision" | "merged" => "\u{f126}",
        "unverified" => "\u{f128}",
        "at risk" | "risk" | "limits" => "\u{f071}",
        "in progress" => "\u{f110}",
        "pending" | "waiting" => CLOCK,
        "partial" => "\u{f042}",
        "skipped" => "\u{f04e}",
        "inferred" | "brainstorm" | "ideas" => "\u{f0eb}",
        "new" => "\u{f067}",
        "changed" => "\u{f040}",
        "optional" => "\u{f10c}",
        "costs money" | "cost" => "\u{f155}",
        "destructive" => "\u{f1f8}",
        "irreversible" | "foreign" => "\u{f023}",
        "review" | "preview" => "\u{f06e}",
        "stale" => "\u{f1da}",
        "draft" => "\u{f044}",
        "investigate" | "finding" => "\u{f002}",
        "qualify" => "\u{f0c3}",
        "shared" => "\u{f0c0}",
        "keep" => "\u{f08d}",
        "healthy" => "\u{f004}",
        "todo" | "plan" => "\u{f0ae}",
        _ => return None,
    })
}

/// Words that display as one canonical word, so the same state always reads the same.
pub fn synonym_of(key: &str) -> Option<&'static str> {
    Some(match key {
        "complete" | "completed" | "ok" | "finished" => "Done",
        "pass" => "Passed",
        "fail" | "failing" | "error" => "Failed",
        "waiting" => "Pending",
        "needs review" => "Review",
        _ => return None,
    })
}

/// A pill label read as its word: a known word may be followed by detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PillLabel {
    /// The icon to draw, when the word has one.
    pub icon: Option<&'static str>,
    /// What to draw: the canonical word plus any detail the author wrote.
    pub word: String,
    /// The lowercase lookup key the tone tables match on.
    pub key: String,
}

/// The longest label a pill accepts, in characters. Longer labels are left as typed.
pub const MAX_LABEL_CHARS: usize = 32;

/// Read a pill label as `{icon, word, key}`.
///
/// A known word may be followed by detail: `Passed 12/12`, `Failed: 3 tests`. A longer word that
/// merely starts with a known one (`Donezo`) is not that word.
pub fn split_label(label: &str) -> PillLabel {
    let text = label.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = text.to_lowercase();
    let known = known_keys()
        .iter()
        .filter(|k| {
            lower == **k
                || (lower.starts_with(*k)
                    && matches!(lower.as_bytes().get(k.len()), Some(b' ' | b':')))
        })
        // `known_keys` is longest-first, so the first match is the longest.
        .max_by_key(|k| k.len())
        .map(String::as_str);
    let canonical = known.and_then(synonym_of);
    let word = match (canonical, known) {
        (Some(canonical), Some(k)) => format!("{canonical}{}", &text[k.len()..]),
        _ => text.clone(),
    };
    let key = canonical.unwrap_or(known.unwrap_or("")).to_lowercase();
    PillLabel {
        icon: icon_of(&key),
        word,
        key,
    }
}

/// The tone a pill label is drawn in.
pub fn tone_of(label: &str) -> PillTone {
    let key = split_label(label).key;
    for (tone, words) in VOCABULARY.iter() {
        if words
            .core
            .iter()
            .chain(words.also.iter())
            .any(|w| w.to_lowercase() == key)
        {
            return *tone;
        }
    }
    PillTone::Neutral
}

/// Every lookup key, longest first so a two-word label beats its first word.
fn known_keys() -> &'static [String] {
    use std::sync::LazyLock;
    static KEYS: LazyLock<Vec<String>> = LazyLock::new(|| {
        let mut keys: Vec<String> = VOCABULARY
            .iter()
            .flat_map(|(_, v)| v.core.iter().chain(v.also.iter()))
            .chain(CONTEXT_WORDS.iter())
            .map(|w| w.to_lowercase())
            .collect();
        keys.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        keys.dedup();
        keys
    });
    &KEYS
}

/// A `[[label]]` found in the source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// Byte range of `[[`.
    pub open: std::ops::Range<usize>,
    /// Byte range of the label between the brackets.
    pub label: std::ops::Range<usize>,
    /// Byte range of `]]`.
    pub close: std::ops::Range<usize>,
}

/// Find every `[[label]]` in `text` that starts inside `range`.
///
/// The scan is bounded by `range` so a streaming chunk only reports pills it fully contains; a
/// pill split across two chunks is picked up by the render that follows once both halves arrive.
pub fn scan(text: &str, range: std::ops::Range<usize>) -> Vec<Span> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = range.start;
    let end = range.end.min(bytes.len());
    while i + 1 < end {
        if bytes[i] != b'[' || bytes[i + 1] != b'[' {
            i += 1;
            continue;
        }
        let label_start = i + 2;
        let mut j = label_start;
        let mut found = None;
        while j + 1 < end {
            match bytes[j] {
                b'\n' => break,
                b'[' => break,
                b']' if bytes[j + 1] == b']' => {
                    found = Some(j);
                    break;
                }
                _ => j += 1,
            }
        }
        let Some(label_end) = found else {
            i += 1;
            continue;
        };
        let label = &text[label_start..label_end];
        // Leading whitespace is not part of the word; a pill with no word is left as typed.
        let trimmed = label.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_LABEL_CHARS {
            i = label_end + 2;
            continue;
        }
        out.push(Span {
            open: i..label_start,
            label: label_start..label_end,
            close: label_end..label_end + 2,
        });
        i = label_end + 2;
    }
    out
}

/// A pair of brackets blanked in place, so the pill never shows its syntax.
///
/// A `Transform` rather than a hidden highlight: the enclosing `Event::Text` carries `ms.text`
/// over its whole range, and `all_hidden` only skips a run when *every* style on it is hidden, so
/// a hidden highlight can never win against the text style it sits inside.
///
/// The replacement must occupy the same bytes as `[[`/`]]` so every source offset still describes
/// the same position — `render_ansi` copies forced transforms into a byte array and the streaming
/// checkpoint indexes the source. Two ASCII spaces do that without shifting anything, and because
/// the brackets carry the chip's own style (see `scan_pills`) they draw as the chip's shoulders
/// rather than as a gap in the text.
pub(crate) fn blank_syntax(syntax: &std::ops::Range<usize>) -> crate::buffers::Transform {
    debug_assert_eq!(
        syntax.end - syntax.start,
        2,
        "a bracket pair is two bytes"
    );
    crate::buffers::Transform {
        range: syntax.clone(),
        to: "  ".to_string(),
        force: true,
    }
}

/// A pill label rewritten to its canonical word, so `[[Completed]]` reads as `Done`.
///
/// Rewriting is only possible when the canonical word occupies exactly the same bytes as the
/// label: the renderer maps source offsets straight through, so a longer or shorter label would
/// shift every span after it. Synonyms that cannot be rewritten keep their own spelling and still
/// get their tone and icon from [`split_label`]; the colour is what carries the meaning, and the
/// vocabulary stays whole either way.
///
/// The padding trick is deliberately not used here. A byte-for-byte rewrite is easy to reason
/// about and cannot leave a visible gap, while zero-width padding produces odd-length labels that
/// are hard to account for in a test.
pub(crate) fn canonicalize(
    label: &str,
    range: &std::ops::Range<usize>,
) -> Option<crate::buffers::Transform> {
    let spaced = label.split_whitespace().collect::<Vec<_>>().join(" ");
    let read = split_label(&spaced);
    if read.word == spaced || read.word.len() != spaced.len() {
        return None;
    }
    Some(crate::buffers::Transform {
        range: range.clone(),
        to: read.word,
        force: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_follows_the_word() {
        assert_eq!(tone_of("Failed"), PillTone::Error);
        assert_eq!(tone_of("Blocked"), PillTone::Error);
        assert_eq!(tone_of("Needs you"), PillTone::Warning);
        assert_eq!(tone_of("Done"), PillTone::Success);
        assert_eq!(tone_of("Costs money"), PillTone::Danger);
        assert_eq!(tone_of("Implement"), PillTone::Mode);
        assert_eq!(tone_of("Shared"), PillTone::Owner);
        assert_eq!(tone_of("Inferred"), PillTone::Neutral);
        assert_eq!(tone_of("Anything else"), PillTone::Neutral);
    }

    #[test]
    fn case_and_spacing_do_not_matter() {
        assert_eq!(tone_of("  needs   YOU "), PillTone::Warning);
    }

    #[test]
    fn a_known_word_may_carry_detail() {
        assert_eq!(tone_of("Passed 12/12"), PillTone::Success);
        assert_eq!(tone_of("Failed: 3 tests"), PillTone::Error);
        assert_eq!(split_label("Failed: 3 tests").word, "Failed: 3 tests");
    }

    #[test]
    fn a_longer_word_is_not_a_known_one() {
        assert_eq!(tone_of("Donezo"), PillTone::Neutral);
    }

    #[test]
    fn synonyms_display_as_one_word_and_keep_their_detail() {
        assert_eq!(split_label("Completed").word, "Done");
        assert_eq!(split_label("Fail: 3 tests").word, "Failed: 3 tests");
        assert_eq!(split_label("OK").word, "Done");
    }

    #[test]
    fn a_known_word_gets_its_icon() {
        assert_eq!(split_label("Done").icon, Some(CHECK));
        assert_eq!(split_label("Failed").icon, Some(CROSS));
        assert_eq!(split_label("Anything else").icon, None);
    }

    #[test]
    fn every_core_word_has_a_tone() {
        for (tone, words) in VOCABULARY.iter() {
            for word in words.core.iter().chain(words.also.iter()) {
                assert_eq!(tone_of(word), *tone, "{word} should be {tone:?}");
            }
        }
        for word in CONTEXT_WORDS {
            assert_eq!(tone_of(word), PillTone::Neutral, "{word}");
        }
    }

    #[test]
    fn scan_finds_each_pill_and_its_parts() {
        let text = "a [[Done]] b [[Failed: 3 tests]] c";
        let spans = scan(text, 0..text.len());
        assert_eq!(spans.len(), 2);
        assert_eq!(&text[spans[0].label.clone()], "Done");
        assert_eq!(&text[spans[0].open.clone()], "[[");
        assert_eq!(&text[spans[0].close.clone()], "]]");
        assert_eq!(&text[spans[1].label.clone()], "Failed: 3 tests");
    }

    #[test]
    fn scan_leaves_what_is_not_a_pill() {
        for text in [
            "[not a link]", // single brackets
            "[[unclosed",   // no closing pair
            "[[a\nb]]",     // a pill never spans lines
            "[[   ]]",      // no word
            "",
        ] {
            assert!(
                scan(text, 0..text.len()).is_empty(),
                "should not scan a pill out of {text:?}"
            );
        }
    }

    #[test]
    fn scan_takes_the_inner_pill_when_brackets_nest() {
        // An outer `[[` whose label opens another `[` cannot close, so the scan resumes inside it.
        let text = "[[nested [[x]]";
        let spans = scan(text, 0..text.len());
        assert_eq!(spans.len(), 1);
        assert_eq!(&text[spans[0].label.clone()], "x");
    }

    #[test]
    fn scan_rejects_labels_over_the_limit() {
        let long = format!("[[{}]]", "x".repeat(MAX_LABEL_CHARS + 1));
        assert!(scan(&long, 0..long.len()).is_empty());
        let at_limit = format!("[[{}]]", "x".repeat(MAX_LABEL_CHARS));
        assert_eq!(scan(&at_limit, 0..at_limit.len()).len(), 1);
    }

    #[test]
    fn scan_is_bounded_by_the_range() {
        let text = "[[Done]]";
        // A range clipped to the opening brackets cannot contain the closing pair.
        assert!(scan(text, 0..4).is_empty());
        assert_eq!(scan(text, 0..text.len()).len(), 1);
    }
}
