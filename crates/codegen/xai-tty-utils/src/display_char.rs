//! Characters that are unsafe to show from untrusted text.

/// True for a character unsafe to render from untrusted or server-supplied text.
/// Controls can inject terminal escapes, invisible format characters (Unicode Cf) hide or reorder text,
/// and line separators break a row. The prepended concatenation marks are Cf too but render visibly, so they stay.
pub fn is_unsafe_display_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
        )
}

#[cfg(test)]
#[path = "display_char_tests.rs"]
mod tests;
