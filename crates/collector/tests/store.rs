//! File-backed integration tests for durable SQLite store bootstrap (Step T02.1).
//! All fixtures are synthetic local storage in temporary directories; no provider claims.

use std::path::Path;

use collector::{
    Store, StoreError, CURRENT_SCHEMA_VERSION, DEFAULT_BUSY_TIMEOUT_MS, EXPECTED_TABLES,
};
use rusqlite::Connection;
use tempfile::TempDir;

#[test]
fn create_seed_and_reopen_preserves_rows_and_schema_version() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let db_path = temp_dir.path().join("store_bootstrap.sqlite3");

    {
        let store = Store::open(&db_path).expect("initial store open");
        assert_eq!(store.path(), db_path.as_path());
        assert_eq!(
            store.schema_version().expect("read schema version"),
            CURRENT_SCHEMA_VERSION
        );

        let expected: Vec<String> = EXPECTED_TABLES.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(store.table_names().expect("read table names"), expected);
    }

    {
        let conn = Connection::open(&db_path).expect("open raw connection for synthetic seeding");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("enable foreign_keys on test seeding connection");

        conn.execute(
            "INSERT INTO integrations (
                id, provider, surface, account_key, version, target_verification,
                capabilities_json, health, last_seen_at, installed_config_hash, enabled
             ) VALUES (
                'int-synth-1', 'codex', 'cli', 'acct-opaque-1', '0.0.0-synth', 'unknown',
                '[]', 'healthy', '2026-10-27T00:00:00Z', 'hash-1', 1
             )",
            [],
        )
        .expect("insert synthetic integration");

        conn.execute(
            "INSERT INTO sessions (
                id, integration_id, source_session_id, workspace_key, activity,
                model_evidence_kind, phase, phase_epoch
             ) VALUES (
                'sess-synth-1', 'int-synth-1', 'src-sess-1', 'ws-1', 'WORKING',
                'unknown', 'PLANNING', 1
             )",
            [],
        )
        .expect("insert synthetic session");

        conn.execute(
            "INSERT INTO events (
                event_id, integration_id, account_key, session_id, type, source,
                confidence, observed_at, received_at, payload_json
             ) VALUES (
                'evt-synth-1', 'int-synth-1', 'acct-opaque-1', 'sess-synth-1',
                'session.started', 'synthetic-test', 'direct',
                '2026-10-27T00:00:01Z', '2026-10-27T00:00:01Z', '{}'
             )",
            [],
        )
        .expect("insert synthetic event");
    }

    let reopened = Store::open(&db_path).expect("reopen existing store idempotently");
    assert_eq!(
        reopened
            .schema_version()
            .expect("schema version after reopen"),
        CURRENT_SCHEMA_VERSION
    );

    let verify_conn = Connection::open(&db_path).expect("open verification connection");
    let integration_count: i64 = verify_conn
        .query_row("SELECT COUNT(*) FROM integrations", [], |row| row.get(0))
        .expect("count integrations");
    let session_phase: String = verify_conn
        .query_row(
            "SELECT phase FROM sessions WHERE id = 'sess-synth-1'",
            [],
            |row| row.get(0),
        )
        .expect("read seeded session phase");
    let event_count: i64 = verify_conn
        .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
        .expect("count events");

    assert_eq!(integration_count, 1);
    assert_eq!(session_phase, "PLANNING");
    assert_eq!(event_count, 1);
}

#[test]
fn verifies_wal_synchronous_full_foreign_keys_and_busy_timeout_settings() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let db_path = temp_dir.path().join("settings.sqlite3");

    let store = Store::open(&db_path).expect("open store");
    let settings = store.settings().expect("read store settings");

    assert!(settings.foreign_keys, "foreign_keys must be enabled");
    assert_eq!(settings.journal_mode.to_ascii_lowercase(), "wal");
    assert_eq!(settings.synchronous, 2, "synchronous must be FULL (2)");
    assert_eq!(
        settings.busy_timeout_ms,
        i64::try_from(DEFAULT_BUSY_TIMEOUT_MS).expect("timeout fits in i64")
    );

    let external_conn = Connection::open(&db_path).expect("open external connection");
    let persisted_journal_mode: String = external_conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .expect("query persisted journal_mode from second connection");
    assert_eq!(
        persisted_journal_mode.to_ascii_lowercase(),
        "wal",
        "WAL journal_mode must persist at the database file level"
    );
}

#[test]
fn foreign_key_violations_are_rejected_across_schema_relations() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let db_path = temp_dir.path().join("fk_enforcement.sqlite3");

    let _store = Store::open(&db_path).expect("initialize store schema");

    let conn = Connection::open(&db_path).expect("open test connection");
    conn.pragma_update(None, "foreign_keys", "ON")
        .expect("enable PRAGMA foreign_keys on separate connection");

    let session_fk_err = conn
        .execute(
            "INSERT INTO sessions (
                id, integration_id, activity
             ) VALUES (
                'sess-orphan', 'nonexistent-integration', 'IDLE'
             )",
            [],
        )
        .expect_err("inserting a session referencing a missing integration must fail FK check");
    assert!(
        session_fk_err
            .to_string()
            .contains("FOREIGN KEY constraint failed"),
        "expected FOREIGN KEY constraint error, got: {session_fk_err}"
    );

    let outbox_fk_err = conn
        .execute(
            "INSERT INTO outbox (
                id, event_id, kind, payload_json
             ) VALUES (
                'outbox-orphan', 'nonexistent-event', 'toast', '{}'
             )",
            [],
        )
        .expect_err("inserting an outbox item referencing a missing event must fail FK check");
    assert!(
        outbox_fk_err
            .to_string()
            .contains("FOREIGN KEY constraint failed"),
        "expected FOREIGN KEY constraint error, got: {outbox_fk_err}"
    );

    let violation_fk_err = conn
        .execute(
            "INSERT INTO violations (
                id, policy_id, session_id, event_id, model_evidence_kind, created_at
             ) VALUES (
                'viol-orphan', 'missing-policy', 'missing-session', 'missing-event',
                'served_response', '2026-10-27T00:00:00Z'
             )",
            [],
        )
        .expect_err("inserting a violation referencing missing parent rows must fail FK check");
    assert!(
        violation_fk_err
            .to_string()
            .contains("FOREIGN KEY constraint failed"),
        "expected FOREIGN KEY constraint error, got: {violation_fk_err}"
    );
}

#[test]
fn refuses_future_schema_version_without_mutating_journal_or_data() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let db_path = temp_dir.path().join("future_version.sqlite3");

    {
        let conn = Connection::open(&db_path).expect("create future-version db");
        let mode: String = conn
            .query_row("PRAGMA journal_mode = DELETE;", [], |row| row.get(0))
            .expect("set DELETE journal_mode");
        assert_eq!(mode.to_ascii_lowercase(), "delete");

        conn.execute(
            "CREATE TABLE future_custom_table (id TEXT PRIMARY KEY, value TEXT NOT NULL);",
            [],
        )
        .expect("create future table");
        conn.execute(
            "INSERT INTO future_custom_table (id, value) VALUES ('k1', 'preserved');",
            [],
        )
        .expect("seed future row");
        conn.pragma_update(None, "user_version", 99)
            .expect("set future user_version = 99");
    }

    let err = Store::open(&db_path).expect_err("future user_version must be refused");
    assert!(
        matches!(err, StoreError::UnsupportedSchemaVersion(99)),
        "expected UnsupportedSchemaVersion(99), got: {err:?}"
    );

    let verify_conn = Connection::open(&db_path).expect("reopen future db for verification");
    let version: i64 = verify_conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read user_version");
    let journal_mode: String = verify_conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .expect("read journal_mode");
    let preserved_value: String = verify_conn
        .query_row(
            "SELECT value FROM future_custom_table WHERE id = 'k1'",
            [],
            |row| row.get(0),
        )
        .expect("read preserved row");
    let table_count: i64 = verify_conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .expect("count tables");

    assert_eq!(version, 99, "future user_version must remain untouched");
    assert_eq!(
        journal_mode.to_ascii_lowercase(),
        "delete",
        "journal_mode must not be mutated to WAL when rejecting unsupported schema version"
    );
    assert_eq!(preserved_value, "preserved");
    assert_eq!(
        table_count, 1,
        "no collector tables may be created on rejected DB"
    );
}

#[test]
fn refuses_nonempty_unversioned_foreign_database_without_mutating_journal_or_data() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let db_path = temp_dir.path().join("foreign_unversioned.sqlite3");

    {
        let conn = Connection::open(&db_path).expect("create foreign unversioned db");
        let mode: String = conn
            .query_row("PRAGMA journal_mode = DELETE;", [], |row| row.get(0))
            .expect("set DELETE journal_mode");
        assert_eq!(mode.to_ascii_lowercase(), "delete");

        conn.execute(
            "CREATE TABLE unrelated_app_data (id INTEGER PRIMARY KEY, note TEXT NOT NULL);",
            [],
        )
        .expect("create foreign table");
        conn.execute(
            "INSERT INTO unrelated_app_data (id, note) VALUES (1, 'do-not-touch');",
            [],
        )
        .expect("insert foreign row");
    }

    let err = Store::open(&db_path).expect_err("nonempty unversioned DB must be refused");
    assert!(
        matches!(
            err,
            StoreError::NonEmptyUnversionedDatabase { object_count } if object_count >= 1
        ),
        "expected NonEmptyUnversionedDatabase, got: {err:?}"
    );

    let verify_conn = Connection::open(&db_path).expect("reopen foreign db for verification");
    let version: i64 = verify_conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read user_version");
    let journal_mode: String = verify_conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .expect("read journal_mode");
    let note: String = verify_conn
        .query_row(
            "SELECT note FROM unrelated_app_data WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .expect("read foreign row");
    let table_count: i64 = verify_conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .expect("count tables");

    assert_eq!(version, 0, "user_version must remain 0");
    assert_eq!(
        journal_mode.to_ascii_lowercase(),
        "delete",
        "journal_mode must not be mutated on rejected foreign DB"
    );
    assert_eq!(note, "do-not-touch");
    assert_eq!(table_count, 1);
}

#[test]
fn rejects_missing_parent_directory_and_in_memory_paths() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let missing_parent = temp_dir.path().join("nonexistent_subdir");
    let db_path = missing_parent.join("collector.sqlite3");

    let err = Store::open(&db_path).expect_err("missing parent directory must fail");
    assert!(
        matches!(err, StoreError::MissingParentDirectory(_)),
        "expected MissingParentDirectory, got: {err:?}"
    );
    assert!(
        !missing_parent.exists(),
        "Store::open must not implicitly create missing parent directories"
    );

    let mem_err = Store::open(Path::new(":memory:")).expect_err("in-memory path must be rejected");
    assert!(
        matches!(mem_err, StoreError::InvalidDatabasePath(_)),
        "expected InvalidDatabasePath, got: {mem_err:?}"
    );
}
