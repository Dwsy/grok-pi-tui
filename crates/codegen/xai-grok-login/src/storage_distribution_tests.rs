use super::*;

const STORED: &str = r#"{"cursor::session":{"key":"stored-login","auth_mode":"oidc","create_time":"2026-01-01T00:00:00Z","user_id":"u"}}"#;

fn stored_login() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    std::fs::write(&path, STORED).unwrap();
    (dir, path)
}

#[test]
fn without_account_logins_there_is_no_login_store() {
    let (_dir, path) = stored_login();
    let error = read_auth_json_as(
        Distribution::withholding(&[Capability::AccountLogin]),
        &path,
    )
    .expect_err("no stored login is ever read");
    assert_eq!(std::io::ErrorKind::NotFound, error.kind());
    assert!(
        read_auth_json_as(Distribution::STOCK, &path)
            .unwrap()
            .contains_key("cursor::session")
    );
}

#[test]
fn without_account_logins_nothing_is_stored_and_the_file_is_left_alone() {
    let (dir, path) = stored_login();
    let error = write_auth_json_as(
        Distribution::withholding(&[Capability::AccountLogin]),
        &path,
        &AuthStore::new(),
    )
    .expect_err("no login is stored");
    assert_eq!(std::io::ErrorKind::PermissionDenied, error.kind());
    assert_eq!(STORED, std::fs::read_to_string(&path).unwrap());
    assert_eq!(1, std::fs::read_dir(dir.path()).unwrap().count());
}

#[test]
fn only_an_unparseable_file_is_backed_up() {
    let (dir, path) = stored_login();
    assert_eq!(None, backup_corrupt_auth_file(&path));

    std::fs::write(&path, "{not json").unwrap();
    let backup = backup_corrupt_auth_file(&path).expect("a corrupt file is backed up");
    assert!(!path.exists());
    assert_eq!("{not json", std::fs::read_to_string(backup).unwrap());
    assert_eq!(1, std::fs::read_dir(dir.path()).unwrap().count());
}

#[test]
fn an_unreadable_file_is_left_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    std::fs::create_dir(&path).unwrap();
    assert_ne!(
        std::io::ErrorKind::InvalidData,
        read_auth_json(&path).unwrap_err().kind()
    );
    assert_eq!(None, backup_corrupt_auth_file(&path));
    assert!(path.is_dir());
    assert_eq!(1, std::fs::read_dir(dir.path()).unwrap().count());
}
