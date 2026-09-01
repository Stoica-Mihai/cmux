//! `cmuxd` — long-lived daemon owning one PTY per session.
//!
//! The daemon is command-agnostic: it execs whatever argv a client sends, and
//! delegates anything it can say about the child to a [`probe`].

mod probe;
mod session;

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

/// Daemon-global state shared across all connections.
struct Registry {
    sessions: Mutex<HashMap<u64, Arc<Session>>>,
    next_id: AtomicU64,
}

impl Registry {
    fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    async fn list(&self) -> Vec<cmux_proto::SessionInfo> {
        let sessions = self.sessions.lock().await;
        sessions.values().map(|s| s.info()).collect()
    }

    async fn insert(&self, sess: Arc<Session>) {
        self.sessions.lock().await.insert(sess.id, sess);
    }

    async fn get(&self, id: u64) -> Option<Arc<Session>> {
        self.sessions.lock().await.get(&id).cloned()
    }

    async fn remove(&self, id: u64) -> Option<Arc<Session>> {
        self.sessions.lock().await.remove(&id)
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
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
                    if let Err(e) = dispatch(req, &registry, &event_tx, &mut subscriptions).await {
                        let _ = event_tx
                            .send(Event::Error {
                                request_id: None,
                                message: format!("{e}"),
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
    drop(event_tx);
    let _ = writer_task.await;
    result
}

async fn dispatch(
    req: Request,
    registry: &Arc<Registry>,
    event_tx: &mpsc::Sender<Event>,
    subscriptions: &mut HashMap<u64, tokio::task::JoinHandle<()>>,
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
            let info = sess.info();
            let _ = event_tx.send(Event::SessionSpawned { id, info }).await;
        }
        Request::Attach { session_id, .. } => {
            let Some(sess) = registry.get(session_id).await else {
                anyhow::bail!("no such session {session_id}");
            };
            // Phase 3 MVP: send empty snapshot (TUI builds fresh Term) and a
            // FrameDelta carrying the full ring so the client reconstructs
            // current state.
            let (rows, cols) = *sess.size.lock().map_err(|_| anyhow::anyhow!("poison"))?;
            let _ = event_tx
                .send(Event::Snapshot {
                    id: session_id,
                    term_bytes: Vec::new(),
                    size: (rows, cols),
                })
                .await;
            let ring = sess.ring_snapshot();
            if !ring.is_empty() {
                let _ = event_tx
                    .send(Event::FrameDelta {
                        id: session_id,
                        bytes: ring,
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
            if let Some(sess) = registry.get(session_id).await {
                sess.resize(rows, cols)?;
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
                    })
                    .await;
                while info_rx.changed().await.is_ok() {
                    let info = info_rx.borrow().clone();
                    if tx2
                        .send(Event::StatusUpdate {
                            id: session_id,
                            status: info.status,
                            label: Some(info.label),
                            attention: info.attention,
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
}
