//! `cmuxd` — long-lived daemon owning one PTY per session.
//!
//! The daemon is command-agnostic: it execs whatever argv a client sends, and
//! delegates anything it can say about the child to a [`probe`].

mod http;
mod probe;
mod session;
mod snapshot;

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use cmux_proto::{Event, FrameError, PROTOCOL_VERSION, Request};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal;
use tokio::sync::{Mutex, broadcast, mpsc};

use crate::session::Session;

const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:7070";

const USAGE: &str = "\
cmuxd — the cmux session daemon

Usage: cmuxd [--http [ADDR]]

Hosts one PTY per session, on $XDG_RUNTIME_DIR/cmux/cmuxd.sock (fallback
/tmp/cmux-<uid>/<home>/cmux). Drive it with `cmux --connect` or `cmux ctl`.

  --http [ADDR]  also serve the HTTP + WebSocket API, default 127.0.0.1:7070.
                 It has NO authentication: whoever reaches the port runs
                 commands as you. Front it with a tunnel or an authenticating
                 proxy rather than binding it somewhere reachable.
  -h, --help     print this message
  -V, --version  print the version

Environment:
  CMUXD_LOG      tracing filter, e.g. `debug` or `cmuxd=trace,info`";

/// Runtime options. Only `--http` is configurable; everything else is derived.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) http: Option<String>,
}

/// What the daemon should do about its argv.
#[derive(Debug, PartialEq, Eq)]
enum Cli {
    Run(Config),
    Print(String),
    Reject(String),
}

/// cmuxd once ignored argv entirely, so `cmuxd --help` silently started a
/// daemon and blocked.
fn parse_cli<I: IntoIterator<Item = String>>(args: I) -> Cli {
    let mut cfg = Config::default();
    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Cli::Print(USAGE.to_string()),
            "-V" | "--version" => return Cli::Print(format!("cmuxd {SERVER_VERSION}")),
            // The address is optional, so only consume the next token when it
            // is not itself a flag.
            "--http" => {
                let addr = match args.peek() {
                    Some(next) if !next.starts_with('-') => args.next(),
                    _ => None,
                };
                cfg.http = Some(addr.unwrap_or_else(|| DEFAULT_HTTP_ADDR.to_string()));
            }
            other => match other.strip_prefix("--http=") {
                Some(addr) if !addr.is_empty() => cfg.http = Some(addr.to_string()),
                _ => return Cli::Reject(format!("cmuxd: unexpected argument `{other}`")),
            },
        }
    }
    Cli::Run(cfg)
}

fn log_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("cmux"))
}

/// Initialize structured logging to `~/.local/state/cmux/cmuxd.log` with daily
/// rotation. Returns the appender guard so the caller can keep it alive for
/// the lifetime of the daemon — dropping it flushes buffered records.
///
/// Honors `CMUXD_LOG` for level/filter overrides (e.g. `CMUXD_LOG=debug`,
/// `CMUXD_LOG=cmuxd=trace,info`); defaults to `info` when unset.
fn init_logging() -> Option<(tracing_appender::non_blocking::WorkerGuard, PathBuf)> {
    use tracing_subscriber::EnvFilter;
    let dir = log_dir()?;
    let _ = fs::create_dir_all(&dir);
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("cmuxd")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env("CMUXD_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .with_target(false)
        .init();
    Some((guard, dir))
}

fn socket_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|h| fallback_socket_dir(nix_compat_uid(), &h.to_string_lossy()))
        })
        .context("no XDG_RUNTIME_DIR or HOME")?;
    Ok(base.join("cmux"))
}

/// Per-uid scratch dir for hosts with no `XDG_RUNTIME_DIR`, notably WSL
/// without `systemd=true`. `HOME` is absolute, and `PathBuf::join` discards
/// the base when handed an absolute path, so the leading slash must go.
fn fallback_socket_dir(uid: u32, home: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/cmux-{}", uid)).join(home.trim_start_matches('/'))
}

fn nix_compat_uid() -> u32 {
    if let Ok(s) = std::fs::read_to_string("/proc/self/status") {
        for line in s.lines() {
            if let Some(rest) = line.strip_prefix("Uid:")
                && let Some(tok) = rest.split_whitespace().next()
                && let Ok(uid) = tok.parse::<u32>()
            {
                return uid;
            }
        }
    }
    0
}

fn socket_path() -> Result<PathBuf> {
    Ok(socket_dir()?.join("cmuxd.sock"))
}

fn ready_path() -> Result<PathBuf> {
    Ok(socket_dir()?.join("cmuxd.ready"))
}

/// Daemon-global state shared across the unix socket and the HTTP API.
pub(crate) struct Registry {
    pub(crate) sessions: Mutex<HashMap<u64, Arc<Session>>>,
    next_id: AtomicU64,
    next_client_id: AtomicU64,
}

impl Registry {
    fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            next_client_id: AtomicU64::new(1),
        }
    }

    fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// One id per attachment — a socket connection or a WebSocket — so each
    /// client's requested grid size can be tracked separately.
    pub(crate) fn alloc_client_id(&self) -> u64 {
        self.next_client_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Forget a departed client everywhere, so a session it was holding small
    /// can grow back for whoever is left.
    pub(crate) async fn drop_client_everywhere(&self, client: u64) {
        for sess in self.sessions.lock().await.values() {
            sess.drop_client(client);
        }
    }

    /// Sorted by id. Clients number sessions by their position here, so
    /// HashMap iteration order would renumber every session on reconnect.
    pub(crate) async fn list(&self) -> Vec<cmux_proto::SessionInfo> {
        let sessions = self.sessions.lock().await;
        let mut out: Vec<cmux_proto::SessionInfo> = sessions.values().map(|s| s.info()).collect();
        out.sort_by_key(|s| s.id);
        out
    }

    async fn insert(&self, sess: Arc<Session>) {
        self.sessions.lock().await.insert(sess.id, sess);
    }

    pub(crate) async fn get(&self, id: u64) -> Option<Arc<Session>> {
        self.sessions.lock().await.get(&id).cloned()
    }

    pub(crate) async fn remove(&self, id: u64) -> Option<Arc<Session>> {
        self.sessions.lock().await.remove(&id)
    }
}

/// Spawn a session and register it. Both transports go through here, so they
/// cannot drift on default labelling, size clamping, or the status ticker.
pub(crate) async fn spawn_session(
    registry: &Arc<Registry>,
    cwd: PathBuf,
    cmd: Vec<String>,
    probe: cmux_proto::ProbeKind,
    label: Option<String>,
    rows: u16,
    cols: u16,
) -> Result<cmux_proto::SessionInfo> {
    let id = registry.alloc_id();
    let label = label.unwrap_or_else(|| {
        cwd.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| cwd.display().to_string())
    });
    let sess = Session::spawn(id, label, cwd, cmd, probe, rows.max(1), cols.max(1))
        .context("Session::spawn")?;
    registry.insert(sess.clone()).await;
    spawn_status_task(sess.clone());
    Ok(sess.info())
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let config = match parse_cli(std::env::args().skip(1)) {
        Cli::Run(cfg) => cfg,
        Cli::Print(text) => {
            println!("{text}");
            return Ok(());
        }
        Cli::Reject(msg) => {
            eprintln!("{msg}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    let _log_guard = init_logging();
    if let Some((_, ref p)) = _log_guard {
        tracing::info!(dir = %p.display(), "log rotation -> daily");
    }
    let dir = socket_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).ok();

    let sock = socket_path()?;
    if sock.exists() {
        if UnixStream::connect(&sock).await.is_ok() {
            anyhow::bail!(
                "another cmuxd already listening at {} — refusing to start",
                sock.display()
            );
        }
        let _ = fs::remove_file(&sock);
    }

    let listener = UnixListener::bind(&sock).with_context(|| format!("bind {}", sock.display()))?;
    fs::set_permissions(&sock, fs::Permissions::from_mode(0o600)).ok();

    let ready = ready_path()?;
    let _ = fs::write(&ready, std::process::id().to_string());

    tracing::info!(version = SERVER_VERSION, socket = %sock.display(), "cmuxd listening");

    let registry = Arc::new(Registry::new());
    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    if let Some(addr) = config.http.as_deref() {
        let bound = http::serve(addr, registry.clone(), shutdown_tx.subscribe())
            .await
            .context("start http api")?;
        tracing::info!(%bound, "http api listening");
        println!("cmuxd http api on http://{bound}");
        println!("  no authentication: whoever reaches this port runs commands as you.");
        println!("  front it with a tunnel or an authenticating proxy before exposing it.");
    }

    let accept_shutdown = shutdown_tx.clone();
    let accept_registry = registry.clone();
    let accept_task = tokio::spawn(async move {
        let mut shutdown_rx = accept_shutdown.subscribe();
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _addr)) => {
                            let reg = accept_registry.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(stream, reg).await {
                                    tracing::warn!(error = %e, "connection error");
                                }
                            });
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "accept error");
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    }
                }
            }
        }
    });

    tokio::select! {
        _ = signal::ctrl_c() => {
            tracing::info!(signal = "SIGINT", "shutting down");
        }
        _ = wait_sigterm() => {
            tracing::info!(signal = "SIGTERM", "shutting down");
        }
    }
    let _ = shutdown_tx.send(());
    let _ = accept_task.await;

    // kill all sessions on shutdown
    let sessions = registry.sessions.lock().await;
    for s in sessions.values() {
        s.kill();
    }
    drop(sessions);

    let _ = fs::remove_file(&sock);
    let _ = fs::remove_file(&ready);
    Ok(())
}

/// Spawn a per-session ticker that runs the session's status probe. A session
/// with no probe gets no ticker at all. Lives as long as the session does;
/// downgrades to a Weak so it exits cleanly when the session is dropped.
fn spawn_status_task(sess: std::sync::Arc<Session>) {
    if !sess.has_probe() {
        return;
    }
    let weak = std::sync::Arc::downgrade(&sess);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            interval.tick().await;
            let Some(sess) = weak.upgrade() else { break };
            if !sess.alive.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            sess.poll_status_once();
        }
    });
}

async fn wait_sigterm() {
    use signal::unix::{SignalKind, signal};
    if let Ok(mut sig) = signal(SignalKind::terminate()) {
        sig.recv().await;
    } else {
        std::future::pending::<()>().await;
    }
}

/// Send SIGTERM to the current process so the main loop's shutdown path runs.
/// Used by the Shutdown request handler.
/// Send SIGTERM to the current process so the main loop's shutdown path runs.
/// Used by the Shutdown request handler.
fn kill_self() {
    let _ = nix::sys::signal::raise(nix::sys::signal::Signal::SIGTERM);
}

async fn handle_connection(stream: UnixStream, registry: Arc<Registry>) -> Result<()> {
    let client_id = registry.alloc_client_id();
    let (mut read_half, write_half) = stream.into_split();

    // outbound queue: any task that wants to send an Event posts here, the writer
    // task drains and frames.
    let (event_tx, mut event_rx) = mpsc::channel::<Event>(256);
    let mut write_half = write_half;
    let writer_task = tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            if write_frame_async(&mut write_half, &ev).await.is_err() {
                break;
            }
        }
        let _ = write_half.shutdown().await;
    });

    // Hello/Welcome handshake
    let first: Request = match read_frame_async(&mut read_half).await {
        Ok(r) => r,
        Err(FrameError::Eof) => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let Request::Hello {
        client_version,
        want_protocol,
    } = first
    else {
        let _ = event_tx
            .send(Event::Goodbye {
                reason: "expected Hello as first message".to_string(),
            })
            .await;
        return Ok(());
    };
    if want_protocol != PROTOCOL_VERSION {
        let _ = event_tx
            .send(Event::Goodbye {
                reason: format!(
                    "protocol skew: client wants {}, server speaks {}",
                    want_protocol, PROTOCOL_VERSION
                ),
            })
            .await;
        return Ok(());
    }
    let sc = registry.sessions.lock().await.len();
    let _ = event_tx
        .send(Event::Welcome {
            server_version: SERVER_VERSION.to_string(),
            protocol: PROTOCOL_VERSION,
            session_count: sc,
        })
        .await;
    tracing::info!(client = %client_version, "handshake ok");

    // Track subscriptions per connection; cancel forwarders on Unsubscribe / drop.
    let mut subscriptions: HashMap<u64, tokio::task::JoinHandle<()>> = HashMap::new();

    let result: Result<()> = async {
        loop {
            match read_frame_async::<Request>(&mut read_half).await {
                Ok(req) => {
                    if let Err(e) =
                        dispatch(req, &registry, &event_tx, &mut subscriptions, client_id).await
                    {
                        let _ = event_tx
                            .send(Event::Error {
                                request_id: None,
                                // `{e:#}` walks the anyhow source chain; plain
                                // `{e}` reports only the outermost context, so
                                // a failed exec came back as "Session::spawn".
                                message: format!("{e:#}"),
                            })
                            .await;
                    }
                }
                Err(FrameError::Eof) => break,
                Err(e) => {
                    let _ = event_tx
                        .send(Event::Error {
                            request_id: None,
                            message: format!("frame error: {e}"),
                        })
                        .await;
                    break;
                }
            }
        }
        Ok(())
    }
    .await;

    for (_, h) in subscriptions.drain() {
        h.abort();
    }
    // This client is gone, so it must stop constraining any session's size.
    registry.drop_client_everywhere(client_id).await;
    drop(event_tx);
    let _ = writer_task.await;
    result
}

async fn dispatch(
    req: Request,
    registry: &Arc<Registry>,
    event_tx: &mpsc::Sender<Event>,
    subscriptions: &mut HashMap<u64, tokio::task::JoinHandle<()>>,
    client_id: u64,
) -> Result<()> {
    match req {
        Request::Hello { .. } => {
            // already greeted; ignore duplicates
        }
        Request::ListSessions => {
            let list = registry.list().await;
            let _ = event_tx.send(Event::SessionList { sessions: list }).await;
        }
        Request::SpawnSession {
            cwd,
            cmd,
            probe,
            label,
            rows,
            cols,
        } => {
            let info = spawn_session(registry, cwd, cmd, probe, label, rows, cols).await?;
            let id = info.id;
            let _ = event_tx.send(Event::SessionSpawned { id, info }).await;
        }
        Request::Attach { session_id, .. } => {
            let Some(sess) = registry.get(session_id).await else {
                anyhow::bail!("no such session {session_id}");
            };
            let payload = sess.attach_payload();
            if !payload.is_empty() {
                let _ = event_tx
                    .send(Event::FrameDelta {
                        id: session_id,
                        bytes: payload,
                    })
                    .await;
            }
        }
        Request::Detach {
            session_id,
            keep_session,
        } => {
            if let Some(h) = subscriptions.remove(&session_id) {
                h.abort();
            }
            if !keep_session && let Some(sess) = registry.remove(session_id).await {
                sess.kill();
                let _ = event_tx
                    .send(Event::SessionExited {
                        id: session_id,
                        status: "detached".into(),
                    })
                    .await;
            }
        }
        Request::Input { session_id, bytes } => {
            if let Some(sess) = registry.get(session_id).await {
                sess.write_input(&bytes)?;
            }
        }
        Request::Resize {
            session_id,
            rows,
            cols,
        } => {
            // This client's own size, not the session's. The PTY runs at the
            // minimum across everyone attached.
            if let Some(sess) = registry.get(session_id).await {
                sess.set_client_size(client_id, rows, cols)?;
            }
        }
        Request::Rename { session_id, label } => {
            if let Some(sess) = registry.get(session_id).await {
                sess.rename(label);
                let info = sess.info();
                let _ = event_tx
                    .send(Event::StatusUpdate {
                        id: session_id,
                        status: info.status,
                        label: Some(info.label),
                        attention: info.attention,
                        rows: info.rows,
                        cols: info.cols,
                    })
                    .await;
            }
        }
        Request::Scroll { .. } => {
            // Scrollback is client-side in the snapshot+delta architecture.
            // No-op on the daemon for now; TUI manipulates its own Term copy.
        }
        Request::Subscribe { session_id } => {
            if subscriptions.contains_key(&session_id) {
                return Ok(());
            }
            let Some(sess) = registry.get(session_id).await else {
                anyhow::bail!("no such session {session_id}");
            };
            // Byte fan-out
            let mut rx = sess.bytes_tx.subscribe();
            let tx = event_tx.clone();
            let h = tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(chunk) => {
                            if tx
                                .send(Event::FrameDelta {
                                    id: session_id,
                                    bytes: chunk,
                                })
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            let _ = tx.send(Event::Resync { id: session_id }).await;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            subscriptions.insert(session_id, h);

            // Status fan-out: forward info_tx changes as StatusUpdate events.
            let mut info_rx = sess.info_rx.clone();
            let tx2 = event_tx.clone();
            tokio::spawn(async move {
                // emit initial state
                let info = info_rx.borrow().clone();
                let _ = tx2
                    .send(Event::StatusUpdate {
                        id: session_id,
                        status: info.status,
                        label: Some(info.label),
                        attention: info.attention,
                        rows: info.rows,
                        cols: info.cols,
                    })
                    .await;
                let mut was_alive = true;
                while info_rx.changed().await.is_ok() {
                    let info = info_rx.borrow().clone();
                    // The child died on its own. Only an explicit kill used to
                    // produce this event, so a crashed session went on being
                    // drawn as running by every client.
                    if was_alive && !info.alive {
                        was_alive = false;
                        let status = info
                            .exit_status
                            .clone()
                            .unwrap_or_else(|| "exited".to_string());
                        if tx2
                            .send(Event::SessionExited {
                                id: session_id,
                                status,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    if tx2
                        .send(Event::StatusUpdate {
                            id: session_id,
                            status: info.status,
                            label: Some(info.label),
                            attention: info.attention,
                            rows: info.rows,
                            cols: info.cols,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }
        Request::Unsubscribe { session_id } => {
            if let Some(h) = subscriptions.remove(&session_id) {
                h.abort();
            }
        }
        Request::Shutdown => {
            // Kill every session and ask the daemon to exit. Sends Goodbye
            // first so the requesting client knows it landed.
            let _ = event_tx
                .send(Event::Goodbye {
                    reason: "shutdown requested".into(),
                })
                .await;
            let sessions = registry.sessions.lock().await;
            for s in sessions.values() {
                s.kill();
            }
            drop(sessions);
            // Self-signal SIGTERM so the main shutdown path runs (cleans up
            // socket + ready file).
            kill_self();
        }
        Request::KillAll => {
            let sessions = registry.sessions.lock().await;
            for s in sessions.values() {
                s.kill();
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Async framing helpers
// ---------------------------------------------------------------------------

async fn read_frame_async<T: for<'de> serde::Deserialize<'de>>(
    stream: &mut tokio::net::unix::OwnedReadHalf,
) -> Result<T, FrameError> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.map_err(framing_io)?;
    let len = u32::from_le_bytes(len_buf);
    if len > cmux_proto::MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(len));
    }
    let mut payload = vec![0u8; len as usize];
    stream.read_exact(&mut payload).await.map_err(framing_io)?;
    let msg = serde_json::from_slice(&payload)?;
    Ok(msg)
}

async fn write_frame_async<T: serde::Serialize>(
    stream: &mut tokio::net::unix::OwnedWriteHalf,
    msg: &T,
) -> Result<(), FrameError> {
    let payload = serde_json::to_vec(msg)?;
    if payload.len() as u64 > cmux_proto::MAX_FRAME_BYTES as u64 {
        return Err(FrameError::TooLarge(payload.len() as u32));
    }
    let len = (payload.len() as u32).to_le_bytes();
    stream.write_all(&len).await.map_err(framing_io)?;
    stream.write_all(&payload).await.map_err(framing_io)?;
    stream.flush().await.map_err(framing_io)?;
    Ok(())
}

fn framing_io(e: std::io::Error) -> FrameError {
    if e.kind() == std::io::ErrorKind::UnexpectedEof {
        FrameError::Eof
    } else {
        FrameError::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Insert out of id order; `list` must still come back ascending. With a
    /// HashMap the insertion order is not the iteration order, so this fails
    /// if the sort is removed.
    #[tokio::test]
    async fn list_is_ordered_by_session_id() {
        let registry = Registry::new();
        for id in [4u64, 1, 5, 3, 2] {
            let sess = Session::spawn(
                id,
                format!("s{id}"),
                PathBuf::from("/tmp"),
                vec!["/bin/sleep".into(), "30".into()],
                cmux_proto::ProbeKind::None,
                24,
                80,
            )
            .expect("spawn /bin/sleep");
            registry.insert(sess).await;
        }
        let ids: Vec<u64> = registry.list().await.into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5]);

        for s in registry.sessions.lock().await.values() {
            s.kill();
        }
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn bare_cmuxd_runs_the_daemon_with_no_http() {
        assert_eq!(parse_cli(Vec::<String>::new()), Cli::Run(Config::default()));
    }

    #[test]
    fn http_is_opt_in_and_takes_an_optional_address() {
        // Bare --http means the default address.
        assert_eq!(
            parse_cli(args(&["--http"])),
            Cli::Run(Config {
                http: Some(DEFAULT_HTTP_ADDR.to_string()),
            })
        );
        // Both spellings of an explicit address.
        let want = Cli::Run(Config {
            http: Some("0.0.0.0:9000".to_string()),
        });
        assert_eq!(parse_cli(args(&["--http", "0.0.0.0:9000"])), want);
        assert_eq!(parse_cli(args(&["--http=0.0.0.0:9000"])), want);
    }

    /// `--http` must not swallow a following flag as its address.
    #[test]
    fn http_does_not_consume_the_next_flag() {
        assert!(matches!(
            parse_cli(args(&["--http", "--version"])),
            Cli::Print(_)
        ));
        assert!(matches!(
            parse_cli(args(&["--http", "--nope"])),
            Cli::Reject(_)
        ));
    }

    /// The bug this guards: argv was ignored, so `cmuxd --help` started a
    /// daemon and blocked instead of printing anything.
    #[test]
    fn help_and_version_print_instead_of_daemonizing() {
        let help = parse_cli(vec!["--help".to_string()]);
        assert!(
            matches!(help, Cli::Print(ref t) if t.contains("Usage: cmuxd")),
            "{help:?}"
        );
        assert_eq!(parse_cli(vec!["-h".to_string()]), help);

        match parse_cli(vec!["--version".to_string()]) {
            Cli::Print(t) => assert_eq!(t, format!("cmuxd {SERVER_VERSION}")),
            other => panic!("expected a version line, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_argument_is_rejected_not_ignored() {
        match parse_cli(vec!["--nope".to_string()]) {
            Cli::Reject(msg) => assert!(msg.contains("--nope"), "{msg}"),
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[test]
    fn fallback_socket_dir_nests_home_under_the_uid_dir() {
        assert_eq!(
            fallback_socket_dir(1000, "/home/mcs"),
            PathBuf::from("/tmp/cmux-1000/home/mcs")
        );
        assert_eq!(
            fallback_socket_dir(0, "/root"),
            PathBuf::from("/tmp/cmux-0/root")
        );
    }

    fn sleeper(id: u64) -> Arc<Session> {
        Session::spawn(
            id,
            format!("s{id}"),
            PathBuf::from("/tmp"),
            vec!["/bin/sleep".into(), "30".into()],
            cmux_proto::ProbeKind::None,
            24,
            80,
        )
        .expect("spawn /bin/sleep")
    }

    #[test]
    fn session_ids_and_client_ids_come_from_separate_counters() {
        let registry = Registry::new();
        assert_eq!(registry.alloc_id(), 1);
        assert_eq!(registry.alloc_id(), 2);
        assert_eq!(
            registry.alloc_client_id(),
            1,
            "a client id should not be drawn from the session pool"
        );
        assert_eq!(registry.alloc_client_id(), 2);
        assert_eq!(registry.alloc_id(), 3, "allocating a client moved the ids");
    }

    #[tokio::test]
    async fn removing_a_session_takes_it_out_once() {
        let registry = Registry::new();
        let sess = sleeper(1);
        registry.insert(sess.clone()).await;
        assert!(registry.get(1).await.is_some());
        assert!(registry.remove(1).await.is_some());
        assert!(registry.get(1).await.is_none(), "it is still registered");
        assert!(
            registry.remove(1).await.is_none(),
            "removing twice should not hand out a second copy"
        );
        sess.kill();
    }

    /// The pty runs at the smallest size among attached clients, so a client
    /// that leaves has to be forgotten in every session at once. While it was
    /// not, a phone that closed its tab kept every session pinned small.
    #[tokio::test]
    async fn a_departing_client_stops_holding_every_session_small() {
        let registry = Registry::new();
        let (a, b) = (sleeper(1), sleeper(2));
        registry.insert(a.clone()).await;
        registry.insert(b.clone()).await;

        let phone = registry.alloc_client_id();
        a.set_client_size(phone, 10, 40).expect("size a");
        b.set_client_size(phone, 10, 40).expect("size b");
        for s in [&a, &b] {
            let info = s.info();
            assert_eq!((info.rows, info.cols), (10, 40), "session {} ", info.id);
        }

        registry.drop_client_everywhere(phone).await;

        for s in [&a, &b] {
            let info = s.info();
            assert_eq!(
                (info.rows, info.cols),
                (24, 80),
                "session {} stayed small after the client left",
                info.id
            );
            assert_eq!(s.attached_clients(), 0);
        }
        a.kill();
        b.kill();
    }

    #[tokio::test]
    async fn a_spawned_session_is_labelled_after_its_directory() {
        let registry = Arc::new(Registry::new());
        let info = spawn_session(
            &registry,
            PathBuf::from("/tmp/some-project"),
            vec!["/bin/sleep".into(), "30".into()],
            cmux_proto::ProbeKind::None,
            None,
            24,
            80,
        )
        .await
        .expect("spawn");
        assert_eq!(info.label, "some-project");

        let named = spawn_session(
            &registry,
            PathBuf::from("/tmp/some-project"),
            vec!["/bin/sleep".into(), "30".into()],
            cmux_proto::ProbeKind::None,
            Some("chosen".into()),
            24,
            80,
        )
        .await
        .expect("spawn");
        assert_eq!(named.label, "chosen", "an explicit label should win");

        for s in registry.sessions.lock().await.values() {
            s.kill();
        }
    }

    /// openpty rejects a zero dimension, so a client asking for one must be
    /// clamped rather than failing the spawn.
    #[tokio::test]
    async fn a_zero_size_request_is_clamped_instead_of_refused() {
        let registry = Arc::new(Registry::new());
        let info = spawn_session(
            &registry,
            PathBuf::from("/tmp"),
            vec!["/bin/sleep".into(), "30".into()],
            cmux_proto::ProbeKind::None,
            None,
            0,
            0,
        )
        .await
        .expect("a zero size should not fail the spawn");
        assert!(info.rows >= 1 && info.cols >= 1, "got {info:?}");

        for s in registry.sessions.lock().await.values() {
            s.kill();
        }
    }

    #[tokio::test]
    async fn spawning_an_unknown_program_reports_the_program_name() {
        let registry = Arc::new(Registry::new());
        let err = spawn_session(
            &registry,
            PathBuf::from("/tmp"),
            vec!["definitely-not-a-real-binary".into()],
            cmux_proto::ProbeKind::None,
            None,
            24,
            80,
        )
        .await
        .expect_err("an unknown program should not spawn");
        let text = format!("{err:#}");
        assert!(
            text.contains("definitely-not-a-real-binary"),
            "the error should name the program: {text}"
        );
        assert!(
            registry.sessions.lock().await.is_empty(),
            "a failed spawn should register nothing"
        );
    }

    async fn frame_pair() -> (
        tokio::net::unix::OwnedWriteHalf,
        tokio::net::unix::OwnedReadHalf,
    ) {
        let (a, b) = tokio::net::UnixStream::pair().expect("socketpair");
        let (_, w) = a.into_split();
        let (r, _) = b.into_split();
        (w, r)
    }

    #[tokio::test]
    async fn a_frame_survives_the_round_trip() {
        let (mut w, mut r) = frame_pair().await;
        let sent = cmux_proto::Request::Resize {
            session_id: 7,
            rows: 30,
            cols: 100,
        };
        write_frame_async(&mut w, &sent).await.expect("write");
        let got: cmux_proto::Request = read_frame_async(&mut r).await.expect("read");
        assert!(
            matches!(
                got,
                cmux_proto::Request::Resize {
                    session_id: 7,
                    rows: 30,
                    cols: 100
                }
            ),
            "got {got:?}"
        );
    }

    #[tokio::test]
    async fn frames_come_back_in_the_order_they_were_written() {
        let (mut w, mut r) = frame_pair().await;
        for id in 1..=3u64 {
            write_frame_async(
                &mut w,
                &cmux_proto::Request::Detach {
                    session_id: id,
                    keep_session: true,
                },
            )
            .await
            .expect("write");
        }
        for want in 1..=3u64 {
            match read_frame_async::<cmux_proto::Request>(&mut r)
                .await
                .expect("read")
            {
                cmux_proto::Request::Detach { session_id, .. } => assert_eq!(session_id, want),
                other => panic!("expected Detach, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_closed_stream_reads_as_eof_not_an_io_error() {
        let (w, mut r) = frame_pair().await;
        drop(w);
        match read_frame_async::<cmux_proto::Request>(&mut r).await {
            Err(FrameError::Eof) => {}
            other => panic!("expected Eof, got {other:?}"),
        }
    }

    /// A length header larger than the cap has to be refused before the
    /// payload is allocated, or a bad peer can ask for gigabytes.
    #[tokio::test]
    async fn an_oversized_length_header_is_refused() {
        let (a, b) = tokio::net::UnixStream::pair().expect("socketpair");
        let (_, mut raw) = a.into_split();
        let (mut r, _keep) = b.into_split();
        let too_big = cmux_proto::MAX_FRAME_BYTES + 1;
        raw.write_all(&too_big.to_le_bytes()).await.expect("write");
        raw.flush().await.expect("flush");
        // Bounded, because without the check the read blocks on a payload that
        // never comes rather than returning anything.
        let got = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            read_frame_async::<cmux_proto::Request>(&mut r),
        )
        .await
        .expect("the header should be refused, not waited on");
        match got {
            Err(FrameError::TooLarge(n)) => assert_eq!(n, too_big),
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_truncated_payload_reads_as_eof() {
        let (a, b) = tokio::net::UnixStream::pair().expect("socketpair");
        let (_, mut raw) = a.into_split();
        let (mut r, _keep) = b.into_split();
        raw.write_all(&64u32.to_le_bytes()).await.expect("len");
        raw.write_all(b"{\"tru").await.expect("partial");
        raw.flush().await.expect("flush");
        drop(raw);
        match read_frame_async::<cmux_proto::Request>(&mut r).await {
            Err(FrameError::Eof) => {}
            other => panic!("expected Eof, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_well_framed_but_unparseable_payload_is_a_decode_error() {
        let (a, b) = tokio::net::UnixStream::pair().expect("socketpair");
        let (_, mut raw) = a.into_split();
        let (mut r, _keep) = b.into_split();
        let junk = b"not json at all";
        raw.write_all(&(junk.len() as u32).to_le_bytes())
            .await
            .expect("len");
        raw.write_all(junk).await.expect("payload");
        raw.flush().await.expect("flush");
        match read_frame_async::<cmux_proto::Request>(&mut r).await {
            Err(FrameError::Eof) | Err(FrameError::Io(_)) | Err(FrameError::TooLarge(_)) => {
                panic!("a bad payload should be a decode error, not a transport one")
            }
            Err(_) => {}
            Ok(msg) => panic!("junk decoded to {msg:?}"),
        }
    }
}
