//! Conservative snapshot projection; every accepted event remains in history.
use normalized_schema::{Confidence, EventType, ModelEvidenceKind, NormalizedEvent};
use rusqlite::{params, OptionalExtension, Transaction};

use crate::ingest::IngestError;

pub(crate) fn apply(
    tx: &Transaction<'_>,
    event: &NormalizedEvent,
    session: &str,
) -> Result<(), IngestError> {
    if event.event_type == EventType::PhaseChanged {
        return Ok(());
    }
    let (phase, epoch, boundary): (String, i64, String) = tx.query_row(
        "SELECT s.phase,s.phase_epoch,p.phase_changed_at FROM sessions s
         JOIN session_projection p ON p.session_id=s.id WHERE s.id=?1",
        [session],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let claimed_phase = event
        .phase_at_start
        .map(|p| serde_json::to_value(p).map(|v| v.as_str().unwrap_or_default().to_owned()))
        .transpose()?;
    let current_context = (event.phase_epoch == Some(epoch)
        || (epoch == 0 && event.phase_epoch.is_none()))
        && claimed_phase.as_ref().is_none_or(|p| p == &phase)
        && event.observed_at >= boundary;

    // Bind a start to its phase once. Reused inference IDs cannot silently rebind it.
    if event.event_type == EventType::InferenceStarted && current_context {
        if let Some(inference) = &event.source_inference_id {
            let prior: Option<String> = tx
                .query_row(
                    "SELECT start_event_id FROM inference_context
                 WHERE session_id=?1 AND source=?2 AND inference_id=?3",
                    params![session, event.source, inference],
                    |r| r.get(0),
                )
                .optional()?;
            if prior.is_some() {
                return Err(IngestError::Invalid("inference identity is already bound"));
            }
            if event.confidence == Confidence::Direct {
                tx.execute("INSERT INTO inference_context(session_id,source,inference_id,phase,phase_epoch,started_at,start_sequence,start_event_id)
                    VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![session,event.source,inference,phase,epoch,event.observed_at,event.source_sequence,event.event_id])?;
            }
        }
    }
    if !current_context || event.confidence != Confidence::Direct || event.evidence_ref.is_none() {
        return Ok(());
    }
    if event
        .evidence_expires_at
        .as_ref()
        .is_some_and(|expiry| expiry <= &event.observed_at || expiry <= &event.received_at)
    {
        return Ok(());
    }
    let channel = match event.model_evidence_kind {
        ModelEvidenceKind::SessionSelection | ModelEvidenceKind::HookCommonMetadata
            if event.model_selected.is_some() =>
        {
            "selection"
        }
        ModelEvidenceKind::RequestAttribute if event.model_requested.is_some() => "request",
        ModelEvidenceKind::ServedResponse if event.model_observed.is_some() => {
            let context: Option<(String, i64, String, Option<i64>)> = tx
                .query_row(
                    "SELECT phase,phase_epoch,started_at,start_sequence FROM inference_context
                 WHERE session_id=?1 AND source=?2 AND inference_id=?3",
                    params![session, event.source, event.source_inference_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let Some((start_phase, start_epoch, start_time, start_sequence)) = context else {
                return Ok(());
            };
            // Response claims must match the durable start, never receipt-time phase guesses.
            if start_phase != phase
                || start_epoch != epoch
                || claimed_phase.as_ref() != Some(&start_phase)
                || event.phase_epoch != Some(start_epoch)
                || event.observed_at < start_time
                || start_sequence
                    .is_some_and(|old| event.source_sequence.is_none_or(|new| new <= old))
            {
                return Ok(());
            }
            "served"
        }
        _ => return Ok(()),
    };
    let head: Option<(String,Option<i64>,String)> = tx.query_row(
        "SELECT source,source_sequence,observed_at FROM model_projection WHERE session_id=?1 AND channel=?2",
        params![session,channel], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).optional()?;
    if let Some((source, sequence, time)) = head {
        // Distinct sources and unsequenced observations do not establish a causal order.
        if source != event.source
            || event.observed_at < time
            || !matches!((sequence,event.source_sequence), (Some(old),Some(new)) if new > old)
        {
            return Ok(());
        }
    }
    match channel {
        "selection" => {
            tx.execute("UPDATE sessions SET model_selected=?1,
            model_evidence_kind=CASE WHEN model_observed IS NULL AND model_requested IS NULL THEN ?2 ELSE model_evidence_kind END,
            model_evidence=CASE WHEN model_observed IS NULL AND model_requested IS NULL THEN ?3 ELSE model_evidence END,
            updated_at=MAX(COALESCE(updated_at,''),?4) WHERE id=?5",
            params![event.model_selected,serde_json::to_value(event.model_evidence_kind)?.as_str(),event.evidence_ref,event.observed_at,session])?;
        }
        "request" => {
            tx.execute("UPDATE sessions SET model_requested=?1,model_resolved=?2,
            model_evidence_kind=CASE WHEN model_observed IS NULL THEN 'request_attribute' ELSE model_evidence_kind END,
            model_evidence=CASE WHEN model_observed IS NULL THEN ?3 ELSE model_evidence END,
            updated_at=MAX(COALESCE(updated_at,''),?4) WHERE id=?5",
            params![event.model_requested,event.model_resolved,event.evidence_ref,event.observed_at,session])?;
        }
        "served" => {
            tx.execute(
                "UPDATE sessions SET model_observed=?1,model_evidence_kind='served_response',
            model_evidence=?2,model_observed_at=?3,model_evidence_expires_at=?4,
            updated_at=MAX(COALESCE(updated_at,''),?3) WHERE id=?5",
                params![
                    event.model_observed,
                    event.evidence_ref,
                    event.observed_at,
                    event.evidence_expires_at,
                    session
                ],
            )?;
        }
        _ => unreachable!(),
    }
    tx.execute("INSERT INTO model_projection(session_id,channel,source,source_sequence,observed_at,phase_epoch,event_id)
        VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(session_id,channel) DO UPDATE SET
        source=excluded.source,source_sequence=excluded.source_sequence,observed_at=excluded.observed_at,
        phase_epoch=excluded.phase_epoch,event_id=excluded.event_id",
        params![session,channel,event.source,event.source_sequence,event.observed_at,epoch,event.event_id])?;
    Ok(())
}
