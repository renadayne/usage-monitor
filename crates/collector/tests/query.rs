use collector::query::QueryError;
use collector::Store;
use rusqlite::{params, Connection};
use tempfile::TempDir;

#[test]
fn keyset_pages_include_ties_and_null_times_without_duplicates() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("query.db");
    let store = Store::open(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    conn.execute("INSERT INTO integrations (id,provider,surface,account_key,capabilities_json,health,enabled) VALUES ('i','codex','synthetic','a','[]','unknown',1)", []).unwrap();
    for (id, time) in [
        ("a", Some("2026-10-27T01:00:00Z")),
        ("b", Some("2026-10-27T01:00:00Z")),
        ("c", Some("2026-10-27T00:00:00Z")),
        ("d", None),
        ("e", None),
    ] {
        conn.execute("INSERT INTO sessions (id,integration_id,activity,updated_at) VALUES (?1,'i','WAITING_USER',?2)", params![id,time]).unwrap();
    }
    let mut cursor = None;
    let mut seen = Vec::new();
    loop {
        let page = store.list_sessions(2, cursor.as_ref()).unwrap();
        assert!(page.items.iter().all(|s| s.activity == "WAITING_USER"));
        seen.extend(page.items.into_iter().map(|s| s.id));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen, ["a", "b", "c", "d", "e"]);
    assert!(matches!(
        store.list_sessions(0, None),
        Err(QueryError::InvalidPageSize)
    ));
    assert!(matches!(
        store.list_sessions(201, None),
        Err(QueryError::InvalidPageSize)
    ));

    for id in ["e1", "e2", "e3"] {
        conn.execute("INSERT INTO events (event_id,integration_id,account_key,session_id,type,source,confidence,observed_at,received_at,payload_json) VALUES (?1,'i','a','a','session.started','synthetic','unknown','2026-10-27T00:00:00Z','2026-10-27T00:00:00Z','{}')", [id]).unwrap();
    }
    let page1 = store.list_session_events("a", 2, None).unwrap();
    assert_eq!(
        page1
            .items
            .iter()
            .map(|e| e.event_id.as_str())
            .collect::<Vec<_>>(),
        ["e1", "e2"]
    );
    let page2 = store
        .list_session_events("a", 2, page1.next_cursor.as_ref())
        .unwrap();
    assert_eq!(page2.items[0].event_id, "e3");
    assert!(page2.next_cursor.is_none());
    assert!(store
        .list_session_events("a' OR 1=1 --", 2, None)
        .unwrap()
        .items
        .is_empty());
    assert!(store
        .list_session_events("missing", 2, None)
        .unwrap()
        .items
        .is_empty());
}
