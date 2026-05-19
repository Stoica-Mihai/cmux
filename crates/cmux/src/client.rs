//! Thin client for talking to `cmuxd` over the UNIX socket.
//!
//! Phase 3 MVP: connect, Hello/Welcome handshake, basic helpers for sending
//! Requests and receiving Events. The actual TUI render loop wiring lands in
//! phase 4. For now this exists so that `cmux --connect` can verify the
//! daemon is reachable.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cmux_proto::{Event, FrameError, PROTOCOL_VERSION, Request};

pub struct Client {
    stream: UnixStream,
}

impl Client {
    pub fn connect(path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(path)
            .with_context(|| format!("connect {}", path.display()))?;
        let mut this = Self { stream };
        this.send(&Request::Hello {
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            want_protocol: PROTOCOL_VERSION,
        })?;
        match this.recv()? {
            Event::Welcome { protocol, .. } => {
                if protocol != PROTOCOL_VERSION {
                    anyhow::bail!("protocol skew: server={} client={}", protocol, PROTOCOL_VERSION);
                }
                Ok(this)
            }
            Event::Goodbye { reason } => anyhow::bail!("daemon refused: {reason}"),
            other => anyhow::bail!("expected Welcome, got {other:?}"),
        }
    }

    pub fn send(&mut self, req: &Request) -> Result<(), FrameError> {
        write_frame(&mut self.stream, req)
    }

    pub fn recv(&mut self) -> Result<Event, FrameError> {
        read_frame(&mut self.stream)
    }

    /// Split into reader (blocking recv) and writer (blocking send) halves.
    /// Both halves clone the underlying socket fd; either side can be parked
    /// on its respective syscall without blocking the other.
    pub fn split(self) -> std::io::Result<(ClientReader, ClientWriter)> {
        let r = self.stream.try_clone()?;
        Ok((
            ClientReader { stream: r },
            ClientWriter {
                stream: self.stream,
            },
        ))
    }
}

pub struct ClientReader {
    stream: UnixStream,
}

impl ClientReader {
    pub fn recv(&mut self) -> Result<Event, FrameError> {
        read_frame(&mut self.stream)
    }
}

pub struct ClientWriter {
    stream: UnixStream,
}

impl ClientWriter {
    pub fn send(&mut self, req: &Request) -> Result<(), FrameError> {
        write_frame(&mut self.stream, req)
    }
}

pub fn socket_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("cmux").join("cmuxd.sock"))
}

// Local re-implementation of the framing helpers (cmux-proto's are generic
// enough but we want to stick to std::io here, not import all of cmux-proto's
// optional async machinery).

fn write_frame<W: Write, T: serde::Serialize>(w: &mut W, msg: &T) -> Result<(), FrameError> {
    let payload = serde_json::to_vec(msg)?;
    if payload.len() as u64 > cmux_proto::MAX_FRAME_BYTES as u64 {
        return Err(FrameError::TooLarge(payload.len() as u32));
    }
    let len = (payload.len() as u32).to_le_bytes();
    w.write_all(&len)?;
    w.write_all(&payload)?;
    w.flush()?;
    Ok(())
}

fn read_frame<R: Read, T: for<'de> serde::Deserialize<'de>>(r: &mut R) -> Result<T, FrameError> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(FrameError::Eof),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len_buf);
    if len > cmux_proto::MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(len));
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)?;
    Ok(serde_json::from_slice(&payload)?)
}
