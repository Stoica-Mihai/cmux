//! Optional HTTP + WebSocket surface for the daemon.
//!
//! The unix socket speaks a length-prefixed JSON protocol that only `cmux`
//! implements, so reading a session or driving it meant writing a client. This
//! module puts the same daemon behind plain HTTP: list sessions, read what is
//! on a session's screen, send input, stream output live, and open any of it
//! in a browser.
//!
//! Off unless `--http` is passed. It binds loopback by default and requires a
//! bearer token, because a session is an arbitrary command — reaching this API
//! is equivalent to running code as the daemon's user.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, body::Bytes};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::Registry;

#[derive(Clone)]
struct HttpState {
    registry: Arc<Registry>,
    token: Arc<String>,
}

/// 32 bytes of urandom, hex encoded.
pub(crate) fn generate_token() -> Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom").context("open /dev/urandom")?;
    let mut buf = [0u8; 32];
    f.read_exact(&mut buf).context("read /dev/urandom")?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Bind and serve in the background. Returns the address actually bound, so a
/// caller can pass port 0 and still print a usable URL.
pub(crate) async fn serve(
    addr: &str,
    registry: Arc<Registry>,
    token: String,
    mut shutdown: broadcast::Receiver<()>,
) -> Result<SocketAddr> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    let bound = listener.local_addr().context("local_addr")?;
    let app = router(registry, token);
    tokio::spawn(async move {
        let served = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown.recv().await;
            })
            .await;
        if let Err(e) = served {
            tracing::warn!(error = %e, "http server stopped");
        }
    });
    Ok(bound)
}

fn router(registry: Arc<Registry>, token: String) -> Router {
    let state = HttpState {
        registry,
        token: Arc::new(token),
    };
    Router::new()
        .route("/", get(index))
        .route("/api/health", get(health))
        .route("/api/sessions", get(list_sessions).post(spawn_session))
        .route(
            "/api/sessions/{id}",
            get(get_session).delete(delete_session),
        )
        .route("/api/sessions/{id}/screen", get(screen))
        .route("/api/sessions/{id}/buffer", get(buffer))
        .route("/api/sessions/{id}/input", post(input))
        .route("/api/sessions/{id}/resize", post(resize))
        .route("/ws/sessions/{id}", get(ws_session))
        // Applied to the whole router rather than per handler, so a new route
        // cannot be added unauthenticated by forgetting a check.
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

/// Length-checked, non-short-circuiting compare.
fn tokens_match(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Bearer header, or `?token=` so a browser can open the page and a WebSocket
/// (neither of which can set headers).
fn supplied_token(req: &Request) -> Option<String> {
    let header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string);
    header.or_else(|| req.uri().query().and_then(|q| query_param(q, "token")))
}

async fn require_token(State(st): State<HttpState>, req: Request, next: Next) -> Response {
    match supplied_token(&req) {
        Some(t) if tokens_match(&t, &st.token) => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            "missing or invalid token; pass Authorization: Bearer <token> or ?token=<token>\n",
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

fn no_such_session(id: u64) -> Response {
    (StatusCode::NOT_FOUND, format!("no session {id}\n")).into_response()
}

async fn index() -> Html<&'static str> {
    Html(include_str!("terminal.html"))
}

#[derive(Serialize)]
struct Health {
    ok: bool,
    version: &'static str,
    protocol: u32,
    sessions: usize,
}

async fn health(State(st): State<HttpState>) -> Json<Health> {
    Json(Health {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
        protocol: cmux_proto::PROTOCOL_VERSION,
        sessions: st.registry.sessions.lock().await.len(),
    })
}

async fn list_sessions(State(st): State<HttpState>) -> Json<Vec<cmux_proto::SessionInfo>> {
    Json(st.registry.list().await)
}

#[derive(Deserialize)]
struct SpawnBody {
    cmd: Vec<String>,
    cwd: Option<String>,
    label: Option<String>,
    #[serde(default)]
    probe: cmux_proto::ProbeKind,
    #[serde(default = "default_rows")]
    rows: u16,
    #[serde(default = "default_cols")]
    cols: u16,
}

fn default_rows() -> u16 {
    24
}
fn default_cols() -> u16 {
    80
}

async fn spawn_session(State(st): State<HttpState>, Json(body): Json<SpawnBody>) -> Response {
    let cwd = body.cwd.map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
    });
    match crate::spawn_session(
        &st.registry,
        cwd,
        body.cmd,
        body.probe,
        body.label,
        body.rows,
        body.cols,
    )
    .await
    {
        Ok(info) => (StatusCode::CREATED, Json(info)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e:#}\n")).into_response(),
    }
}

async fn get_session(State(st): State<HttpState>, Path(id): Path<u64>) -> Response {
    match st.registry.get(id).await {
        Some(sess) => Json(sess.info()).into_response(),
        None => no_such_session(id),
    }
}

async fn delete_session(State(st): State<HttpState>, Path(id): Path<u64>) -> Response {
    match st.registry.remove(id).await {
        Some(sess) => {
            sess.kill();
            StatusCode::NO_CONTENT.into_response()
        }
        None => no_such_session(id),
    }
}

/// The visible grid as plain text — the cheapest way to see what a session is
/// showing without speaking the protocol or rendering escape sequences.
async fn screen(State(st): State<HttpState>, Path(id): Path<u64>) -> Response {
    let Some(sess) = st.registry.get(id).await else {
        return no_such_session(id);
    };
    let text = match sess.term_state.lock() {
        Ok(t) => crate::probe::grid_text(&t.term),
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "terminal lock poisoned\n",
            )
                .into_response();
        }
    };
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response()
}

/// Raw replay ring: every byte the PTY produced, escape sequences included.
async fn buffer(State(st): State<HttpState>, Path(id): Path<u64>) -> Response {
    match st.registry.get(id).await {
        Some(sess) => (
            [(header::CONTENT_TYPE, "application/octet-stream")],
            sess.ring_snapshot(),
        )
            .into_response(),
        None => no_such_session(id),
    }
}

/// Body bytes go to the PTY verbatim, so escape sequences and control
/// characters work as typed.
async fn input(State(st): State<HttpState>, Path(id): Path<u64>, body: Bytes) -> Response {
    let Some(sess) = st.registry.get(id).await else {
        return no_such_session(id);
    };
    match sess.write_input(&body) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}\n")).into_response(),
    }
}

#[derive(Deserialize)]
struct ResizeBody {
    rows: u16,
    cols: u16,
}

async fn resize(
    State(st): State<HttpState>,
    Path(id): Path<u64>,
    Json(body): Json<ResizeBody>,
) -> Response {
    let Some(sess) = st.registry.get(id).await else {
        return no_such_session(id);
    };
    match sess.resize(body.rows.max(1), body.cols.max(1)) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}\n")).into_response(),
    }
}

// ---------------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------------

async fn ws_session(
    ws: WebSocketUpgrade,
    Path(id): Path<u64>,
    State(st): State<HttpState>,
) -> Response {
    ws.on_upgrade(move |socket| pump(socket, id, st))
}

/// Bytes out, input in. Replays the ring first so a fresh tab is not blank.
async fn pump(mut socket: WebSocket, id: u64, st: HttpState) {
    let Some(sess) = st.registry.get(id).await else {
        let _ = socket
            .send(Message::Text(format!("no session {id}").into()))
            .await;
        return;
    };
    let mut rx = sess.bytes_tx.subscribe();

    let ring = sess.ring_snapshot();
    if !ring.is_empty() && socket.send(Message::Binary(ring.into())).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(t))) => { let _ = sess.write_input(t.as_bytes()); }
                Some(Ok(Message::Binary(b))) => { let _ = sess.write_input(&b); }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => break,
            },
            chunk = rx.recv() => match chunk {
                Ok(bytes) => {
                    if socket.send(Message::Binary(bytes.into())).await.is_err() {
                        break;
                    }
                }
                // Fell behind the broadcast queue: repaint rather than
                // deliver a stream with a hole in it.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let ring = sess.ring_snapshot();
                    if socket.send(Message::Binary(ring.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_match_only_on_an_exact_match() {
        assert!(tokens_match("abc123", "abc123"));
        assert!(!tokens_match("abc123", "abc124"));
        // A prefix must not pass, which a length-blind compare would allow.
        assert!(!tokens_match("abc", "abc123"));
        assert!(!tokens_match("abc123", "abc"));
        assert!(!tokens_match("", "abc"));
        assert!(tokens_match("", ""));
    }

    #[test]
    fn query_param_picks_the_right_key() {
        assert_eq!(query_param("token=xyz", "token").as_deref(), Some("xyz"));
        assert_eq!(
            query_param("a=1&token=xyz&b=2", "token").as_deref(),
            Some("xyz")
        );
        assert_eq!(query_param("a=1&b=2", "token"), None);
        // A key that merely contains "token" must not match.
        assert_eq!(query_param("mytoken=xyz", "token"), None);
        assert_eq!(query_param("", "token"), None);
    }

    #[test]
    fn generated_tokens_are_long_and_unique() {
        let a = generate_token().expect("token");
        let b = generate_token().expect("token");
        assert_eq!(a.len(), 64, "32 bytes hex encoded");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
