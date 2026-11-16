//! Small, backend-independent text bounds used by memory and tools.

/// Truncate at a UTF-8 boundary without adding an ellipsis.
pub fn truncate_str(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_truncation_preserves_boundaries() {
        assert_eq!(truncate_str("猫a", 1), "");
        assert_eq!(truncate_str("猫a", 2), "");
        assert_eq!(truncate_str("猫a", 3), "猫");
        assert_eq!(truncate_str("猫a", 4), "猫a");
        assert_eq!(truncate_str("long", 0), "");
    }
}
