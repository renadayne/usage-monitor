//! Durable SQLite store initialization, pragma verification, and schema migration.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, TransactionBehavior};

/// Current SQLite `PRAGMA user_version` managed by this crate.
pub const CURRENT_SCHEMA_VERSION: u32 = 3;

/// Bounded SQLite busy timeout in milliseconds.
pub const DEFAULT_BUSY_TIMEOUT_MS: u64 = 5_000;

/// SQLite numeric code for `PRAGMA synchronous = FULL`.
const SYNCHRONOUS_FULL: i64 = 2;

/// Canonical initial DDL (10 tables + index), with connection pragmas stripped out.
const INITIAL_MIGRATION_SQL: &str = include_str!("../migrations/0001_initial.sql");

/// Schema v2 migration adding `session_projection` and backfilling existing sessions.
const PROJECTION_MIGRATION_SQL: &str = include_str!("../migrations/0002_projection.sql");
const MODEL_MIGRATION_SQL: &str = include_str!("../migrations/0003_model_projection.sql");

/// Base v1 table inventory (10 tables).
const V1_TABLES: [&str; 10] = [
    "events",
    "integrations",
    "outbox",
    "policies",
    "policy_overrides",
    "quotas",
    "sessions",
    "usage_checkpoints",
    "usage_contributions",
    "violations",
];

/// Expected table inventory for `CURRENT_SCHEMA_VERSION` (sorted ascending).
const V2_TABLES: [&str; 11] = [
    "events",
    "integrations",
    "outbox",
    "policies",
    "policy_overrides",
    "quotas",
    "session_projection",
    "sessions",
    "usage_checkpoints",
    "usage_contributions",
    "violations",
];

/// Expected table inventory for the current schema, sorted ascending.
pub const EXPECTED_TABLES: [&str; 13] = [
    "events",
    "inference_context",
    "integrations",
    "model_projection",
    "outbox",
    "policies",
    "policy_overrides",
    "quotas",
    "session_projection",
    "sessions",
    "usage_checkpoints",
    "usage_contributions",
    "violations",
];

/// Read-only snapshot of active connection PRAGMA settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreSettings {
    pub foreign_keys: bool,
    pub journal_mode: String,
    pub synchronous: i64,
    pub busy_timeout_ms: i64,
}

/// Errors returned while opening, verifying, or migrating the SQLite store.
#[derive(Debug)]
pub enum StoreError {
    /// The caller supplied an empty or in-memory path rather than a file path.
    InvalidDatabasePath(PathBuf),
    /// The parent directory of the database path does not exist.
    MissingParentDirectory(PathBuf),
    /// The database has a negative or newer `user_version` not supported by this build.
    UnsupportedSchemaVersion(i64),
    /// The database has `user_version = 0` but already contains user schema objects.
    NonEmptyUnversionedDatabase { object_count: i64 },
    /// The database claims a supported `user_version` but is missing required schema objects.
    InvalidExistingSchema { missing_object: &'static str },
    /// SQLite failed to switch to `WAL` journal mode.
    UnexpectedJournalMode(String),
    /// A required connection pragma could not be verified after application.
    PragmaVerificationFailed {
        pragma: &'static str,
        expected: String,
        actual: String,
    },
    /// Underlying SQLite error.
    Sqlite(rusqlite::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDatabasePath(path) => {
                write!(f, "invalid or non-file database path: '{}'", path.display())
            }
            Self::MissingParentDirectory(parent) => {
                write!(
                    f,
                    "database parent directory does not exist: '{}'",
                    parent.display()
                )
            }
            Self::UnsupportedSchemaVersion(version) => {
                write!(
                    f,
                    "unsupported database schema user_version {} (supported: {})",
                    version, CURRENT_SCHEMA_VERSION
                )
            }
            Self::NonEmptyUnversionedDatabase { object_count } => {
                write!(
                    f,
                    "refusing to adopt unversioned database containing {} existing schema objects",
                    object_count
                )
            }
            Self::InvalidExistingSchema { missing_object } => {
                write!(
                    f,
                    "existing versioned database is missing required schema object '{}'",
                    missing_object
                )
            }
            Self::UnexpectedJournalMode(mode) => {
                write!(f, "expected WAL journal_mode, got '{}'", mode)
            }
            Self::PragmaVerificationFailed {
                pragma,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "failed to verify PRAGMA {}: expected {}, got {}",
                    pragma, expected, actual
                )
            }
            Self::Sqlite(err) => write!(f, "sqlite error: {}", err),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(err) => Some(err),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sqlite(err)
    }
}

/// Durable local SQLite store owning a configured connection.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    pub(crate) conn: Connection,
}

impl Store {
    /// Opens or initializes a file-backed SQLite database at `path`.
    ///
    /// - Requires `path` to be a file path whose parent directory already exists.
    /// - Validates `PRAGMA user_version` and schema cleanliness before mutating `journal_mode`.
    /// - Enforces `foreign_keys = ON`, `journal_mode = WAL`, `synchronous = FULL`, and a bounded
    ///   `busy_timeout`.
    /// - Applies each ordered migration transactionally through schema version 3.
    ///   Existing v1 data is preserved and its projection checkpoints are backfilled.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path_ref = path.as_ref();
        validate_file_path(path_ref)?;

        let mut conn = Connection::open(path_ref)?;
        conn.busy_timeout(Duration::from_millis(DEFAULT_BUSY_TIMEOUT_MS))?;

        let initial_version = read_user_version(&conn)?;
        if initial_version < 0 || initial_version > i64::from(CURRENT_SCHEMA_VERSION) {
            return Err(StoreError::UnsupportedSchemaVersion(initial_version));
        }

        if initial_version == 0 {
            let object_count = count_user_schema_objects(&conn)?;
            if object_count > 0 {
                return Err(StoreError::NonEmptyUnversionedDatabase { object_count });
            }
        } else {
            verify_schema_for_version(&conn, initial_version as u32)?;
        }

        configure_and_verify_connection(&conn)?;

        if initial_version < 1 {
            apply_migration_sql(&mut conn, INITIAL_MIGRATION_SQL, 1)?;
        }
        if initial_version < 2 {
            apply_migration_sql(&mut conn, PROJECTION_MIGRATION_SQL, 2)?;
        }
        if initial_version < 3 {
            apply_migration_sql(&mut conn, MODEL_MIGRATION_SQL, 3)?;
        }

        verify_expected_schema(&conn)?;

        Ok(Self {
            path: path_ref.to_path_buf(),
            conn,
        })
    }

    /// Returns the filesystem path of the opened SQLite database.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Queries the live `PRAGMA user_version` of the database.
    pub fn schema_version(&self) -> Result<u32, StoreError> {
        let raw = read_user_version(&self.conn)?;
        u32::try_from(raw).map_err(|_| StoreError::UnsupportedSchemaVersion(raw))
    }

    /// Reads the active connection PRAGMA settings (`foreign_keys`, `journal_mode`, `synchronous`,
    /// and `busy_timeout`).
    pub fn settings(&self) -> Result<StoreSettings, StoreError> {
        read_connection_settings(&self.conn)
    }

    /// Returns the sorted list of user tables currently present in the database.
    pub fn table_names(&self) -> Result<Vec<String>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name ASC",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut names = Vec::new();
        for row in rows {
            names.push(row?);
        }
        Ok(names)
    }
}

fn validate_file_path(path: &Path) -> Result<(), StoreError> {
    let raw = path.as_os_str().to_string_lossy();
    if raw.is_empty() || raw == ":memory:" || raw.starts_with("file::memory:") {
        return Err(StoreError::InvalidDatabasePath(path.to_path_buf()));
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            return Err(StoreError::MissingParentDirectory(parent.to_path_buf()));
        }
    }

    Ok(())
}

fn read_user_version(conn: &Connection) -> Result<i64, StoreError> {
    let version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok(version)
}

fn count_user_schema_objects(conn: &Connection) -> Result<i64, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    Ok(count)
}

fn configure_and_verify_connection(conn: &Connection) -> Result<(), StoreError> {
    conn.pragma_update(None, "foreign_keys", "ON")?;

    let journal_mode: String =
        conn.query_row("PRAGMA journal_mode = WAL;", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(StoreError::UnexpectedJournalMode(journal_mode));
    }

    conn.pragma_update(None, "synchronous", "FULL")?;

    let settings = read_connection_settings(conn)?;
    if !settings.foreign_keys {
        return Err(StoreError::PragmaVerificationFailed {
            pragma: "foreign_keys",
            expected: "1".to_string(),
            actual: "0".to_string(),
        });
    }
    if !settings.journal_mode.eq_ignore_ascii_case("wal") {
        return Err(StoreError::UnexpectedJournalMode(settings.journal_mode));
    }
    if settings.synchronous != SYNCHRONOUS_FULL {
        return Err(StoreError::PragmaVerificationFailed {
            pragma: "synchronous",
            expected: SYNCHRONOUS_FULL.to_string(),
            actual: settings.synchronous.to_string(),
        });
    }
    if settings.busy_timeout_ms <= 0 {
        return Err(StoreError::PragmaVerificationFailed {
            pragma: "busy_timeout",
            expected: DEFAULT_BUSY_TIMEOUT_MS.to_string(),
            actual: settings.busy_timeout_ms.to_string(),
        });
    }

    Ok(())
}

fn read_connection_settings(conn: &Connection) -> Result<StoreSettings, StoreError> {
    let foreign_keys_raw: i64 = conn.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    let journal_mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    let synchronous: i64 = conn.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    let busy_timeout_ms: i64 = conn.pragma_query_value(None, "busy_timeout", |row| row.get(0))?;

    Ok(StoreSettings {
        foreign_keys: foreign_keys_raw != 0,
        journal_mode,
        synchronous,
        busy_timeout_ms,
    })
}

fn apply_migration_sql(
    conn: &mut Connection,
    migration_sql: &str,
    target_version: u32,
) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    let current_version = read_user_version(&tx)?;
    if current_version == 0 && target_version == 1 {
        let object_count = count_user_schema_objects(&tx)?;
        if object_count > 0 {
            return Err(StoreError::NonEmptyUnversionedDatabase { object_count });
        }
        tx.pragma_update(None, "user_version", target_version)?;
        tx.execute_batch(migration_sql)?;
        verify_schema_for_version(&tx, target_version)?;
    } else if current_version > 0 && current_version + 1 == i64::from(target_version) {
        verify_schema_for_version(&tx, current_version as u32)?;
        tx.pragma_update(None, "user_version", target_version)?;
        tx.execute_batch(migration_sql)?;
        verify_schema_for_version(&tx, target_version)?;
    } else if current_version == i64::from(target_version) {
        verify_schema_for_version(&tx, target_version)?;
    } else {
        return Err(StoreError::UnsupportedSchemaVersion(current_version));
    }

    tx.commit()?;
    Ok(())
}

fn verify_expected_schema(conn: &Connection) -> Result<(), StoreError> {
    verify_schema_for_version(conn, CURRENT_SCHEMA_VERSION)
}

fn verify_schema_for_version(conn: &Connection, version: u32) -> Result<(), StoreError> {
    let expected_tables: &[&str] = match version {
        1 => &V1_TABLES,
        2 => &V2_TABLES,
        3 => &EXPECTED_TABLES,
        _ => return Err(StoreError::UnsupportedSchemaVersion(i64::from(version))),
    };

    for &table in expected_tables {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )?;
        if exists != 1 {
            return Err(StoreError::InvalidExistingSchema {
                missing_object: table,
            });
        }
    }

    let index_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master \
         WHERE type = 'index' AND name = 'idx_event_session_time' AND tbl_name = 'events'",
        [],
        |row| row.get(0),
    )?;
    if index_exists != 1 {
        return Err(StoreError::InvalidExistingSchema {
            missing_object: "idx_event_session_time",
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn failed_migration_rolls_back_atomically_and_allows_subsequent_clean_open() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let db_path = temp_dir.path().join("atomic_rollback.db");

        {
            let mut conn = Connection::open(&db_path).expect("open raw sqlite connection");
            configure_and_verify_connection(&conn).expect("configure connection pragmas");

            let broken_migration = format!(
                "{}\nINSERT INTO definitely_missing_table VALUES (1);",
                INITIAL_MIGRATION_SQL
            );

            let err = apply_migration_sql(&mut conn, &broken_migration, 1)
                .expect_err("broken migration SQL must fail");
            assert!(matches!(err, StoreError::Sqlite(_)));

            assert_eq!(
                read_user_version(&conn).expect("read user_version after rollback"),
                0,
                "user_version must remain 0 when migration fails"
            );
            assert_eq!(
                count_user_schema_objects(&conn).expect("count schema objects after rollback"),
                0,
                "all tables/indexes from failed migration must be rolled back"
            );
        }

        let store = Store::open(&db_path).expect("clean open after rolled-back migration");
        assert_eq!(
            store.schema_version().expect("schema version"),
            CURRENT_SCHEMA_VERSION
        );
        assert_eq!(
            store.table_names().expect("table names"),
            EXPECTED_TABLES
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
        );
    }
}
