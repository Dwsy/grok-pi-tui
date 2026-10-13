use std::collections::BTreeMap;
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt as _;

use crate::command::policy::EnvPolicy;

use super::{EnvGlobs, apply_env_policy, apply_env_set_only, excluded_names};

fn names(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

fn default_policy() -> EnvPolicy {
    EnvPolicy {
        exclude_globs: EnvGlobs::new(EnvPolicy::default_excludes()).unwrap(),
        set: EnvPolicy::proxy_vars(3128),
    }
}

#[test]
fn default_excludes_match_secret_shaped_names_case_insensitively() {
    let excluded = excluded_names(
        &default_policy(),
        names(&[
            "PATH",
            "HOME",
            "xai_api_key",
            "GITHUB_TOKEN",
            "NPM_CONFIG_PASSWORD",
            "AWS_SECRET_ACCESS_KEY",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "LD_LIBRARY_PATH",
            "LD_DEBUG",
            "TOKENIZERS_PARALLELISM",
        ]),
        names(&[]),
    );
    assert_eq!(
        names(&[
            "AWS_SECRET_ACCESS_KEY",
            "DYLD_INSERT_LIBRARIES",
            "GITHUB_TOKEN",
            "LD_LIBRARY_PATH",
            "LD_PRELOAD",
            "NPM_CONFIG_PASSWORD",
            "TOKENIZERS_PARALLELISM",
            "xai_api_key",
        ]),
        excluded
    );
}

/// Credential files' pointers and personal access tokens are secrets too, and a name that is not
/// UTF-8 is matched on its readable part; `*_PATH`, git's author and X's authority stay.
#[cfg(unix)]
#[test]
fn credentials_access_tokens_and_non_utf8_secret_names_are_excluded() {
    let non_utf8 = OsString::from_vec(b"API\xffKEY".to_vec());
    let mut inherited = names(&[
        "GOOGLE_APPLICATION_CREDENTIALS",
        "GITLAB_PAT",
        "GITHUB_PATH",
        "GIT_AUTHOR_NAME",
        "XAUTHORITY",
    ]);
    inherited.push(non_utf8.clone());
    let excluded = excluded_names(&default_policy(), inherited, names(&[]));
    let mut expected = names(&["GITLAB_PAT", "GOOGLE_APPLICATION_CREDENTIALS"]);
    expected.push(non_utf8);
    expected.sort();
    assert_eq!(expected, excluded);
}

#[test]
fn explicit_overrides_are_excluded_too_and_deduplicated() {
    let excluded = excluded_names(
        &default_policy(),
        names(&["MY_TOKEN"]),
        names(&["MY_TOKEN", "TOOL_SECRET"]),
    );
    assert_eq!(names(&["MY_TOKEN", "TOOL_SECRET"]), excluded);
}

/// A glob that does not parse is refused when the list is made, and a stored policy that carries
/// one does not load, so no policy that exists can fail to apply; the list is stored as written.
#[test]
fn an_invalid_glob_is_refused_when_the_list_is_made_or_loaded() {
    let error = EnvGlobs::new(vec!["*TOKEN*".to_owned(), "[unclosed".to_owned()]).unwrap_err();
    assert_eq!("[unclosed", error.glob);
    let stored = serde_json::json!({ "exclude_globs": ["[unclosed"], "set": {} });
    assert!(serde_json::from_value::<EnvPolicy>(stored).is_err());
    let stored = serde_json::json!({ "exclude_globs": ["*token*"], "set": {} });
    let loaded: EnvPolicy = serde_json::from_value(stored.clone()).unwrap();
    assert!(loaded.excludes("GITHUB_TOKEN"));
    assert!(!loaded.excludes("PATH"));
    assert_eq!(stored, serde_json::to_value(&loaded).unwrap());
}

#[test]
fn apply_removes_excluded_overrides_and_sets_the_proxy_pointers() {
    let mut cmd = tokio::process::Command::new("true");
    cmd.env("TOOL_API_KEY", "leak");
    cmd.env("PLAIN", "kept");
    apply_env_policy(&mut cmd, &default_policy());
    let envs: BTreeMap<OsString, Option<OsString>> = cmd
        .as_std()
        .get_envs()
        .map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string())))
        .collect();
    assert_eq!(Some(&None), envs.get(&OsString::from("TOOL_API_KEY")));
    assert_eq!(
        Some(&Some(OsString::from("kept"))),
        envs.get(&OsString::from("PLAIN"))
    );
    assert_eq!(
        Some(&Some(OsString::from("http://127.0.0.1:3128"))),
        envs.get(&OsString::from("HTTPS_PROXY"))
    );
    assert_eq!(
        Some(&Some(OsString::from("localhost,127.0.0.1,::1"))),
        envs.get(&OsString::from("NO_PROXY"))
    );
}

#[test]
fn set_only_never_removes_anything() {
    let mut cmd = tokio::process::Command::new("true");
    cmd.env("TOOL_API_KEY", "kept in observe");
    apply_env_set_only(&mut cmd, &default_policy());
    let envs: BTreeMap<OsString, Option<OsString>> = cmd
        .as_std()
        .get_envs()
        .map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string())))
        .collect();
    assert_eq!(
        Some(&Some(OsString::from("kept in observe"))),
        envs.get(&OsString::from("TOOL_API_KEY"))
    );
    assert_eq!(1 + default_policy().set.len(), envs.len());
}
