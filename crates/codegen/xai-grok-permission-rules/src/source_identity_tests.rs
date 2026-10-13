use super::*;

#[test]
#[rustfmt::skip]
fn git_url_identity_folds_scheme_and_host_and_one_git_suffix() {
    let rows: [(&str, &str, &str); 5] = [
        (
            "HTTPS://Git.Corp.com/Team/Tools.git",
            "https://git.corp.com/Team/Tools",
            "scheme+host fold case; one .git strips",
        ),
        (
            "https://h.example/repo.git.git",
            "https://h.example/repo.git",
            "`repo.git.git` is a repo named `repo.git`",
        ),
        (
            "Git@GitHub.com:Org/Repo.git",
            "git@github.com:Org/Repo",
            "scp-style: user@host folds, path does not",
        ),
        ("/tmp/Marketplace", "/tmp/Marketplace", "local paths pass through untouched"),
        ("/tmp/A@b:c/mkt", "/tmp/A@b:c/mkt", "a `/` before `:` is a local path, not scp"),
    ];
    for (input, want, label) in rows {
        assert_eq!(want, normalize_git_url(input), "{label}");
    }
    assert_ne!(
        normalize_git_url("https://git.corp.com/team/tools"),
        normalize_git_url("https://git.corp.com/Team/Tools"),
        "path case is identity: a different-cased path is a different repo"
    );
}

#[test]
#[rustfmt::skip]
fn source_identity_matches_git_url_drift_but_not_local_path_suffix() {
    let rows: [(&str, &str, bool, &str); 16] = [
        ("https://github.com/org/repo.git", "https://github.com/org/repo", true, ".git suffix drift"),
        ("HTTPS://GitHub.com/org/repo", "https://github.com/org/repo.git", true, "scheme and host case drift"),
        ("git@GitHub.com:org/repo.git", "git@github.com:org/repo", true, "scp host case and .git drift"),
        ("ssh://git@GitHub.com/org/repo.git", "ssh://git@github.com/org/repo", true, "ssh URL drift"),
        ("https://github.com/org/repo", "https://github.com/org/other", false, "different repo"),
        ("https://github.com/org/repo", "https://gitlab.com/org/repo", false, "different host"),
        ("https://github.com/Org/Repo", "https://github.com/org/repo", false, "path case is identity"),
        ("git@github.com:org/repo.git", "https://github.com/org/repo.git", false, "scp and https never alias"),
        ("/tmp/mkt.git", "/tmp/mkt", false, "local paths differing by .git are different directories"),
        ("/tmp/Mkt", "/tmp/mkt", false, "local path case is kept"),
        ("/tmp/mkt", "/tmp/mkt", true, "same local path"),
        ("/tmp/github.com/org/repo", "https://github.com/org/repo", false, "local path never aliases a URL"),
        ("/srv/mkt.git", "/srv/mkt", false, "a git source written as a bare path compares as a path"),
        ("github.com:org/repo.git", "github.com:org/repo", false, "user-less scp compares exactly"),
        ("C:/mkt.git", "C:/mkt", false, "Windows drive path is local"),
        ("/tmp/a@B:c/mkt", "/tmp/a@b:c/mkt", false, "`@` before `:` in a path is not scp"),
    ];
    for (a, b, want, label) in rows {
        assert_eq!(want, is_same_source_identity(a, b), "{label}: {a} vs {b}");
        assert_eq!(want, is_same_source_identity(b, a), "{label} (reversed): {b} vs {a}");
    }
}
