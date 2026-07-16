//! The oxen-harness agent backend as an HTTP server.
//!
//! One `harness_host::SessionService` behind two surfaces any UI can consume:
//!
//! - `GET /v1/events` — a single SSE stream carrying every
//!   [`harness_protocol::ProtocolEvent`], session-tagged, with `Last-Event-ID`
//!   replay for reconnects and an optional `?session=` filter.
//! - REST routes for everything a client sends: sessions, turns, question and
//!   approval answers, model selection, attachments.
//!
//! Auth is a single bearer token (localhost, single user): `Authorization:
//! Bearer <token>` on every `/v1` route, or `?token=` for `EventSource`,
//! which can't set headers. The wire shapes are `harness-protocol`'s; the
//! tests in `tests/http.rs` pin the HTTP contract.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use harness_host::{EventSink, SessionService, SessionServiceBuilder};
use harness_protocol::ProtocolEvent;
use serde::Deserialize;
use tokio::sync::broadcast;

/// An event with its monotonic stream id.
type Numbered = (u64, ProtocolEvent);

/// How many events the replay buffer keeps for `Last-Event-ID` reconnects.
const REPLAY_CAPACITY: usize = 4096;
/// Broadcast channel depth; a slower SSE consumer lags (skips) past this.
const CHANNEL_CAPACITY: usize = 4096;

/// The server's [`EventSink`]: every protocol event gets a monotonic id, goes
/// to all live SSE subscribers, and lands in a bounded replay buffer so a
/// reconnecting client can catch up from its `Last-Event-ID`.
pub struct BroadcastSink {
    tx: broadcast::Sender<Numbered>,
    replay: Mutex<VecDeque<Numbered>>,
    next_id: AtomicU64,
}

impl Default for BroadcastSink {
    fn default() -> Self {
        Self::new()
    }
}

impl BroadcastSink {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            tx,
            replay: Mutex::new(VecDeque::with_capacity(REPLAY_CAPACITY)),
            next_id: AtomicU64::new(1),
        }
    }

    /// Subscribe, returning the backlog after `after_id` plus the live feed.
    fn subscribe(&self, after_id: u64) -> (Vec<Numbered>, broadcast::Receiver<Numbered>) {
        // Subscribe FIRST, then snapshot: an event arriving in between shows
        // up in both, and the id-based dedupe downstream drops the duplicate.
        let rx = self.tx.subscribe();
        let backlog = self
            .replay
            .lock()
            .expect("replay buffer poisoned")
            .iter()
            .filter(|(id, _)| *id > after_id)
            .cloned()
            .collect();
        (backlog, rx)
    }
}

impl EventSink for BroadcastSink {
    fn emit(&self, event: ProtocolEvent) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut replay = self.replay.lock().expect("replay buffer poisoned");
            if replay.len() == REPLAY_CAPACITY {
                replay.pop_front();
            }
            replay.push_back((id, event.clone()));
        }
        let _ = self.tx.send((id, event));
    }
}

/// Everything the routes share.
struct AppState {
    service: Arc<SessionService>,
    sink: Arc<BroadcastSink>,
    token: String,
    uploads_dir: std::path::PathBuf,
}

/// Customizes the service the server runs (tests inject a mock client and an
/// in-memory store; production passes `None` and gets the shared defaults).
pub type ConfigureFn = Box<dyn FnOnce(SessionServiceBuilder) -> SessionServiceBuilder + Send>;

pub struct ServerConfig {
    /// The bearer token every request must carry.
    pub token: String,
    /// Optional hook to customize the underlying session service.
    pub configure: Option<ConfigureFn>,
}

/// Build the axum router and its backing service. Exposed for embedding; most
/// callers use [`serve`] or [`serve_on_ephemeral_port`].
pub fn build_router(config: ServerConfig) -> Router {
    let sink = Arc::new(BroadcastSink::new());
    let mut builder = SessionService::builder(sink.clone());
    if let Some(configure) = config.configure {
        builder = configure(builder);
    }
    let service = Arc::new(builder.build());
    let uploads_dir = harness_config::paths::base_dir()
        .map(|d| d.join("uploads"))
        .unwrap_or_else(|_| std::env::temp_dir().join("oxen-harness-uploads"));
    let state = Arc::new(AppState {
        service,
        sink,
        token: config.token,
        uploads_dir,
    });

    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/events", get(events))
        .route("/v1/sessions", get(list_sessions).post(new_session))
        .route(
            "/v1/sessions/{id}",
            get(resume_session).delete(delete_session),
        )
        .route("/v1/sessions/{id}/messages", get(session_messages))
        .route("/v1/sessions/{id}/turns", post(run_turn))
        .route("/v1/sessions/{id}/turns/retry", post(retry_turn))
        .route("/v1/sessions/{id}/interject", post(interject))
        .route("/v1/sessions/{id}/cancel", post(cancel_turn))
        .route("/v1/sessions/{id}/refresh-client", post(refresh_client))
        .route("/v1/sessions/{id}/review", post(run_review))
        .route("/v1/sessions/{id}/loop", post(run_loop))
        .route("/v1/questions/{id}/answer", post(answer_question))
        .route("/v1/approvals/{id}/answer", post(answer_approval))
        .route("/v1/model", post(set_model))
        .route("/v1/models", get(list_models))
        .route("/v1/connection", get(get_connection).put(put_connection))
        .route("/v1/attachments", post(upload_attachment))
        .with_state(state)
}

/// A running server bound to a concrete address.
pub struct ServerHandle {
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl ServerHandle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Serve on `addr`, returning once bound. The server runs until the handle
/// drops.
pub async fn serve(addr: SocketAddr, config: ServerConfig) -> Result<ServerHandle, String> {
    let router = build_router(config);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("could not bind {addr}: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(ServerHandle { addr, task })
}

/// Serve on 127.0.0.1 with an OS-assigned port (tests, embedding).
pub async fn serve_on_ephemeral_port(config: ServerConfig) -> Result<ServerHandle, String> {
    serve(([127, 0, 0, 1], 0).into(), config).await
}

// --- Auth --------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct AuthQuery {
    token: Option<String>,
    session: Option<String>,
}

/// Check the bearer token (header, or `?token=` for EventSource).
fn authorize(state: &AppState, headers: &HeaderMap, query_token: Option<&str>) -> Result<(), ApiError> {
    let header_token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if header_token == Some(state.token.as_str()) || query_token == Some(state.token.as_str()) {
        return Ok(());
    }
    Err(ApiError(
        StatusCode::UNAUTHORIZED,
        "missing or invalid bearer token".into(),
    ))
}

/// An error a route returns: status + plain message, serialized as
/// `{"error": …}` so clients have one failure shape.
struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

impl From<String> for ApiError {
    fn from(message: String) -> Self {
        ApiError(StatusCode::BAD_REQUEST, message)
    }
}

type ApiResult<T> = Result<T, ApiError>;

// --- Routes --------------------------------------------------------------------

async fn health(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&state, &headers, None)?;
    Ok(Json(serde_json::json!({
        "status": "ok",
        "protocol": "v1",
    })))
}

/// The SSE protocol-event stream: every session's events (or one session's,
/// with `?session=`), with `Last-Event-ID` replay on reconnect.
async fn events(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
) -> ApiResult<impl IntoResponse> {
    authorize(&state, &headers, query.token.as_deref())?;
    // `Last-Event-ID` (set by EventSource on reconnect) replays everything the
    // client missed; a fresh connection without it starts live-only.
    let after_id = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(u64::MAX);
    let (backlog, rx) = state.sink.subscribe(after_id);
    let stream = event_stream(backlog, rx, query.session);
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// The subscription as an SSE event stream: backlog first, then the live
/// feed, deduped by id and filtered by session. Runs as a forwarding task so
/// the broadcast receiver's lag handling stays plain `async` code.
fn event_stream(
    backlog: Vec<Numbered>,
    mut rx: broadcast::Receiver<Numbered>,
    session_filter: Option<String>,
) -> impl futures_core::Stream<Item = Result<Event, std::convert::Infallible>> {
    use tokio_stream::StreamExt;

    let (tx, out) = tokio::sync::mpsc::channel::<Event>(64);
    tokio::spawn(async move {
        let wanted = |event: &ProtocolEvent| match (&session_filter, event.session()) {
            (Some(filter), Some(session)) => filter == session,
            // App-wide events (local model status, downloads) go to everyone.
            (_, None) => true,
            (None, Some(_)) => true,
        };
        let mut last_sent = 0u64;
        for (id, event) in backlog {
            if wanted(&event) && tx.send(sse_event(id, &event)).await.is_err() {
                return;
            }
            last_sent = id;
        }
        loop {
            match rx.recv().await {
                Ok((id, event)) => {
                    // The subscribe-then-snapshot order means an event can
                    // arrive on both paths; the id dedupes it.
                    if id <= last_sent {
                        continue;
                    }
                    last_sent = id;
                    if wanted(&event) && tx.send(sse_event(id, &event)).await.is_err() {
                        return;
                    }
                }
                // Lagged: this client missed events mid-stream; it can
                // reconnect with Last-Event-ID to replay. Keep going live.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    tokio_stream::wrappers::ReceiverStream::new(out).map(Ok)
}

fn sse_event(id: u64, event: &ProtocolEvent) -> Event {
    let data = serde_json::to_string(event).unwrap_or_else(|_| "{}".into());
    Event::default().id(id.to_string()).data(data)
}

async fn list_sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<harness_store::SessionSummary>>> {
    authorize(&state, &headers, None)?;
    Ok(Json(state.service.list_sessions()?))
}

async fn new_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<harness_protocol::SessionInfo>> {
    authorize(&state, &headers, None)?;
    Ok(Json(state.service.new_session().await?))
}

async fn resume_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<harness_protocol::SessionView>> {
    authorize(&state, &headers, None)?;
    Ok(Json(state.service.resume_session(&id).await?))
}

async fn delete_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    authorize(&state, &headers, None)?;
    state.service.delete_session(&id).await?;
    Ok(StatusCode::OK)
}

async fn session_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    authorize(&state, &headers, None)?;
    Ok(Json(state.service.session_messages(&id)?))
}

async fn run_turn(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<harness_protocol::TurnRequest>,
) -> ApiResult<Json<harness_protocol::TurnResponse>> {
    authorize(&state, &headers, None)?;
    let text = state
        .service
        .run_turn(&id, request.prompt, request.attachments)
        .await?;
    Ok(Json(harness_protocol::TurnResponse { text }))
}

async fn retry_turn(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<harness_protocol::TurnResponse>> {
    authorize(&state, &headers, None)?;
    let text = state.service.retry_turn(&id).await?;
    Ok(Json(harness_protocol::TurnResponse { text }))
}

async fn interject(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<harness_protocol::InterjectRequest>,
) -> ApiResult<Json<harness_protocol::InterjectResponse>> {
    authorize(&state, &headers, None)?;
    let accepted = state.service.interject(&id, request.text).await;
    Ok(Json(harness_protocol::InterjectResponse { accepted }))
}

async fn cancel_turn(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    authorize(&state, &headers, None)?;
    state.service.cancel_turn(&id).await;
    Ok(StatusCode::OK)
}

async fn refresh_client(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    authorize(&state, &headers, None)?;
    state.service.refresh_client(&id).await?;
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
struct ReviewRequest {
    #[serde(default)]
    base_branch: Option<String>,
}

async fn run_review(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<ReviewRequest>,
) -> ApiResult<Json<harness_protocol::ReviewResult>> {
    authorize(&state, &headers, None)?;
    Ok(Json(
        state.service.run_code_review(&id, request.base_branch).await?,
    ))
}

#[derive(Deserialize)]
struct LoopRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    goal: Option<String>,
}

async fn run_loop(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<LoopRequest>,
) -> ApiResult<Json<harness_protocol::LoopResult>> {
    authorize(&state, &headers, None)?;
    Ok(Json(
        state
            .service
            .run_loop(&id, request.name, request.goal)
            .await?,
    ))
}

#[derive(Deserialize)]
struct AnswerQuestionRequest {
    answers: Vec<harness_protocol::QuestionAnswer>,
}

async fn answer_question(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<AnswerQuestionRequest>,
) -> ApiResult<StatusCode> {
    authorize(&state, &headers, None)?;
    state.service.answer_question(&id, request.answers);
    Ok(StatusCode::OK)
}

async fn answer_approval(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(answer): Json<harness_protocol::ApprovalAnswer>,
) -> ApiResult<StatusCode> {
    authorize(&state, &headers, None)?;
    state.service.answer_approval(&id, answer);
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
struct SetModelRequest {
    model: String,
}

async fn set_model(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<SetModelRequest>,
) -> ApiResult<Json<harness_protocol::SessionInfo>> {
    authorize(&state, &headers, None)?;
    Ok(Json(state.service.set_model(&request.model).await?))
}

async fn list_models(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<harness_runtime::models::CloudModel>>> {
    authorize(&state, &headers, None)?;
    Ok(Json(harness_runtime::models::catalog()))
}

async fn get_connection(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<harness_runtime::connection::ConnectionView>> {
    authorize(&state, &headers, None)?;
    Ok(Json(harness_runtime::connection::view()))
}

#[derive(Deserialize)]
struct ConnectionRequest {
    #[serde(default)]
    host: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    brave_api_key: String,
}

async fn put_connection(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ConnectionRequest>,
) -> ApiResult<StatusCode> {
    authorize(&state, &headers, None)?;
    harness_runtime::connection::save(&request.host, &request.api_key, &request.brave_api_key)
        .map_err(|e| e.to_string())?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct UploadQuery {
    #[serde(default)]
    filename: Option<String>,
}

/// Accept raw attachment bytes and stash them server-side; the returned path
/// goes into a later turn's `attachments`.
async fn upload_attachment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UploadQuery>,
    body: axum::body::Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&state, &headers, None)?;
    let filename = sanitize_filename(query.filename.as_deref().unwrap_or("attachment.bin"));
    std::fs::create_dir_all(&state.uploads_dir)
        .map_err(|e| format!("could not create uploads dir: {e}"))?;
    let unique = format!(
        "{:x}-{filename}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let path = state.uploads_dir.join(unique);
    std::fs::write(&path, &body).map_err(|e| format!("could not save attachment: {e}"))?;
    Ok(Json(serde_json::json!({
        "path": path.display().to_string(),
        "bytes": body.len(),
    })))
}

/// Keep only the basename and safe characters — an uploaded filename must
/// never traverse out of the uploads directory.
fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("attachment.bin");
    let safe: String = base
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    if safe.is_empty() {
        "attachment.bin".into()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_filename;

    #[test]
    fn uploaded_filenames_cannot_traverse() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("..\\..\\boot.ini"), "boot.ini");
        assert_eq!(sanitize_filename("note.txt"), "note.txt");
        assert_eq!(sanitize_filename("we ird$/na me.png"), "name.png");
        assert_eq!(sanitize_filename("///"), "attachment.bin");
    }
}
