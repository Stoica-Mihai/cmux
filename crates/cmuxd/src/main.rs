//! `cmuxd` — long-lived daemon owning every `claude` PTY.
//!
//! Phase 2: socket lifecycle + Hello/Welcome handshake. No session ownership
//! yet — that lands in phase 3. See `DAEMON_PLAN.md`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use anyhow::{Context, Result};
use cmux_proto::{Event, FrameError, PROTOCOL_VERSION, Request};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal;
use tokio::sync::broadcast;

const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

fn socket_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| {
                let uid = nix_compat_uid();
                PathBuf::from(format!("/tmp/cmux-{}", uid))
                    .join(h.to_string_lossy().to_string())
            })
        })
        .context("no XDG_RUNTIME_DIR or HOME")?;
    Ok(base.join("cmux"))
}

fn nix_compat_uid() -> u32 {
    // Avoid the libc dep just for getuid: read /proc/self/status, or fall back.
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

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let dir = socket_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).ok();

    let sock = socket_path()?;
    // Stale-socket cleanup. A previous daemon may have crashed without unlinking.
    if sock.exists() {
        // If something is already listening we'd refuse to bind below; bail early
        // in that case to avoid clobbering a live daemon.
        if UnixStream::connect(&sock).await.is_ok() {
            anyhow::bail!(
                "another cmuxd already listening at {} — refusing to start",
                sock.display()
            );
        }
        let _ = fs::remove_file(&sock);
    }

    let listener = UnixListener::bind(&sock)
        .with_context(|| format!("bind {}", sock.display()))?;
    fs::set_permissions(&sock, fs::Permissions::from_mode(0o600)).ok();

    // ready-stamp: tells the spawning TUI we're listening
    let ready = ready_path()?;
    let _ = fs::write(&ready, std::process::id().to_string());

    eprintln!("cmuxd v{} listening at {}", SERVER_VERSION, sock.display());

    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    let accept_shutdown = shutdown_tx.clone();
    let accept_task = tokio::spawn(async move {
        let mut shutdown_rx = accept_shutdown.subscribe();
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _addr)) => {
                            tokio::spawn(handle_connection(stream));
                        }
                        Err(e) => {
                            eprintln!("cmuxd: accept error: {e}");
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    }
                }
            }
        }
    });

    // Graceful shutdown on SIGINT / SIGTERM
    tokio::select! {
        _ = signal::ctrl_c() => {
            eprintln!("cmuxd: SIGINT received, shutting down");
        }
        _ = wait_sigterm() => {
            eprintln!("cmuxd: SIGTERM received, shutting down");
        }
    }
    let _ = shutdown_tx.send(());
    let _ = accept_task.await;

    let _ = fs::remove_file(&sock);
    let _ = fs::remove_file(&ready);
    Ok(())
}

async fn wait_sigterm() {
    use signal::unix::{SignalKind, signal};
    if let Ok(mut sig) = signal(SignalKind::terminate()) {
        sig.recv().await;
    } else {
        std::future::pending::<()>().await;
    }
}

async fn handle_connection(mut stream: UnixStream) {
    if let Err(e) = handshake(&mut stream).await {
        eprintln!("cmuxd: handshake failed: {e}");
        let _ = stream.shutdown().await;
        return;
    }
    // Phase 2 ends here — connection is closed after handshake. Phase 3 will
    // keep the connection open for session ownership operations.
    let _ = stream.shutdown().await;
}

async fn handshake(stream: &mut UnixStream) -> Result<()> {
    let req: Request = read_frame_async(stream)
        .await
        .context("reading Hello")?;
    let Request::Hello { client_version, want_protocol } = req else {
        anyhow::bail!("expected Hello as first message, got {req:?}");
    };
    if want_protocol != PROTOCOL_VERSION {
        let goodbye = Event::Goodbye {
            reason: format!(
                "protocol skew: client wants {}, server speaks {}",
                want_protocol, PROTOCOL_VERSION
            ),
        };
        write_frame_async(stream, &goodbye).await.ok();
        anyhow::bail!("protocol skew");
    }
    let welcome = Event::Welcome {
        server_version: SERVER_VERSION.to_string(),
        protocol: PROTOCOL_VERSION,
        session_count: 0,
    };
    write_frame_async(stream, &welcome).await?;
    eprintln!("cmuxd: client {client_version} handshake ok");
    Ok(())
}

// Async wrappers around the sync framing helpers.

async fn read_frame_async<T: for<'de> serde::Deserialize<'de>>(
    stream: &mut UnixStream,
) -> Result<T, FrameError> {
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .await
        .map_err(framing_io)?;
    let len = u32::from_le_bytes(len_buf);
    if len > cmux_proto::MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(len));
    }
    let mut payload = vec![0u8; len as usize];
    stream
        .read_exact(&mut payload)
        .await
        .map_err(framing_io)?;
    let msg = serde_json::from_slice(&payload)?;
    Ok(msg)
}

async fn write_frame_async<T: serde::Serialize>(
    stream: &mut UnixStream,
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
