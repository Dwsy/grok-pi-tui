/// Canonical form for domain comparison: trim whitespace, strip trailing
/// slashes and dots, remove `www.` prefix, and lowercase.
pub fn normalize_domain(raw: &str) -> String {
    let s = raw.trim().trim_end_matches('/').trim_end_matches('.');
    let s = s.strip_prefix("www.").unwrap_or(s);
    s.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_www_and_trailing_dot() {
        assert_eq!(normalize_domain("www.Example.COM."), "example.com");
    }

    #[test]
    fn normalize_trims_whitespace() {
        assert_eq!(normalize_domain("  docs.rs  "), "docs.rs");
    }
}
