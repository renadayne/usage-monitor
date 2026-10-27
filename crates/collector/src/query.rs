//! Bounded, read-only views of committed session and event state.

use std::fmt;

use rusqlite::{params, Row};
use serde::{Deserialize, Serialize};

use crate::Store;

pub const MAX_PAGE_SIZE: u32 = 200;

#[derive(Debug)]
pub enum QueryError {
    InvalidPageSize,
    Sqlite(rusqlite::Error),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPageSize => write!(f, "page size must be between 1 and {MAX_PAGE_SIZE}"),
            Self::Sqlite(error) => write!(f, "session query failed: {error}"),
        }
    }
}

impl std::error::Error for QueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for QueryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCursor {
    pub updated_at: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub integration_id: String,
    pub source_session_id: Option<String>,
    pub activity: String,
    pub phase: String,
    pub phase_epoch: i64,
    pub updated_at: Option<String>,
    pub model_selected: Option<String>,
    pub model_requested: Option<String>,
    pub model_resolved: Option<String>,
    pub model_observed: Option<String>,
    pub model_evidence_kind: String,
    pub model_observed_at: Option<String>,
    pub model_evidence_expires_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPage {
    pub items: Vec<SessionSummary>,
    pub next_cursor: Option<SessionCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventCursor {
    pub observed_at: String,
    pub event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventSummary {
    pub event_id: String,
    pub event_type: String,
    pub source: String,
    pub confidence: String,
    pub observed_at: String,
    pub received_at: String,
    pub source_inference_id: Option<String>,
    pub phase_at_start: Option<String>,
    pub phase_epoch: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPage {
    pub items: Vec<EventSummary>,
    pub next_cursor: Option<EventCursor>,
}

impl Store {
    /// At most 500 integration bindings, excluding any configuration or credentials.
    pub fn list_integrations(&self) -> Result<Vec<IntegrationSummary>, QueryError> {
        let mut stmt=self.conn.prepare("SELECT id,provider,surface,account_key,health,target_verification,last_seen_at FROM integrations ORDER BY provider,surface,id LIMIT 500")?;
        let rows = stmt.query_map([], |r| {
            Ok(IntegrationSummary {
                id: r.get(0)?,
                provider: r.get(1)?,
                surface: r.get(2)?,
                account_key: r.get(3)?,
                health: r.get(4)?,
                target_verification: r.get(5)?,
                last_seen_at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    /// Recent committed sessions, ordered by fixed-width UTC update time descending then ID ascending.
    /// Cursors are keyset boundaries, not a frozen snapshot if sessions change between requests.
    /// Reading never infers idle/disconnected or upgrades stale model evidence.
    pub fn list_sessions(
        &self,
        limit: u32,
        after: Option<&SessionCursor>,
    ) -> Result<SessionPage, QueryError> {
        let fetch = fetch_limit(limit)?;
        let mut statement = self.conn.prepare(
            "SELECT id, integration_id, source_session_id, activity, phase, phase_epoch,
                    updated_at, model_selected, model_requested, model_observed,
                    model_evidence_kind, model_observed_at, model_evidence_expires_at, model_resolved
             FROM sessions
             WHERE ?1 IS NULL OR COALESCE(updated_at, '') < ?1
                OR (COALESCE(updated_at, '') = ?1 AND id > ?2)
             ORDER BY COALESCE(updated_at, '') DESC, id ASC LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![after.map(|c| &c.updated_at), after.map(|c| &c.id), fetch],
            session_from_row,
        )?;
        let mut items = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let more = items.len() > limit as usize;
        items.truncate(limit as usize);
        let next_cursor = if more {
            items.last().map(|last| SessionCursor {
                updated_at: last.updated_at.clone().unwrap_or_default(),
                id: last.id.clone(),
            })
        } else {
            None
        };
        Ok(SessionPage { items, next_cursor })
    }

    /// Sanitized timeline metadata. Raw payload JSON and workspace paths are not exposed.
    pub fn list_session_events(
        &self,
        session_id: &str,
        limit: u32,
        after: Option<&EventCursor>,
    ) -> Result<EventPage, QueryError> {
        let fetch = fetch_limit(limit)?;
        let mut statement = self.conn.prepare(
            "SELECT event_id, type, source, confidence, observed_at, received_at,
                    source_inference_id, phase_at_start, phase_epoch
             FROM events WHERE session_id = ?1
               AND (?2 IS NULL OR observed_at > ?2 OR (observed_at = ?2 AND event_id > ?3))
             ORDER BY observed_at ASC, event_id ASC LIMIT ?4",
        )?;
        let rows = statement.query_map(
            params![
                session_id,
                after.map(|c| &c.observed_at),
                after.map(|c| &c.event_id),
                fetch
            ],
            |row| {
                Ok(EventSummary {
                    event_id: row.get(0)?,
                    event_type: row.get(1)?,
                    source: row.get(2)?,
                    confidence: row.get(3)?,
                    observed_at: row.get(4)?,
                    received_at: row.get(5)?,
                    source_inference_id: row.get(6)?,
                    phase_at_start: row.get(7)?,
                    phase_epoch: row.get(8)?,
                })
            },
        )?;
        let mut items = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let more = items.len() > limit as usize;
        items.truncate(limit as usize);
        let next_cursor = if more {
            items.last().map(|last| EventCursor {
                observed_at: last.observed_at.clone(),
                event_id: last.event_id.clone(),
            })
        } else {
            None
        };
        Ok(EventPage { items, next_cursor })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationSummary {
    pub id: String,
    pub provider: String,
    pub surface: String,
    pub account_key: String,
    pub health: String,
    pub target_verification: String,
    pub last_seen_at: Option<String>,
}

fn fetch_limit(limit: u32) -> Result<i64, QueryError> {
    if !(1..=MAX_PAGE_SIZE).contains(&limit) {
        return Err(QueryError::InvalidPageSize);
    }
    Ok(i64::from(limit) + 1)
}

pub(crate) fn session_from_row(row: &Row<'_>) -> rusqlite::Result<SessionSummary> {
    Ok(SessionSummary {
        id: row.get(0)?,
        integration_id: row.get(1)?,
        source_session_id: row.get(2)?,
        activity: row.get(3)?,
        phase: row.get(4)?,
        phase_epoch: row.get(5)?,
        updated_at: row.get(6)?,
        model_selected: row.get(7)?,
        model_requested: row.get(8)?,
        model_observed: row.get(9)?,
        model_evidence_kind: row.get(10)?,
        model_observed_at: row.get(11)?,
        model_evidence_expires_at: row.get(12)?,
        model_resolved: row.get(13)?,
    })
}
