//! Authenticated loopback-only Codex observer intake with a bounded durable writer.
use adapters::codex::{Surface, CODEX_HOOKS_SOURCE};
use axum::{
    body::to_bytes,
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use collector::{
    ingest::{IngestError, IngestOutcome, IntegrationRegistration},
    Store,
};
use normalized_schema::{
    format_canonical_utc, Confidence, EventType, ModelEvidenceKind, NormalizedEvent, Provider,
};
use std::{
    collections::HashMap,
    net::Ipv4Addr,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::{
    net::TcpListener,
    sync::{oneshot, Semaphore},
};

pub const MAX_REQUEST_BYTES: usize = 65_536;
pub const DEFAULT_QUEUE_CAPACITY: usize = 32;
const ACK_DEADLINE: Duration = Duration::from_millis(150);

/// App-owned random 32-byte secret encoded as 64 hex digits. Never passed in a hook command.
pub struct ObserverBinding {
    pub integration_id: String,
    pub account_key: String,
    pub surface: Surface,
    token: String,
}
impl ObserverBinding {
    pub fn new(
        integration_id: String,
        account_key: String,
        surface: Surface,
        token: String,
    ) -> Result<Self, &'static str> {
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("observer token must contain 64 hex digits");
        }
        normalized_schema::validate_identifier("integrationId", &integration_id)
            .map_err(|_| "invalid integration identity")?;
        if !integration_id.is_ascii() {
            return Err("observer HTTP integration identity must be ASCII");
        }
        normalized_schema::validate_identifier("accountKey", &account_key)
            .map_err(|_| "invalid account identity")?;
        Ok(Self {
            integration_id,
            account_key,
            surface,
            token,
        })
    }
}

struct WriteRequest {
    event: NormalizedEvent,
    reply: oneshot::Sender<Result<IngestOutcome, IngestError>>,
}
struct IntakeState {
    bindings: HashMap<String, ObserverBinding>,
    host: String,
    sender: mpsc::SyncSender<WriteRequest>,
    stopping: Arc<AtomicBool>,
    inflight: Arc<Semaphore>,
}

/// Dropping the guard stops admission; the writer finishes its current transaction and rejects queued requests.
pub struct WriterGuard {
    stopping: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl WriterGuard {
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }
    /// Call from a blocking thread when the application quits.
    pub fn join(mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for WriterGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct ObserverServer {
    pub address: std::net::SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    writer: WriterGuard,
}
impl ObserverServer {
    pub async fn shutdown(mut self) -> Result<(), &'static str> {
        self.writer.stop();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        match tokio::time::timeout(Duration::from_secs(2), &mut self.task).await {
            Ok(result) => result
                .map_err(|_| "observer server task failed")?
                .map_err(|_| "observer listener failed")?,
            Err(_) => {
                self.task.abort();
                let _ = (&mut self.task).await;
            }
        }
        tokio::task::spawn_blocking(move || self.writer.join())
            .await
            .map_err(|_| "observer writer task failed")?;
        Ok(())
    }
}

/// The listener is always IPv4 loopback; callers can select a port or zero for an ephemeral port.
pub async fn start(
    store: Store,
    bindings: Vec<ObserverBinding>,
    port: u16,
) -> Result<ObserverServer, &'static str> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|_| "cannot bind observer loopback port")?;
    let address = listener
        .local_addr()
        .map_err(|_| "cannot inspect observer listener")?;
    let (router, writer) =
        build_router(store, bindings, address.to_string(), DEFAULT_QUEUE_CAPACITY)?;
    let (shutdown, done) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = done.await;
            })
            .await
    });
    Ok(ObserverServer {
        address,
        shutdown: Some(shutdown),
        task,
        writer,
    })
}

fn build_router(
    mut store: Store,
    bindings: Vec<ObserverBinding>,
    host: String,
    capacity: usize,
) -> Result<(Router, WriterGuard), &'static str> {
    if bindings.is_empty() || bindings.len() > 16 || !(1..=128).contains(&capacity) {
        return Err("invalid observer binding or queue count");
    }
    let mut by_id = HashMap::new();
    for binding in bindings {
        if by_id.contains_key(&binding.integration_id) {
            return Err("duplicate observer integration");
        }
        store
            .register_integration(&IntegrationRegistration {
                id: binding.integration_id.clone(),
                account_key: binding.account_key.clone(),
                provider: Provider::Codex,
                surface: binding.surface.as_str().into(),
            })
            .map_err(|_| "observer integration registration failed")?;
        by_id.insert(binding.integration_id.clone(), binding);
    }
    let (sender, receiver) = mpsc::sync_channel::<WriteRequest>(capacity);
    let stopping = Arc::new(AtomicBool::new(false));
    let stop = stopping.clone();
    let worker = thread::Builder::new()
        .name("aum-durable-writer".into())
        .spawn(move || {
            while !stop.load(Ordering::Acquire) {
                match receiver.recv_timeout(Duration::from_millis(25)) {
                    Ok(request) => {
                        let result = store.ingest(&request.event);
                        let _ = request.reply.send(result);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            // Dropping replies yields 503 to outstanding callers. No queued item is acknowledged.
            while receiver.try_recv().is_ok() {}
        })
        .map_err(|_| "cannot start observer writer")?;
    let state = Arc::new(IntakeState {
        bindings: by_id,
        host,
        sender,
        stopping: stopping.clone(),
        inflight: Arc::new(Semaphore::new(64)),
    });
    Ok((
        Router::new()
            .route("/events/codex", post(intake))
            .with_state(state),
        WriterGuard {
            stopping,
            worker: Some(worker),
        },
    ))
}

fn response(status: StatusCode, body: serde_json::Value) -> Response {
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}
fn failure(status: StatusCode, code: &'static str) -> Response {
    response(status, serde_json::json!({"error":code}))
}

async fn intake(State(state): State<Arc<IntakeState>>, request: Request) -> Response {
    if state.stopping.load(Ordering::Acquire) {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "stopping");
    }
    let Ok(_permit) = state.inflight.clone().try_acquire_owned() else {
        return failure(StatusCode::TOO_MANY_REQUESTS, "busy");
    };
    let headers = request.headers();
    if headers.contains_key(header::ORIGIN)
        || headers.get(header::HOST).and_then(|h| h.to_str().ok()) != Some(state.host.as_str())
    {
        return failure(StatusCode::FORBIDDEN, "local_request_required");
    }
    let binding = headers
        .get("x-aum-integration")
        .and_then(|h| h.to_str().ok())
        .and_then(|id| state.bindings.get(id));
    let Some(binding) = binding else {
        return failure(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let supplied = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or_default();
    if supplied
        .as_bytes()
        .ct_eq(binding.token.as_bytes())
        .unwrap_u8()
        != 1
    {
        return failure(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        != Some("application/json")
    {
        return failure(StatusCode::UNSUPPORTED_MEDIA_TYPE, "json_required");
    }
    let bytes = match tokio::time::timeout(
        Duration::from_millis(100),
        to_bytes(request.into_body(), MAX_REQUEST_BYTES),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => return failure(StatusCode::PAYLOAD_TOO_LARGE, "body_limit"),
        Err(_) => return failure(StatusCode::REQUEST_TIMEOUT, "body_timeout"),
    };
    let mut event: NormalizedEvent = match serde_json::from_slice(&bytes) {
        Ok(event) => event,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "invalid_metadata"),
    };
    if !matches_binding(&event, binding) {
        return failure(StatusCode::FORBIDDEN, "observer_contract_required");
    }
    if event.validate().is_err() {
        return failure(StatusCode::BAD_REQUEST, "invalid_metadata");
    }
    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "clock_unavailable");
    };
    let Some(timestamp) = i64::try_from(now.as_secs())
        .ok()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, now.subsec_nanos()))
    else {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "clock_unavailable");
    };
    event.received_at = format_canonical_utc(timestamp);
    let (reply, result) = oneshot::channel();
    match state.sender.try_send(WriteRequest { event, reply }) {
        Ok(()) => {}
        Err(mpsc::TrySendError::Full(_)) => {
            return failure(StatusCode::TOO_MANY_REQUESTS, "queue_full")
        }
        Err(mpsc::TrySendError::Disconnected(_)) => {
            return failure(StatusCode::SERVICE_UNAVAILABLE, "writer_unavailable")
        }
    }
    match tokio::time::timeout(ACK_DEADLINE, result).await {
        Ok(Ok(Ok(outcome))) => response(
            StatusCode::OK,
            serde_json::json!({"eventId":outcome.event_key,"sessionId":outcome.session_id,"alreadyCommitted":outcome.already_committed}),
        ),
        Ok(Ok(Err(IngestError::Sqlite(_)))) => {
            failure(StatusCode::SERVICE_UNAVAILABLE, "commit_failed")
        }
        Ok(Ok(Err(_))) => failure(StatusCode::CONFLICT, "event_rejected"),
        _ => failure(StatusCode::SERVICE_UNAVAILABLE, "commit_unconfirmed"),
    }
}

fn matches_binding(event: &NormalizedEvent, binding: &ObserverBinding) -> bool {
    event.integration_id == binding.integration_id
        && event.account_key == binding.account_key
        && event.surface == binding.surface.as_str()
        && event.provider == Provider::Codex
        && event.source == CODEX_HOOKS_SOURCE
        && matches!(
            event.event_type,
            EventType::SessionStarted | EventType::SessionEnded | EventType::ModelObserved
        )
        && event.phase_at_start.is_none()
        && event.phase_epoch.is_none()
        && event.source_sequence.is_none()
        && event.source_event_id.is_none()
        && event.source_inference_id.is_none()
        && event.parent_agent_id.is_none()
        && event.local_session_id.is_none()
        && event.activity.is_none()
        && event.model_requested.is_none()
        && event.model_resolved.is_none()
        && event.model_observed.is_none()
        && event.evidence_expires_at.is_none()
        && match event.model_selected {
            Some(_) => {
                event.model_evidence_kind == ModelEvidenceKind::HookCommonMetadata
                    && event.confidence == Confidence::Direct
                    && event.evidence_ref.as_ref() == Some(&event.event_id)
            }
            None => {
                event.model_evidence_kind == ModelEvidenceKind::Unknown
                    && event.confidence == Confidence::Unknown
                    && event.evidence_ref.is_none()
            }
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adapters::codex::{normalize_hook, CodexContext};
    use axum::{body::Body, http::Request as HttpRequest};
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn binding() -> ObserverBinding {
        ObserverBinding::new(
            "codex".into(),
            "account".into(),
            Surface::VsCodeExtension,
            "a".repeat(64),
        )
        .unwrap()
    }
    fn metadata() -> NormalizedEvent {
        normalize_hook(
            include_str!("../../../tests/fixtures/codex/session_start.json"),
            &CodexContext {
                integration_id: "codex".into(),
                account_key: "account".into(),
                surface: Surface::VsCodeExtension,
                event_id: "stable-helper-delivery".into(),
                observed_at: "2026-10-28T00:00:00Z".into(),
                received_at: "2026-10-28T00:00:00Z".into(),
            },
        )
        .unwrap()
    }
    fn request(bytes: Vec<u8>) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method("POST")
            .uri("/events/codex")
            .header("host", "127.0.0.1:4319")
            .header("content-type", "application/json")
            .header("x-aum-integration", "codex")
            .header("authorization", format!("Bearer {}", "a".repeat(64)))
            .body(Body::from(bytes))
            .unwrap()
    }
    fn setup(capacity: usize) -> (TempDir, Router, WriterGuard) {
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path().join("intake.db")).unwrap();
        let (router, guard) =
            build_router(store, vec![binding()], "127.0.0.1:4319".into(), capacity).unwrap();
        (dir, router, guard)
    }

    #[tokio::test]
    async fn success_ack_requires_commit_and_retries_are_durable_duplicates() {
        let (dir, router, guard) = setup(4);
        let db = rusqlite::Connection::open(dir.path().join("intake.db")).unwrap();
        db.execute_batch(
            "CREATE TRIGGER fail BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT,'test'); END;",
        )
        .unwrap();
        let bytes = serde_json::to_vec(&metadata()).unwrap();
        let failed = router
            .clone()
            .oneshot(request(bytes.clone()))
            .await
            .unwrap();
        assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        db.execute_batch("DROP TRIGGER fail;").unwrap();
        let accepted = router
            .clone()
            .oneshot(request(bytes.clone()))
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::OK);
        assert_eq!(accepted.headers()["cache-control"], "no-store");
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        let retried = router.oneshot(request(bytes)).await.unwrap();
        assert_eq!(retried.status(), StatusCode::OK);
        let body = to_bytes(retried.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["alreadyCommitted"],
            true
        );
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        tokio::task::spawn_blocking(move || guard.join())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn origin_auth_host_content_and_forged_evidence_are_rejected_without_writes() {
        let (dir, router, guard) = setup(4);
        let bytes = serde_json::to_vec(&metadata()).unwrap();
        for (name, value, status) in [
            ("origin", "https://example.invalid", StatusCode::FORBIDDEN),
            ("authorization", "Bearer invalid", StatusCode::UNAUTHORIZED),
            ("host", "example.invalid:4319", StatusCode::FORBIDDEN),
            (
                "content-type",
                "text/plain",
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
        ] {
            let mut req = request(bytes.clone());
            req.headers_mut()
                .insert(name, header::HeaderValue::from_str(value).unwrap());
            assert_eq!(router.clone().oneshot(req).await.unwrap().status(), status);
        }
        let mut forged = metadata();
        forged.model_observed = Some("forged".into());
        assert_eq!(
            router
                .clone()
                .oneshot(request(serde_json::to_vec(&forged).unwrap()))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let mut content = serde_json::to_value(metadata()).unwrap();
        content["prompt"] = serde_json::json!("fake-secret-prompt");
        assert_eq!(
            router
                .clone()
                .oneshot(request(serde_json::to_vec(&content).unwrap()))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            router
                .oneshot(request(vec![b'x'; MAX_REQUEST_BYTES + 1]))
                .await
                .unwrap()
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let db = rusqlite::Connection::open(dir.path().join("intake.db")).unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        tokio::task::spawn_blocking(move || guard.join())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn saturated_queue_returns_429_and_unconfirmed_commits_never_return_200() {
        let (dir, router, guard) = setup(1);
        let db = rusqlite::Connection::open(dir.path().join("intake.db")).unwrap();
        db.execute_batch("BEGIN IMMEDIATE;").unwrap();
        let bytes = serde_json::to_vec(&metadata()).unwrap();
        let (a, b, c) = tokio::join!(
            router.clone().oneshot(request(bytes.clone())),
            router.clone().oneshot(request(bytes.clone())),
            router.oneshot(request(bytes))
        );
        let statuses = [
            a.unwrap().status(),
            b.unwrap().status(),
            c.unwrap().status(),
        ];
        assert!(statuses.contains(&StatusCode::TOO_MANY_REQUESTS));
        assert!(statuses
            .iter()
            .all(|s| *s == StatusCode::SERVICE_UNAVAILABLE || *s == StatusCode::TOO_MANY_REQUESTS));
        db.execute_batch("ROLLBACK;").unwrap();
        tokio::task::spawn_blocking(move || guard.join())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn listener_is_loopback_and_shuts_down_with_its_writer() {
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path().join("listener.db")).unwrap();
        let server = start(store, vec![binding()], 0).await.unwrap();
        assert_eq!(
            server.address.ip(),
            std::net::IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        server.shutdown().await.unwrap();
    }
}
