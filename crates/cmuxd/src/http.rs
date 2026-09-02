//! Optional HTTP + WebSocket surface for the daemon.
//!
//! The unix socket speaks a length-prefixed JSON protocol that only `cmux`
//! implements, so reading a session or driving it meant writing a client. This
//! module puts the same daemon behind plain HTTP: list sessions, read what is
//! on a session's screen, send input, stream output live, and open any of it
//! in a browser.
//!
//! ## There is no authentication here, deliberately
//!
//! Anything that can reach the port can spawn and drive arbitrary commands as
//! the daemon's user. Deciding who may reach it is the operator's job, handled
//! by whatever fronts the port — an SSH tunnel, a peer-to-peer tunnel, a
//! reverse proxy that authenticates. The daemon binds where it is told and
//! serves; it does not try to be a second, weaker copy of that.
//!
//! The default bind is loopback, which is not a security control but the
//! address a tunnel connects to.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, body::Bytes};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::Registry;

/// Bind and serve in the background. Returns the address actually bound, so a
/// caller can pass port 0 and still print a usable URL.
pub(crate) async fn serve(
    addr: &str,
    registry: Arc<Registry>,
    mut shutdown: broadcast::Receiver<()>,
) -> Result<SocketAddr> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    let bound = listener.local_addr().context("local_addr")?;
    let app = router(registry);
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

fn router(registry: Arc<Registry>) -> Router {
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
        .route("/fonts/{name}", get(font))
        .with_state(registry)
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

/// Fonts compiled into the binary. The page cannot assume the device viewing
/// it has a Nerd Font — a phone will not — and a statusline built from
/// Powerline separators and Nerd icons renders as tofu without one. Serving
/// them from the host's font directories would only work on the host, so they
/// travel with the daemon. Regenerate with `scripts/vendor-fonts.sh`.
fn embedded_font(name: &str) -> Option<&'static [u8]> {
    match name {
        "mono.woff2" => Some(include_bytes!("../assets/fonts/mono.woff2")),
        "mono-bold.woff2" => Some(include_bytes!("../assets/fonts/mono-bold.woff2")),
        "symbols.woff2" => Some(include_bytes!("../assets/fonts/symbols.woff2")),
        _ => None,
    }
}

async fn font(Path(name): Path<String>) -> Response {
    match embedded_font(&name) {
        Some(bytes) => (
            [
                (header::CONTENT_TYPE, "font/woff2"),
                (header::CACHE_CONTROL, "public, max-age=604800, immutable"),
            ],
            bytes,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, format!("no font {name}\n")).into_response(),
    }
}

#[derive(Serialize)]
struct Health {
    ok: bool,
    version: &'static str,
    protocol: u32,
    sessions: usize,
}

async fn health(State(registry): State<Arc<Registry>>) -> Json<Health> {
    Json(Health {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
        protocol: cmux_proto::PROTOCOL_VERSION,
        sessions: registry.sessions.lock().await.len(),
    })
}

async fn list_sessions(
    State(registry): State<Arc<Registry>>,
) -> Json<Vec<cmux_proto::SessionInfo>> {
    Json(registry.list().await)
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

async fn spawn_session(
    State(registry): State<Arc<Registry>>,
    Json(body): Json<SpawnBody>,
) -> Response {
    let cwd = body.cwd.map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
    });
    match crate::spawn_session(
        &registry, cwd, body.cmd, body.probe, body.label, body.rows, body.cols,
    )
    .await
    {
        Ok(info) => (StatusCode::CREATED, Json(info)).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e:#}\n")).into_response(),
    }
}

async fn get_session(State(registry): State<Arc<Registry>>, Path(id): Path<u64>) -> Response {
    match registry.get(id).await {
        Some(sess) => Json(sess.info()).into_response(),
        None => no_such_session(id),
    }
}

async fn delete_session(State(registry): State<Arc<Registry>>, Path(id): Path<u64>) -> Response {
    match registry.remove(id).await {
        Some(sess) => {
            sess.kill();
            StatusCode::NO_CONTENT.into_response()
        }
        None => no_such_session(id),
    }
}

/// The visible grid as plain text — the cheapest way to see what a session is
/// showing without speaking the protocol or rendering escape sequences.
async fn screen(State(registry): State<Arc<Registry>>, Path(id): Path<u64>) -> Response {
    let Some(sess) = registry.get(id).await else {
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
async fn buffer(State(registry): State<Arc<Registry>>, Path(id): Path<u64>) -> Response {
    match registry.get(id).await {
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
async fn input(
    State(registry): State<Arc<Registry>>,
    Path(id): Path<u64>,
    body: Bytes,
) -> Response {
    let Some(sess) = registry.get(id).await else {
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

/// Sets the size used while nothing is attached. A one-shot HTTP call has no
/// attachment to speak for, so it cannot join the minimum that governs when
/// clients are connected — saying so beats silently doing nothing.
async fn resize(
    State(registry): State<Arc<Registry>>,
    Path(id): Path<u64>,
    Json(body): Json<ResizeBody>,
) -> Response {
    let Some(sess) = registry.get(id).await else {
        return no_such_session(id);
    };
    let attached = sess.attached_clients();
    if attached > 0 {
        return (
            StatusCode::CONFLICT,
            format!(
                "{attached} client(s) attached; the pty runs at the smallest of their sizes. \
                 Resize from the attached client instead — over the WebSocket, send \
                 `1{{\"rows\":R,\"cols\":C}}`.\n"
            ),
        )
            .into_response();
    }
    match sess.set_baseline_size(body.rows.max(1), body.cols.max(1)) {
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
    State(registry): State<Arc<Registry>>,
) -> Response {
    ws.on_upgrade(move |socket| pump(socket, id, registry))
}

/// Client → server messages carry a leading command byte, so a resize is not
/// mistaken for something to type into the shell:
///
///   `0` + bytes  input, passed to the pty verbatim
///   `1` + JSON   this client's grid size, `{"rows":R,"cols":C}`
///
/// Server → client, binary frames are pty output and text frames are JSON
/// control. The split matters for ordering: when the pty changes size the
/// control frame arrives before the output drawn at the new size, so the
/// client has already resized its grid by the time it has to render it.
fn on_client_message(sess: &crate::session::Session, client: u64, msg: &[u8]) {
    let Some((&cmd, rest)) = msg.split_first() else {
        return;
    };
    match cmd {
        b'0' => {
            let _ = sess.write_input(rest);
        }
        b'1' => match serde_json::from_slice::<ResizeBody>(rest) {
            Ok(sz) => {
                tracing::debug!(client, rows = sz.rows, cols = sz.cols, "client size");
                let _ = sess.set_client_size(client, sz.rows, sz.cols);
            }
            Err(e) => tracing::warn!(
                error = %e,
                payload = %String::from_utf8_lossy(rest),
                "bad resize message"
            ),
        },
        other => tracing::warn!(
            command = other,
            "unknown websocket command byte; message dropped"
        ),
    }
}

/// Bytes out, input in. Replays the ring first so a fresh tab is not blank.
async fn pump(mut socket: WebSocket, id: u64, registry: Arc<Registry>) {
    let Some(sess) = registry.get(id).await else {
        let _ = socket
            .send(Message::Text(format!("no session {id}").into()))
            .await;
        return;
    };
    // Each socket is its own client, so its size constrains the pty only for
    // as long as it stays attached.
    let client = registry.alloc_client_id();
    let mut rx = sess.bytes_tx.subscribe();
    let mut info_rx = sess.info_rx.clone();

    let (mut last_size, already_dead) = {
        let i = info_rx.borrow();
        ((i.rows, i.cols), (!i.alive).then(|| i.exit_status.clone()))
    };
    let mut announced_exit = false;
    if send_size(&mut socket, last_size).await.is_err() {
        return;
    }

    let opening = sess.attach_payload();
    if !opening.is_empty() && socket.send(Message::Binary(opening.into())).await.is_err() {
        return;
    }

    // A session that died before this tab opened never fires a watch change,
    // so say so here or the last screen looks like a live one.
    if let Some(status) = already_dead {
        announced_exit = true;
        if send_exited(&mut socket, status).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(t))) => on_client_message(&sess, client, t.as_bytes()),
                Some(Ok(Message::Binary(b))) => on_client_message(&sess, client, &b),
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
                    let repaint = sess.attach_payload();
                    if socket.send(Message::Binary(repaint.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            changed = info_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let (now, alive, exit) = {
                    let i = info_rx.borrow();
                    ((i.rows, i.cols), i.alive, i.exit_status.clone())
                };
                if !alive && !announced_exit {
                    announced_exit = true;
                    if send_exited(&mut socket, exit).await.is_err() {
                        break;
                    }
                }
                if now != last_size {
                    last_size = now;
                    // Size first, then a repaint at that size. A client that
                    // learned the new size only on its next poll would render
                    // the program's redraw into the old grid and wrap it.
                    if send_size(&mut socket, now).await.is_err() {
                        break;
                    }
                    let repaint = sess.attach_payload();
                    if socket.send(Message::Binary(repaint.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
    // Tab closed: stop holding the grid down to this client's size.
    sess.drop_client(client);
}

async fn send_size(socket: &mut WebSocket, (rows, cols): (u16, u16)) -> Result<(), ()> {
    let msg = format!(r#"{{"type":"size","rows":{rows},"cols":{cols}}}"#);
    socket.send(Message::Text(msg.into())).await.map_err(|_| ())
}

/// Built through serde_json because the status carries an OS error string,
/// which can hold a quote and would otherwise break the frame.
async fn send_exited(socket: &mut WebSocket, status: Option<String>) -> Result<(), ()> {
    let msg = serde_json::json!({
        "type": "exited",
        "status": status.unwrap_or_else(|| "exited".to_string()),
    })
    .to_string();
    socket.send(Message::Text(msg.into())).await.map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Serve the router on an ephemeral port. The returned sender must be kept
    /// alive: dropping it closes the channel, which fires the graceful-shutdown
    /// future and stops the server mid-test.
    async fn spawn_server() -> (SocketAddr, broadcast::Sender<()>) {
        let (addr, tx, _registry) = spawn_server_with_registry().await;
        (addr, tx)
    }

    /// Same, but hands back the registry for tests that need to reach a
    /// session directly rather than over HTTP.
    async fn spawn_server_with_registry() -> (SocketAddr, broadcast::Sender<()>, Arc<Registry>) {
        let registry = Arc::new(Registry::new());
        let (tx, rx) = broadcast::channel::<()>(1);
        let addr = serve("127.0.0.1:0", registry.clone(), rx)
            .await
            .expect("serve");
        (addr, tx, registry)
    }

    /// Minimal HTTP/1.1 GET, so the test needs no client dependency.
    async fn get(addr: SocketAddr, path: &str) -> String {
        let mut s = tokio::net::TcpStream::connect(addr).await.expect("connect");
        let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.expect("write");
        let mut out = String::new();
        s.read_to_string(&mut out).await.expect("read");
        out
    }

    /// The contract after auth was removed: no credential is asked for, and
    /// none is required. Access control belongs to whatever fronts the port.
    #[tokio::test]
    async fn every_route_answers_without_credentials() {
        let (addr, _keepalive) = spawn_server().await;

        let health = get(addr, "/api/health").await;
        assert!(health.starts_with("HTTP/1.1 200"), "{health}");
        assert!(health.contains("\"ok\":true"), "{health}");
        assert!(
            !health.to_lowercase().contains("www-authenticate"),
            "must not challenge for credentials: {health}"
        );

        let list = get(addr, "/api/sessions").await;
        assert!(list.starts_with("HTTP/1.1 200"), "{list}");
        assert!(list.contains("[]"), "empty registry lists nothing: {list}");

        let page = get(addr, "/").await;
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(page.contains("<title>cmuxd</title>"), "{page}");
    }

    /// The page must not depend on the viewing device having these fonts, so
    /// the binary has to actually carry them.
    #[test]
    fn fonts_are_compiled_into_the_binary() {
        for name in ["mono.woff2", "mono-bold.woff2", "symbols.woff2"] {
            let bytes = embedded_font(name).unwrap_or_else(|| panic!("{name} is not embedded"));
            assert_eq!(&bytes[..4], b"wOF2", "{name} is not a woff2 file");
            assert!(
                bytes.len() > 10_000,
                "{name} is only {} bytes; the vendored file looks wrong",
                bytes.len()
            );
        }
        assert!(embedded_font("../../../etc/passwd").is_none());
        assert!(embedded_font("nope.woff2").is_none());
    }

    #[tokio::test]
    async fn fonts_are_served() {
        let (addr, _keepalive) = spawn_server().await;
        let mut s = tokio::net::TcpStream::connect(addr).await.expect("connect");
        s.write_all(
            b"GET /fonts/symbols.woff2 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await
        .expect("write");
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.expect("read");
        let head = String::from_utf8_lossy(&out[..out.len().min(300)]).to_string();
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(head.contains("font/woff2"), "{head}");
        assert!(out.len() > 100_000, "body was only {} bytes", out.len());

        let missing = get(addr, "/fonts/nope.woff2").await;
        assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");
    }

    #[tokio::test]
    async fn an_unknown_session_is_a_404() {
        let (addr, _keepalive) = spawn_server().await;
        let res = get(addr, "/api/sessions/42/screen").await;
        assert!(res.starts_with("HTTP/1.1 404"), "{res}");
    }

    /// One request with a body, so the tests need no HTTP client dependency.
    async fn send(addr: SocketAddr, method: &str, path: &str, ctype: &str, body: &str) -> String {
        let mut s = tokio::net::TcpStream::connect(addr).await.expect("connect");
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
             Content-Length: {}\r\n",
            body.len()
        );
        if !ctype.is_empty() {
            req.push_str(&format!("Content-Type: {ctype}\r\n"));
        }
        req.push_str("\r\n");
        req.push_str(body);
        s.write_all(req.as_bytes()).await.expect("write");
        let mut out = String::new();
        s.read_to_string(&mut out).await.expect("read");
        out
    }

    async fn post_json(addr: SocketAddr, path: &str, body: &str) -> String {
        send(addr, "POST", path, "application/json", body).await
    }

    fn status(res: &str) -> &str {
        res.lines().next().unwrap_or("")
    }

    /// Poll a route until its body holds `needle`, or give up. Output arrives
    /// through a pty read thread, so there is nothing to await on directly.
    async fn wait_for(addr: SocketAddr, path: &str, needle: &str) -> String {
        for _ in 0..60 {
            let res = get(addr, path).await;
            if res.contains(needle) {
                return res;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        get(addr, path).await
    }

    async fn spawn_over_http(addr: SocketAddr, label: &str, argv: &[&str]) -> String {
        let cmd = argv
            .iter()
            .map(|a| format!("{:?}", a))
            .collect::<Vec<_>>()
            .join(",");
        let res = post_json(
            addr,
            "/api/sessions",
            &format!(r#"{{"cmd":[{cmd}],"cwd":"/tmp","label":"{label}"}}"#),
        )
        .await;
        assert!(
            res.starts_with("HTTP/1.1 201"),
            "spawning {label} failed: {res}"
        );
        res
    }

    #[tokio::test]
    async fn a_session_spawned_over_http_is_listed() {
        let (addr, _keepalive) = spawn_server().await;
        spawn_over_http(addr, "web", &["/bin/sleep", "30"]).await;

        let list = get(addr, "/api/sessions").await;
        assert!(list.contains("\"label\":\"web\""), "{list}");
        assert!(list.contains("\"alive\":true"), "{list}");

        let one = get(addr, "/api/sessions/1").await;
        assert!(one.starts_with("HTTP/1.1 200"), "{one}");
        assert!(one.contains("\"cwd\":\"/tmp\""), "{one}");
    }

    #[tokio::test]
    async fn spawning_an_unknown_program_is_a_bad_request_naming_it() {
        let (addr, _keepalive) = spawn_server().await;
        let res = post_json(
            addr,
            "/api/sessions",
            r#"{"cmd":["definitely-not-a-real-binary"],"cwd":"/tmp"}"#,
        )
        .await;
        assert_eq!(status(&res), "HTTP/1.1 400 Bad Request", "{res}");
        assert!(res.contains("definitely-not-a-real-binary"), "{res}");
        assert!(get(addr, "/api/sessions").await.contains("[]"), "it stuck");
    }

    #[tokio::test]
    async fn a_spawn_body_without_a_size_gets_the_default_grid() {
        let (addr, _keepalive) = spawn_server().await;
        let res = post_json(
            addr,
            "/api/sessions",
            r#"{"cmd":["/bin/sleep","30"],"cwd":"/tmp"}"#,
        )
        .await;
        assert!(res.contains("\"rows\":24"), "{res}");
        assert!(res.contains("\"cols\":80"), "{res}");
    }

    #[tokio::test]
    async fn a_malformed_spawn_body_is_refused() {
        let (addr, _keepalive) = spawn_server().await;
        for body in [r#"{"cmd":"not a list"}"#, "{not json at all", "{}"] {
            let res = post_json(addr, "/api/sessions", body).await;
            assert!(
                status(&res).starts_with("HTTP/1.1 4"),
                "body {body:?} should be refused, got: {}",
                status(&res)
            );
        }
    }

    #[tokio::test]
    async fn deleting_a_session_removes_it_and_says_so_only_once() {
        let (addr, _keepalive) = spawn_server().await;
        spawn_over_http(addr, "doomed", &["/bin/sleep", "30"]).await;

        let first = send(addr, "DELETE", "/api/sessions/1", "", "").await;
        assert_eq!(status(&first), "HTTP/1.1 204 No Content", "{first}");
        assert!(get(addr, "/api/sessions").await.contains("[]"));

        let second = send(addr, "DELETE", "/api/sessions/1", "", "").await;
        assert_eq!(status(&second), "HTTP/1.1 404 Not Found", "{second}");
    }

    #[tokio::test]
    async fn the_screen_route_shows_what_the_session_printed() {
        let (addr, _keepalive) = spawn_server().await;
        spawn_over_http(
            addr,
            "printer",
            &["/bin/sh", "-c", "echo SCREEN-MARKER; sleep 30"],
        )
        .await;

        let screen = wait_for(addr, "/api/sessions/1/screen", "SCREEN-MARKER").await;
        assert!(screen.contains("text/plain"), "{screen}");
        assert!(screen.contains("SCREEN-MARKER"), "{screen}");

        let buffer = wait_for(addr, "/api/sessions/1/buffer", "SCREEN-MARKER").await;
        assert!(buffer.contains("application/octet-stream"), "{buffer}");
        assert!(buffer.contains("SCREEN-MARKER"), "{buffer}");
    }

    #[tokio::test]
    async fn posted_input_reaches_the_pty() {
        let (addr, _keepalive) = spawn_server().await;
        spawn_over_http(
            addr,
            "shell",
            &["/bin/sh", "-c", "read line; echo GOT-$line"],
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;

        let res = send(addr, "POST", "/api/sessions/1/input", "", "typed\n").await;
        assert_eq!(status(&res), "HTTP/1.1 204 No Content", "{res}");

        let screen = wait_for(addr, "/api/sessions/1/screen", "GOT-typed").await;
        assert!(
            screen.contains("GOT-typed"),
            "input never arrived: {screen}"
        );
    }

    #[tokio::test]
    async fn resizing_works_while_nothing_is_attached() {
        let (addr, _keepalive) = spawn_server().await;
        spawn_over_http(addr, "sizer", &["/bin/sleep", "30"]).await;

        let res = post_json(addr, "/api/sessions/1/resize", r#"{"rows":30,"cols":100}"#).await;
        assert_eq!(status(&res), "HTTP/1.1 204 No Content", "{res}");

        let info = get(addr, "/api/sessions/1").await;
        assert!(info.contains("\"rows\":30"), "{info}");
        assert!(info.contains("\"cols\":100"), "{info}");
    }

    /// The other half: with a client attached, the pty runs at the minimum of
    /// the attached sizes, so a one-shot resize has no attachment to speak for.
    #[tokio::test]
    async fn resizing_is_refused_while_a_client_is_attached() {
        let (addr, _keepalive, registry) = spawn_server_with_registry().await;
        spawn_over_http(addr, "shared", &["/bin/sleep", "30"]).await;
        let sess = registry.get(1).await.expect("the session");
        sess.set_client_size(registry.alloc_client_id(), 20, 60)
            .expect("attach");

        let res = post_json(addr, "/api/sessions/1/resize", r#"{"rows":30,"cols":100}"#).await;
        assert_eq!(status(&res), "HTTP/1.1 409 Conflict", "{res}");
        assert!(res.contains("client(s) attached"), "{res}");

        let info = get(addr, "/api/sessions/1").await;
        assert!(
            info.contains("\"rows\":20"),
            "the refused resize still took effect: {info}"
        );
    }

    #[tokio::test]
    async fn health_reports_the_protocol_version_and_the_session_count() {
        let (addr, _keepalive) = spawn_server().await;
        let empty = get(addr, "/api/health").await;
        assert!(
            empty.contains(&format!("\"protocol\":{}", cmux_proto::PROTOCOL_VERSION)),
            "{empty}"
        );
        assert!(empty.contains("\"sessions\":0"), "{empty}");

        spawn_over_http(addr, "one", &["/bin/sleep", "30"]).await;
        let one = get(addr, "/api/health").await;
        assert!(one.contains("\"sessions\":1"), "{one}");
    }

    /// The daemon reaps the child and reports how it went, so a browser can
    /// tell a finished session from an idle one.
    #[tokio::test]
    async fn a_finished_session_is_reported_as_exited() {
        let (addr, _keepalive) = spawn_server().await;
        spawn_over_http(addr, "shortlived", &["/bin/sh", "-c", "exit 3"]).await;

        let info = wait_for(addr, "/api/sessions/1", "\"alive\":false").await;
        assert!(info.contains("\"alive\":false"), "{info}");
        assert!(info.contains("\"exit_status\":\"exited 3\""), "{info}");
        assert!(
            get(addr, "/api/sessions").await.contains("shortlived"),
            "a finished session should stay listed so it can be seen"
        );
    }

    #[tokio::test]
    async fn writing_routes_need_no_credentials_either() {
        let (addr, _keepalive) = spawn_server().await;
        let res = post_json(
            addr,
            "/api/sessions",
            r#"{"cmd":["/bin/sleep","30"],"cwd":"/tmp"}"#,
        )
        .await;
        assert!(res.starts_with("HTTP/1.1 201"), "{res}");
        assert!(
            !res.to_lowercase().contains("www-authenticate"),
            "must not challenge for credentials: {res}"
        );
    }
}
