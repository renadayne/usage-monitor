//! Platform-independent desktop command service. Provider setup remains preview-only.
pub mod profile;

use collector::{
    export::HistoryExport,
    query::{EventCursor, SessionCursor, SessionDetail},
    Store,
};
use normalized_schema::{
    format_canonical_utc, Confidence, EventType, ModelEvidenceKind, NormalizedEvent, Phase,
    Provider,
};
use policy_engine::Policy;
use serde::Serialize;
use std::{
    path::Path,
    time::{Duration, Instant, SystemTime},
};

pub struct DesktopService {
    store: Store,
    clock: ClockHealth,
    started: Instant,
}
impl DesktopService {
    pub fn open(path: &Path) -> Result<Self, &'static str> {
        Ok(Self {
            store: Store::open(path).map_err(|_| "Không mở được cơ sở dữ liệu local.")?,
            clock: ClockHealth::default(),
            started: Instant::now(),
        })
    }
    fn time(&mut self) -> (String, bool) {
        let now = chrono::DateTime::<chrono::Utc>::from(SystemTime::now());
        let trusted = self
            .clock
            .observe(now.timestamp_millis(), self.started.elapsed());
        (format_canonical_utc(now), trusted)
    }
    pub fn dashboard(
        &mut self,
        cursor: Option<&SessionCursor>,
        address: Option<&str>,
    ) -> Result<serde_json::Value, &'static str> {
        let (now, trusted) = self.time();
        let data = self
            .store
            .dashboard_data(cursor)
            .map_err(|_| "Không đọc được danh sách phiên.")?;
        let mut value =
            serde_json::to_value(data).map_err(|_| "Không mã hóa được dữ liệu desktop.")?;
        value["receiver"] = serde_json::json!({"running":address.is_some(),"address":address});
        value["generatedAt"] = now.into();
        value["clockTrusted"] = trusted.into();
        Ok(value)
    }
    pub fn session_detail(
        &mut self,
        id: &str,
        cursor: Option<&EventCursor>,
    ) -> Result<SessionDetail, &'static str> {
        let (now, trusted) = self.time();
        self.store
            .session_detail(id, cursor, &now, trusted)
            .map_err(|_| "Không đọc được chi tiết phiên.")
    }
    pub fn list_policies(&self) -> Result<Vec<Policy>, &'static str> {
        self.store
            .list_policies()
            .map_err(|_| "Không đọc được policy local.")
    }
    pub fn save_policy(&mut self, policy: &Policy) -> Result<(), &'static str> {
        self.store
            .save_policy(policy)
            .map_err(|_| "Policy không hợp lệ hoặc chưa lưu được.")
    }
    pub fn change_phase(
        &mut self,
        id: &str,
        expected_epoch: i64,
        phase: Phase,
    ) -> Result<(), &'static str> {
        let (now, trusted) = self.time();
        if !trusted {
            return Err("Đồng hồ hệ thống cần kiểm tra trước khi đổi phase.");
        }
        let session = self
            .store
            .session_summary(id)
            .map_err(|_| "Không đọc được phiên.")?
            .ok_or("Không tìm thấy phiên.")?;
        let integration = self
            .store
            .list_integrations()
            .map_err(|_| "Không đọc được tích hợp.")?
            .into_iter()
            .find(|i| i.id == session.integration_id)
            .ok_or("Không tìm thấy tích hợp của phiên.")?;
        let provider: Provider = serde_json::from_value(integration.provider.into())
            .map_err(|_| "Provider không hợp lệ.")?;
        let next = expected_epoch
            .checked_add(1)
            .filter(|_| expected_epoch >= 0)
            .ok_or("Epoch không hợp lệ.")?;
        let event = NormalizedEvent {
            schema_version: 1,
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: EventType::PhaseChanged,
            integration_id: integration.id,
            account_key: integration.account_key,
            provider,
            surface: integration.surface,
            source: "aum.user".into(),
            source_event_id: None,
            source_sequence: None,
            source_session_id: session
                .source_session_id
                .ok_or("Phiên chưa có định danh nguồn.")?,
            local_session_id: Some(id.into()),
            source_turn_id: None,
            source_inference_id: None,
            parent_agent_id: None,
            phase_at_start: Some(phase),
            phase_epoch: Some(next),
            model_selected: None,
            model_requested: None,
            model_resolved: None,
            model_observed: None,
            model_evidence_kind: ModelEvidenceKind::Unknown,
            evidence_ref: None,
            observed_at: now.clone(),
            received_at: now,
            evidence_expires_at: None,
            confidence: Confidence::Direct,
            activity: None,
        };
        self.store
            .change_phase(&event, expected_epoch)
            .map_err(|_| "Không đổi được phase. Hãy làm mới epoch của phiên.")?;
        Ok(())
    }
    pub fn export_history(&mut self, days: u8) -> Result<HistoryExport, &'static str> {
        let (now, _) = self.time();
        self.store
            .export_history(days, &now)
            .map_err(|_| "Không xuất được lịch sử 7/30 ngày.")
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupPreview {
    pub merged_config: String,
    pub added_events: Vec<String>,
    pub skipped_events: Vec<String>,
    pub requires_trust: bool,
}
pub fn preview_setup(
    current_json: &str,
    helper_command: &str,
) -> Result<SetupPreview, &'static str> {
    let plan = setup_plan::plan_merge(current_json, helper_command)
        .map_err(|_| "JSON hoặc lệnh helper không hợp lệ.")?;
    let mut added = Vec::new();
    let mut skipped = Vec::new();
    for event in plan.preview.events {
        match event.action {
            setup_plan::MergeAction::AddedGroup => added.push(event.event.into()),
            setup_plan::MergeAction::SkippedExistingCommand => skipped.push(event.event.into()),
        }
    }
    Ok(SetupPreview {
        merged_config: plan.merged_config.into_string(),
        requires_trust: !added.is_empty(),
        added_events: added,
        skipped_events: skipped,
    })
}

pub struct ClockHealth {
    last: Option<(i64, Duration)>,
    trusted: bool,
}
impl Default for ClockHealth {
    fn default() -> Self {
        Self {
            last: None,
            trusted: true,
        }
    }
}
impl ClockHealth {
    /// Clock jumps revoke trust for this application run; elapsed time comes from Instant.
    pub fn observe(&mut self, wall_ms: i64, monotonic: Duration) -> bool {
        if let Some((previous, elapsed)) = self.last {
            let wall_delta = wall_ms as i128 - previous as i128;
            let monotonic_delta = monotonic.saturating_sub(elapsed).as_millis() as i128;
            if wall_delta < 0 || (wall_delta - monotonic_delta).abs() > 5000 {
                self.trusted = false;
            }
        }
        self.last = Some((wall_ms, monotonic));
        self.trusted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wall_clock_jump_revokes_trust_until_a_new_run() {
        let mut clock = ClockHealth::default();
        assert!(clock.observe(1000, Duration::ZERO));
        assert!(clock.observe(2000, Duration::from_secs(1)));
        assert!(!clock.observe(1900, Duration::from_secs(2)));
        assert!(!clock.observe(6900, Duration::from_secs(7)));
    }
    #[test]
    fn desktop_preview_is_in_memory_and_requires_hook_review() {
        let plan = preview_setup("{}", "aum-observer --config app-owned.json").unwrap();
        assert_eq!(plan.added_events.len(), 4);
        assert!(plan.requires_trust);
        assert!(preview_setup("{malformed", "safe").is_err());
    }
}
