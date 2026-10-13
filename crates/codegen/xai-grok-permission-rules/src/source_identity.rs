/// Folds only scheme and host case: lowercasing the path would widen an allowlist entry to other repos.
pub fn normalize_git_url(url: &str) -> String {
    let url = url.strip_suffix(".git").unwrap_or(url);
    canonical_remote(url).unwrap_or_else(|| url.to_owned())
}

/// Install records keep their install-time spelling, so git URLs compare normalized and local paths exactly.
pub fn is_same_source_identity(a: &str, b: &str) -> bool {
    a == b || (canonical_remote(a).is_some() && normalize_git_url(a) == normalize_git_url(b))
}

fn canonical_remote(url: &str) -> Option<String> {
    if let Some((scheme, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        return Some(format!(
            "{}://{}{path}",
            scheme.to_ascii_lowercase(),
            authority.to_ascii_lowercase()
        ));
    }
    // Like git, a `/` before the `:` means a local path; requiring `@` keeps `C:/mkt` local.
    let (user_host, path) = url.split_once(':')?;
    (user_host.contains('@') && !user_host.contains('/'))
        .then(|| format!("{}:{path}", user_host.to_ascii_lowercase()))
}

#[cfg(test)]
#[path = "source_identity_tests.rs"]
mod tests;
