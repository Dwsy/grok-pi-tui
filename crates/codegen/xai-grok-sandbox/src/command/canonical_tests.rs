use super::*;

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "xai-sandbox-canonical-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    dunce::canonicalize(&root).unwrap()
}

#[test]
fn dots_fold_lexically_and_never_climb_above_the_root() {
    assert_eq!(
        PathBuf::from("/opt/ws-fixture/.git/hooks"),
        fold_dots(Path::new("/opt/ws-fixture/src/../.git/./hooks"))
    );
    assert_eq!(PathBuf::from("/"), fold_dots(Path::new("/../..")));
    assert_eq!(
        PathBuf::from("/etc/passwd"),
        fold_dots(Path::new("/../../etc/passwd"))
    );
    assert_eq!(
        PathBuf::from("../../x"),
        fold_dots(Path::new("../a/../../x"))
    );
    assert_eq!(PathBuf::from("b"), fold_dots(Path::new("a/../b")));
}

/// The comparison folds case, Unicode form and the `/private` alias under APFS, for paths that
/// exist nowhere, and compares bytes otherwise; either way it compares whole components.
#[test]
fn the_comparison_folds_as_the_volume_rule_does_and_only_whole_components() {
    let root = Path::new("/opt/ws-fixture/ws/.git/hooks");
    let nfc = Path::new("/opt/ws-fixture/caf\u{e9}");
    let nfd = Path::new("/opt/ws-fixture/cafe\u{301}/x");
    for rule in [VolumeRule::Exact, VolumeRule::Apfs] {
        let folds = rule == VolumeRule::Apfs;
        with_volume_rule(rule, || {
            assert!(is_within(root, root));
            assert!(!is_within(
                Path::new("/opt/ws-fixture/ws/.git/hooksx"),
                root
            ));
            assert!(!is_within(Path::new("/opt/ws-fixture/ws/.git"), root));
            assert_eq!(
                folds,
                is_within(Path::new("/opt/ws-fixture/ws/.GIT/Hooks/pre-commit"), root)
            );
            assert!(!is_within(
                Path::new("/opt/ws-fixture/ws/.GIT/hooksx"),
                root
            ));
            assert_eq!(folds, is_within(nfd, nfc));
            assert_eq!(
                folds,
                is_within(
                    Path::new("/tmp/ws-fixture/x"),
                    Path::new("/private/tmp/ws-fixture")
                )
            );
            assert_eq!(
                folds,
                is_same_path(
                    Path::new("/private/VAR/ws-fixture"),
                    Path::new("/var/ws-fixture")
                )
            );
            assert!(!is_same_path(
                Path::new("/tmpx/a"),
                Path::new("/private/tmpx/a")
            ));
            assert_eq!(
                folds.then(|| PathBuf::from("hooks/pre-commit")),
                strip_within(
                    Path::new("/opt/ws-fixture/ws/.git/Hooks/pre-commit"),
                    Path::new("/opt/ws-fixture/ws/.GIT")
                )
            );
            let mut paths = vec![
                PathBuf::from("/opt/ws-fixture/Out"),
                PathBuf::from("/opt/ws-fixture/out"),
                PathBuf::from("/opt/ws-fixture/Out"),
            ];
            dedup_paths(&mut paths);
            assert_eq!(if folds { 1 } else { 2 }, paths.len());
            assert_eq!(
                Some(Path::new("/opt/ws-fixture/Out")),
                paths.first().map(PathBuf::as_path)
            );
        });
    }
}

/// Under APFS `/private/tmp/x` is exactly as deep as `/tmp/x`; byte for byte it is one deeper.
#[test]
fn depth_does_not_count_the_private_of_an_aliased_tree_under_apfs() {
    for (rule, private_depth) in [(VolumeRule::Exact, 3), (VolumeRule::Apfs, 2)] {
        with_volume_rule(rule, || {
            assert_eq!(2, depth(Path::new("/tmp/x")));
            assert_eq!(private_depth, depth(Path::new("/private/tmp/x")));
            assert_eq!(2, depth(Path::new("/private/x")));
        });
    }
}

/// A glob matches as a path compares: case-insensitively and through the alias under APFS.
#[test]
fn a_path_glob_matches_as_the_volume_rule_compares() {
    for rule in [VolumeRule::Exact, VolumeRule::Apfs] {
        let folds = rule == VolumeRule::Apfs;
        with_volume_rule(rule, || {
            let glob = PathGlob::new("/tmp/ws-fixture/.git/modules/**/hooks", true).unwrap();
            assert!(glob.is_match(Path::new("/tmp/ws-fixture/.git/modules/x/hooks")));
            assert_eq!(
                folds,
                glob.is_match(Path::new("/private/tmp/ws-fixture/.GIT/modules/x/Hooks"))
            );
            assert!(!glob.is_match(Path::new("/tmp/ws-fixture/.git/modules/x/hooksx")));
        });
    }
}

/// A path that does not exist folds lexically and keeps its spelling; one that exists resolves
/// through the filesystem (a symlinked prefix), with the missing tail appended as written.
#[test]
fn canonical_resolves_the_existing_prefix_and_appends_the_rest() {
    let root = scratch("prefix");
    let real = root.join("real");
    std::fs::create_dir_all(real.join("sub")).unwrap();
    assert_eq!(
        real.join("sub/not/yet"),
        canonical_path(&root.join("real/sub/../sub/not/yet"))
    );
    assert_eq!(
        PathBuf::from("/opt/ws-fixture/nowhere/.grok"),
        canonical_path(Path::new("/opt/ws-fixture/nowhere/x/../.grok"))
    );
    #[cfg(unix)]
    {
        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            real.join("sub/new.txt"),
            canonical_path(&link.join("sub/new.txt"))
        );
    }
    assert_eq!(PathBuf::from("rel/x"), canonical_path(Path::new("rel/./x")));
    let _ = std::fs::remove_dir_all(&root);
}

/// APFS is case-insensitive and normalisation-insensitive: `.GIT` and an NFD-spelled component
/// name the same directory as `.git` and its NFC spelling, and the canonical form is one.
#[cfg(target_os = "macos")]
#[test]
fn canonical_folds_apfs_case_and_unicode_form() {
    let root = scratch("apfs");
    std::fs::create_dir_all(root.join(".git/hooks")).unwrap();
    let nfc = "caf\u{e9}";
    let nfd = "cafe\u{301}";
    std::fs::create_dir_all(root.join(nfc)).unwrap();
    assert_eq!(
        root.join(".git/hooks/pre-commit"),
        canonical_path(&root.join(".GIT/Hooks/pre-commit"))
    );
    assert_eq!(
        root.join(nfc).join("new"),
        canonical_path(&root.join(nfd).join("new"))
    );
    // The tail that does not exist yet is folded to NFC as well
    assert_eq!(
        root.join("missing").join(nfc),
        canonical_path(&root.join("missing").join(nfd))
    );
    assert_eq!(
        PathBuf::from("/private/tmp/ws-fixture/x"),
        canonical_path(Path::new("/tmp/ws-fixture/x"))
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// One firmlink table: `/tmp`, `/var` and `/etc` toggle with their `/private` spelling, whole
/// components only; the resolved spellings add the canonical path and its toggle.
#[test]
fn firmlink_spellings_toggle_only_the_three_firmlinked_trees() {
    assert_eq!(
        vec![
            PathBuf::from("/tmp/ws-fixture/x"),
            PathBuf::from("/private/tmp/ws-fixture/x")
        ],
        firmlink_spellings(Path::new("/tmp/ws-fixture/x"))
    );
    assert_eq!(
        vec![PathBuf::from("/private/var"), PathBuf::from("/var")],
        firmlink_spellings(Path::new("/private/var"))
    );
    assert_eq!(
        vec![
            PathBuf::from("/etc/ws-fixture"),
            PathBuf::from("/private/etc/ws-fixture")
        ],
        firmlink_spellings(Path::new("/etc/ws-fixture"))
    );
    for untouched in [
        "/tmpx/a",
        "/private/tmpx/a",
        "/private",
        "/opt/tmp",
        "rel/tmp",
    ] {
        assert_eq!(
            vec![PathBuf::from(untouched)],
            firmlink_spellings(Path::new(untouched)),
            "{untouched}"
        );
    }
    #[cfg(unix)]
    {
        let root = scratch("spellings");
        let real = root.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let got = resolved_spellings(&link.join("x"));
        assert_eq!(Some(&link.join("x")), got.first());
        assert!(got.contains(&canonical_path(&real.join("x"))), "{got:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// A glob is compiled once per rule and separator mode, whatever the number of queries: the
/// floor and deny matchers ask the same few on every classification.
#[test]
fn a_glob_is_compiled_once_however_often_it_is_asked() {
    let glob = format!(
        "/opt/ws-fixture/memo-{}-{:?}/**/hooks",
        std::process::id(),
        std::thread::current().id()
    );
    for rule in [VolumeRule::Exact, VolumeRule::Apfs] {
        with_volume_rule(rule, || {
            let before = GLOB_COMPILES.get();
            for _ in 0..100 {
                assert!(PathGlob::new(&glob, true).is_some());
            }
            assert_eq!(1, GLOB_COMPILES.get() - before, "{rule:?}");
        });
    }
}
