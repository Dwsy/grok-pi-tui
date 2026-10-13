//! Read-only copy of `xai-fast-worktree`'s `WorktreeDb::get`, so the rules crate need not link its writer or create the DB
//!
//! Reading a WAL database can still create `-wal` and `-shm` side files, and on a network mount the open may convert the journal mode

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Row};
use xai_sqlite_journal::JournalMode;

const WORKTREES_DB_FILE: &str = "worktrees.db";
// The lookups and decoding below match this writer schema version only
const SCHEMA_VERSION: u32 = 1;
const GET_SCHEMA_VERSION: &str = "SELECT value FROM meta WHERE key = 'schema_version'";
const GET_BY_ID: &str = "SELECT * FROM worktrees WHERE id = ?1";
const GET_BY_LABEL: &str = "SELECT * FROM worktrees WHERE json_valid(metadata) AND json_extract(metadata, '$.label') = ?1 ORDER BY created_at DESC";
const GET_BY_PATH: &str = "SELECT * FROM worktrees WHERE path = ?1";

/// The recorded source repo of the grok-managed worktree containing `cwd`; the DB is not opened unless `cwd` is under `<grok_home>/worktrees/`
pub(crate) fn source_repo_for_cwd(cwd: &str) -> Option<PathBuf> {
    source_repo_for_cwd_in(&xai_dirs::resolve_grok_home()?, cwd)
}

fn source_repo_for_cwd_in(grok_home: &Path, cwd: &str) -> Option<PathBuf> {
    let worktrees_dir = grok_home.join("worktrees");
    let mut path = Path::new(cwd);
    if !path.starts_with(&worktrees_dir) {
        return None;
    }

    let conn = open_read_only(grok_home)?;
    if !schema_supported(&conn) {
        return None;
    }

    while path.starts_with(&worktrees_dir) && path != worktrees_dir {
        if let Ok(Some(source_repo)) = get(&conn, &path.to_string_lossy()) {
            return Some(source_repo);
        }
        path = path.parent()?;
    }
    None
}

fn open_read_only(grok_home: &Path) -> Option<Connection> {
    let db_path = grok_home.join(WORKTREES_DB_FILE);
    let mode = JournalMode::for_db_path(&db_path);
    let effective = mode.effective_db_path(&db_path);
    if !matches!(effective.try_exists(), Ok(true)) {
        return None;
    }
    match mode.open_readonly(&effective) {
        Ok(conn) => Some(conn),
        Err(e) => {
            tracing::warn!(error = %e, "failed to open worktree DB for cwd lookup");
            None
        }
    }
}

fn schema_supported(conn: &Connection) -> bool {
    let version = conn.query_row(GET_SCHEMA_VERSION, [], |row| row.get::<_, String>(0));
    matches!(version, Ok(v) if v.parse::<u32>() == Ok(SCHEMA_VERSION))
}

/// Matches `WorktreeDb::get`, including Windows paths without `/` falling through to the id and label lookups
fn get(conn: &Connection, id_or_path: &str) -> rusqlite::Result<Option<PathBuf>> {
    if id_or_path.contains('/') {
        let canon = crate::trust::canonicalize_or_owned(Path::new(id_or_path));
        return get_one(conn, GET_BY_PATH, &canon.to_string_lossy());
    }
    match get_one(conn, GET_BY_ID, id_or_path)? {
        Some(source_repo) => Ok(Some(source_repo)),
        None => get_one(conn, GET_BY_LABEL, id_or_path),
    }
}

fn get_one(conn: &Connection, sql: &str, param: &str) -> rusqlite::Result<Option<PathBuf>> {
    conn.query_row(sql, [param], decode_source_repo).optional()
}

/// Decodes every column the writer crate's reader decodes, so a row it rejects is rejected here too
fn decode_source_repo(row: &Row<'_>) -> rusqlite::Result<PathBuf> {
    for column in ["id", "path", "repo_name", "kind", "creation_mode", "status"] {
        row.get::<_, String>(column)?;
    }
    for column in ["git_ref", "head_commit", "session_id", "metadata"] {
        row.get::<_, Option<String>>(column)?;
    }
    for column in ["creator_pid", "last_accessed_at"] {
        row.get::<_, Option<i64>>(column)?;
    }
    row.get::<_, i64>("created_at")?;
    Ok(PathBuf::from(row.get::<_, String>("source_repo")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Copy of the writer crate's schema
    const CREATE_SCHEMA: &str = "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    INSERT INTO meta (key, value) VALUES ('schema_version', '1');
    CREATE TABLE worktrees (
        id TEXT PRIMARY KEY,
        path TEXT UNIQUE NOT NULL,
        source_repo TEXT NOT NULL,
        repo_name TEXT NOT NULL,
        kind TEXT NOT NULL DEFAULT 'session',
        creation_mode TEXT NOT NULL DEFAULT 'linked',
        git_ref TEXT,
        head_commit TEXT,
        session_id TEXT,
        creator_pid INTEGER,
        created_at INTEGER NOT NULL,
        last_accessed_at INTEGER,
        status TEXT NOT NULL DEFAULT 'alive',
        metadata TEXT
    );";

    struct Registry {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        conn: Connection,
    }

    fn registry() -> Registry {
        let tmp = tempfile::tempdir().unwrap();
        let home = dunce::canonicalize(tmp.path()).unwrap().join("grok-home");
        std::fs::create_dir_all(home.join("worktrees")).unwrap();
        let conn = Connection::open(effective_db_path(&home)).unwrap();
        conn.execute_batch(CREATE_SCHEMA).unwrap();
        Registry {
            _tmp: tmp,
            home,
            conn,
        }
    }

    fn effective_db_path(home: &Path) -> PathBuf {
        let db_path = home.join(WORKTREES_DB_FILE);
        JournalMode::for_db_path(&db_path).effective_db_path(&db_path)
    }

    fn insert(reg: &Registry, id: &str, path: &str, created_at: &str, metadata: Option<&str>) {
        reg.conn
            .execute(
                "INSERT INTO worktrees (id, path, source_repo, repo_name, created_at, metadata) \
                 VALUES (?1, ?2, ?3, 'repo', ?4, ?5)",
                rusqlite::params![id, path, format!("/src/{id}"), created_at, metadata],
            )
            .unwrap();
    }

    fn worktree_dir(reg: &Registry, name: &str) -> PathBuf {
        let wt = reg.home.join("worktrees").join("repo").join(name);
        std::fs::create_dir_all(&wt).unwrap();
        wt
    }

    #[test]
    fn nested_cwd_resolves_to_registered_worktree() {
        let reg = registry();
        let wt = worktree_dir(&reg, "wt");
        insert(&reg, "wt", &wt.to_string_lossy(), "1", None);
        let nested = wt.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            source_repo_for_cwd_in(&reg.home, &nested.to_string_lossy()),
            Some(PathBuf::from("/src/wt"))
        );
    }

    #[test]
    fn cwd_outside_worktrees_dir_is_a_miss() {
        let reg = registry();
        let outside = reg.home.join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        insert(&reg, "wt", &outside.to_string_lossy(), "1", None);
        assert_eq!(
            source_repo_for_cwd_in(&reg.home, &outside.to_string_lossy()),
            None
        );
    }

    #[test]
    fn missing_registry_is_a_miss_and_is_not_created() {
        let tmp = tempfile::tempdir().unwrap();
        let home = dunce::canonicalize(tmp.path()).unwrap().join("grok-home");
        let wt = home.join("worktrees").join("repo").join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        assert_eq!(source_repo_for_cwd_in(&home, &wt.to_string_lossy()), None);
        assert!(!home.join(WORKTREES_DB_FILE).exists());
        assert!(!effective_db_path(&home).exists());
    }

    #[test]
    fn unreadable_registry_is_a_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let home = dunce::canonicalize(tmp.path()).unwrap().join("grok-home");
        let wt = home.join("worktrees").join("repo").join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(effective_db_path(&home), b"not a sqlite database").unwrap();
        assert_eq!(source_repo_for_cwd_in(&home, &wt.to_string_lossy()), None);
    }

    #[test]
    fn other_schema_version_is_a_miss() {
        let reg = registry();
        let wt = worktree_dir(&reg, "wt");
        insert(&reg, "wt", &wt.to_string_lossy(), "1", None);
        let cwd = wt.to_string_lossy();
        reg.conn
            .execute(
                "UPDATE meta SET value = '2' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        assert_eq!(source_repo_for_cwd_in(&reg.home, &cwd), None);
        reg.conn.execute("DELETE FROM meta", []).unwrap();
        assert_eq!(source_repo_for_cwd_in(&reg.home, &cwd), None);
    }

    #[test]
    fn undecodable_row_is_a_miss() {
        let reg = registry();
        let wt = worktree_dir(&reg, "wt");
        insert(&reg, "wt", &wt.to_string_lossy(), "not a number", None);
        assert_eq!(
            source_repo_for_cwd_in(&reg.home, &wt.to_string_lossy()),
            None
        );
    }

    #[test]
    fn malformed_metadata_still_decodes() {
        let reg = registry();
        let wt = worktree_dir(&reg, "wt");
        insert(&reg, "wt", &wt.to_string_lossy(), "1", Some("{not json"));
        assert_eq!(
            source_repo_for_cwd_in(&reg.home, &wt.to_string_lossy()),
            Some(PathBuf::from("/src/wt"))
        );
    }

    #[test]
    fn key_without_slash_uses_id_then_label_not_path() {
        let reg = registry();
        let backslash_path = r"C:\grok\worktrees\repo\wt";
        insert(
            &reg,
            "wt-id",
            backslash_path,
            "1",
            Some(r#"{"label":"wt"}"#),
        );
        assert_eq!(get(&reg.conn, backslash_path).unwrap(), None);
        assert_eq!(
            get(&reg.conn, "wt-id").unwrap(),
            Some(PathBuf::from("/src/wt-id"))
        );
        assert_eq!(
            get(&reg.conn, "wt").unwrap(),
            Some(PathBuf::from("/src/wt-id"))
        );
    }

    #[test]
    fn newest_label_row_decides_without_fallback() {
        let reg = registry();
        insert(&reg, "old", "/wt/old", "1", Some(r#"{"label":"lbl"}"#));
        insert(&reg, "new", "/wt/new", "2", Some(r#"{"label":"lbl"}"#));
        assert_eq!(
            get(&reg.conn, "lbl").unwrap(),
            Some(PathBuf::from("/src/new"))
        );
        // SQLite sorts text above integers, so the undecodable row now comes first
        reg.conn
            .execute("UPDATE worktrees SET created_at = 'x' WHERE id = 'old'", [])
            .unwrap();
        assert!(get(&reg.conn, "lbl").is_err());
    }

    #[test]
    fn undecodable_id_row_skips_label_lookup() {
        let reg = registry();
        insert(&reg, "x", "/wt/x", "not a number", None);
        insert(&reg, "y", "/wt/y", "1", Some(r#"{"label":"x"}"#));
        assert!(get(&reg.conn, "x").is_err());
    }
}
