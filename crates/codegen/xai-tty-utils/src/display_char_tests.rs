use super::is_unsafe_display_char;

#[test]
fn is_unsafe_display_char_covers_controls_and_bidi_format() {
    // Safe: ordinary printable text (incl. legitimate RTL letters).
    for c in ['a', ' ', '/', '\u{00e9}', '\u{05d0}'] {
        assert!(!is_unsafe_display_char(c), "{c:?} must be safe");
    }
    // Unsafe: C0/C1 controls plus the full bidi-control and zero-width set
    for c in [
        '\u{1b}', '\n', '\t', '\u{061C}', '\u{200B}', '\u{200F}', '\u{202E}', '\u{2066}',
        '\u{2069}', '\u{206F}', '\u{FEFF}',
    ] {
        assert!(
            is_unsafe_display_char(c),
            "{:#06x} must be unsafe",
            c as u32
        );
    }
}

/// Every invisible format character range counts, not only the bidi and zero-width ones
#[test]
fn is_unsafe_display_char_covers_every_invisible_format_range() {
    let ranges = [
        ('\u{00AD}', '\u{00AD}'),
        ('\u{061C}', '\u{061C}'),
        ('\u{180E}', '\u{180E}'),
        ('\u{2028}', '\u{2029}'),
        ('\u{FFF9}', '\u{FFFB}'),
        ('\u{13430}', '\u{1343F}'),
        ('\u{1BCA0}', '\u{1BCA3}'),
        ('\u{1D173}', '\u{1D17A}'),
        ('\u{E0001}', '\u{E0001}'),
        ('\u{E0020}', '\u{E007F}'),
    ];

    for c in ranges.into_iter().flat_map(|(first, last)| first..=last) {
        assert!(is_unsafe_display_char(c), "U+{:04X}", c as u32);
    }
}
