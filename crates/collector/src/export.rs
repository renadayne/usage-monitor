//! Explicit, bounded metadata export. Never exports raw hook bodies, paths, or account identities.
use crate::{ingest::IngestError, Store};
use normalized_schema::{format_canonical_utc, parse_canonical_utc, NormalizedEvent};
use rusqlite::params;
use serde::Serialize;

pub const MAX_EXPORT_ROWS: usize = 10_000;
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryExport {
    pub csv: String,
    pub rows: usize,
    pub truncated: bool,
}

impl Store {
    /// Observation-time history, inclusive of the cutoff, excluding future observations.
    pub fn export_history(&self, days: u8, now: &str) -> Result<HistoryExport, IngestError> {
        if !matches!(days, 7 | 30) {
            return Err(IngestError::Invalid("export window must be 7 or 30 days"));
        }
        let now = parse_canonical_utc("now", now)?;
        let since = now
            .checked_sub_signed(chrono::TimeDelta::days(days.into()))
            .ok_or(IngestError::Invalid("export date range is invalid"))?;
        let mut stmt=self.conn.prepare("SELECT e.payload_json,e.session_id,i.provider,i.surface FROM events e JOIN integrations i ON i.id=e.integration_id
            WHERE e.session_id IS NOT NULL AND e.observed_at>=?1 AND e.observed_at<=?2 ORDER BY e.observed_at DESC,e.event_id ASC LIMIT ?3")?;
        let rows = stmt
            .query_map(
                params![
                    format_canonical_utc(since),
                    format_canonical_utc(now),
                    (MAX_EXPORT_ROWS + 1) as i64
                ],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let truncated = rows.len() > MAX_EXPORT_ROWS;
        let mut csv="\u{feff}ObservedUTC,ReceivedUTC,Provider,Surface,SessionID,EventID,EventType,Phase,PhaseEpoch,Selected,Requested,Resolved,Served,EvidenceKind,Confidence,Source,InferenceID\r\n".to_owned();
        let count = rows.len().min(MAX_EXPORT_ROWS);
        for (body, session, provider, surface) in rows.into_iter().take(MAX_EXPORT_ROWS) {
            let value: serde_json::Value = serde_json::from_str(&body)?;
            let event: NormalizedEvent = if value.get("event").is_some() {
                let report: normalized_schema::usage::UsageReport = serde_json::from_value(value)?;
                report.validate()?;
                report.event
            } else {
                serde_json::from_value(value)?
            };
            event.validate()?;
            let fields = [
                event.observed_at,
                event.received_at,
                provider,
                surface,
                session,
                event.event_id,
                wire(event.event_type)?,
                event
                    .phase_at_start
                    .map(wire)
                    .transpose()?
                    .unwrap_or_default(),
                event.phase_epoch.map(|n| n.to_string()).unwrap_or_default(),
                event.model_selected.unwrap_or_default(),
                event.model_requested.unwrap_or_default(),
                event.model_resolved.unwrap_or_default(),
                event.model_observed.unwrap_or_default(),
                wire(event.model_evidence_kind)?,
                wire(event.confidence)?,
                event.source,
                event.source_inference_id.unwrap_or_default(),
            ];
            csv.push_str(
                &fields
                    .iter()
                    .map(|s| csv_cell(s))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            csv.push_str("\r\n");
        }
        Ok(HistoryExport {
            csv,
            rows: count,
            truncated,
        })
    }
}

fn wire(value: impl Serialize) -> Result<String, IngestError> {
    Ok(serde_json::to_value(value)?
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

fn csv_cell(value: &str) -> String {
    let formula = value
        .trim_start()
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '=' | '+' | '-' | '@'))
        || value.chars().any(char::is_control);
    let escaped = value.replace('"', "\"\"");
    format!("\"{}{escaped}\"", if formula { "'" } else { "" })
}
