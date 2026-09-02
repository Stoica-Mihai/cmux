//! Thin client for talking to `cmuxd` over the UNIX socket.
//!
//! Phase 3 MVP: connect, Hello/Welcome handshake, basic helpers for sending
//! Requests and receiving Events. The actual TUI render loop wiring lands in
//! phase 4. For now this exists so that `cmux --connect` can verify the
//! daemon is reachable.

use std::ffi::OsStr;
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
        let stream =
            UnixStream::connect(path).with_context(|| format!("connect {}", path.display()))?;
        let mut this = Self { stream };
        this.send(&Request::Hello {
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            want_protocol: PROTOCOL_VERSION,
        })?;
        match this.recv()? {
            Event::Welcome { protocol, .. } => {
                if protocol != PROTOCOL_VERSION {
                    anyhow::bail!(
                        "protocol skew: server={} client={}",
                        protocol,
                        PROTOCOL_VERSION
                    );
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

/// Compose the socket path from the two directory variables. Split out from
/// [`socket_path`] so the composition is testable without mutating the
/// process environment, which would race parallel tests.
fn compose_socket_path(runtime_dir: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let base = runtime_dir
        .map(PathBuf::from)
        .or_else(|| home.map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("cmux").join("cmuxd.sock"))
}

pub fn socket_path() -> Option<PathBuf> {
    compose_socket_path(
        std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use cmux_proto::{MAX_FRAME_BYTES, ProbeKind, SessionStatus};
    use std::ffi::OsString;
    use std::io::Cursor;
    use std::os::unix::net::UnixListener;

    fn framed(bytes: &[u8]) -> Cursor<Vec<u8>> {
        Cursor::new(bytes.to_vec())
    }

    /// A length header followed by fewer payload bytes than it claims.
    fn header_then(len: u32, payload: &[u8]) -> Cursor<Vec<u8>> {
        let mut buf = len.to_le_bytes().to_vec();
        buf.extend_from_slice(payload);
        framed(&buf)
    }

    #[test]
    fn a_request_round_trips_through_a_frame() {
        let sent = Request::Input {
            session_id: 9,
            bytes: vec![0x1b, b'[', b'A'],
        };
        let mut wire: Vec<u8> = Vec::new();
        write_frame(&mut wire, &sent).expect("write");

        let back: Request = read_frame(&mut framed(&wire)).expect("read");
        assert_eq!(
            format!("{back:?}"),
            format!("{sent:?}"),
            "a Request did not survive the frame round trip"
        );
    }

    #[test]
    fn an_event_round_trips_through_a_frame() {
        let sent = Event::StatusUpdate {
            id: 3,
            status: SessionStatus::Busy,
            label: Some("dev".into()),
            attention: true,
            rows: 40,
            cols: 120,
        };
        let mut wire: Vec<u8> = Vec::new();
        write_frame(&mut wire, &sent).expect("write");

        let back: Event = read_frame(&mut framed(&wire)).expect("read");
        assert_eq!(
            format!("{back:?}"),
            format!("{sent:?}"),
            "an Event did not survive the frame round trip"
        );
    }

    #[test]
    fn frames_read_back_in_write_order() {
        let sent = [
            Request::ListSessions,
            Request::Attach {
                session_id: 1,
                want_history: true,
            },
            Request::Shutdown,
        ];
        let mut wire: Vec<u8> = Vec::new();
        for req in &sent {
            write_frame(&mut wire, req).expect("write");
        }

        let mut cur = framed(&wire);
        for expected in &sent {
            let back: Request = read_frame(&mut cur).expect("read");
            assert_eq!(
                format!("{back:?}"),
                format!("{expected:?}"),
                "frames came back out of write order"
            );
        }
        assert!(
            matches!(read_frame::<_, Request>(&mut cur), Err(FrameError::Eof)),
            "the stream should be exhausted after the last frame"
        );
    }

    #[test]
    fn an_empty_stream_reads_as_eof() {
        let err = read_frame::<_, Request>(&mut framed(&[])).expect_err("empty stream");
        assert!(
            matches!(err, FrameError::Eof),
            "an empty stream gave {err:?}, want FrameError::Eof"
        );
    }

    #[test]
    fn a_header_with_no_payload_after_it_is_eof() {
        let err = read_frame::<_, Request>(&mut header_then(3, &[])).expect_err("bare header");
        let io_eof =
            matches!(&err, FrameError::Io(e) if e.kind() == std::io::ErrorKind::UnexpectedEof);
        assert!(
            io_eof,
            "a header with no payload gave {err:?}, want an UnexpectedEof I/O error"
        );
    }

    #[test]
    fn a_truncated_payload_does_not_block_or_panic() {
        let err = read_frame::<_, Request>(&mut header_then(64, b"{\"kind\""))
            .expect_err("truncated payload");
        let io_eof =
            matches!(&err, FrameError::Io(e) if e.kind() == std::io::ErrorKind::UnexpectedEof);
        assert!(
            io_eof,
            "a truncated payload gave {err:?}, want an UnexpectedEof I/O error"
        );
    }

    #[test]
    fn a_zero_length_frame_is_a_decode_error() {
        let err = read_frame::<_, Request>(&mut header_then(0, b"")).expect_err("zero length");
        assert!(
            matches!(err, FrameError::Json(_)),
            "a zero-length frame gave {err:?}, want FrameError::Json"
        );
    }

    #[test]
    fn a_garbage_payload_is_a_decode_error() {
        let err = read_frame::<_, Request>(&mut header_then(11, b"not json at"))
            .expect_err("garbage payload");
        assert!(
            matches!(err, FrameError::Json(_)),
            "a garbage payload gave {err:?}, want FrameError::Json"
        );
    }

    #[test]
    fn a_length_header_over_the_cap_is_rejected_before_reading_a_payload() {
        let err = read_frame::<_, Request>(&mut header_then(MAX_FRAME_BYTES + 1, b""))
            .expect_err("oversize header");
        match err {
            FrameError::TooLarge(n) => {
                assert_eq!(n, MAX_FRAME_BYTES + 1, "TooLarge reported the wrong length")
            }
            other => panic!("an oversize header gave {other:?}, want FrameError::TooLarge"),
        }
    }

    #[test]
    fn an_oversize_payload_is_rejected_on_write() {
        let req = Request::Rename {
            session_id: 1,
            label: "a".repeat(MAX_FRAME_BYTES as usize + 16),
        };
        let mut wire: Vec<u8> = Vec::new();
        let err = write_frame(&mut wire, &req).expect_err("oversize write");
        assert!(
            matches!(err, FrameError::TooLarge(_)),
            "an oversize payload gave {err:?}, want FrameError::TooLarge"
        );
        assert!(
            wire.is_empty(),
            "an oversize payload put {} bytes on the wire before failing",
            wire.len()
        );
    }

    #[test]
    fn split_halves_carry_traffic_in_both_directions() {
        let (near, mut far) = UnixStream::pair().expect("socketpair");
        let (mut reader, mut writer) = Client { stream: near }.split().expect("split");

        writer
            .send(&Request::Detach {
                session_id: 4,
                keep_session: false,
            })
            .expect("send");
        let seen: Request = read_frame(&mut far).expect("peer read");
        assert!(
            matches!(
                seen,
                Request::Detach {
                    session_id: 4,
                    keep_session: false
                }
            ),
            "the writer half sent {seen:?}"
        );

        write_frame(
            &mut far,
            &Event::Goodbye {
                reason: "bye".into(),
            },
        )
        .expect("peer write");
        let got = reader.recv().expect("recv");
        assert!(
            matches!(got, Event::Goodbye { .. }),
            "the reader half got {got:?}, want Goodbye"
        );
    }

    fn fixture_socket(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("cmux-client-{}-{name}.sock", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    /// `Client` has no `Debug`, so `Result::expect_err` is unavailable here.
    fn connect_error(path: &Path, what: &str) -> anyhow::Error {
        match Client::connect(path) {
            Ok(_) => panic!("connect accepted {what}"),
            Err(e) => e,
        }
    }

    /// Accept one connection, read the Hello, answer with `reply`, hand the
    /// Hello back to the test.
    fn serve_one_handshake(
        listener: UnixListener,
        reply: Event,
    ) -> std::thread::JoinHandle<Request> {
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().expect("accept");
            let hello: Request = read_frame(&mut sock).expect("read hello");
            write_frame(&mut sock, &reply).expect("write reply");
            hello
        })
    }

    #[test]
    fn connect_sends_hello_and_accepts_a_matching_welcome() {
        let path = fixture_socket("welcome");
        let listener = UnixListener::bind(&path).expect("bind");
        let server = serve_one_handshake(
            listener,
            Event::Welcome {
                server_version: "0.1.0".into(),
                protocol: PROTOCOL_VERSION,
                session_count: 0,
            },
        );

        let client = Client::connect(&path);
        let hello = server.join().expect("server thread");
        let _ = std::fs::remove_file(&path);

        assert!(client.is_ok(), "connect failed: {:?}", client.err());
        match hello {
            Request::Hello { want_protocol, .. } => assert_eq!(
                want_protocol, PROTOCOL_VERSION,
                "the handshake asked for the wrong protocol version"
            ),
            other => panic!("the daemon saw {other:?}, want Request::Hello"),
        }
    }

    #[test]
    fn connect_rejects_a_protocol_mismatch() {
        let path = fixture_socket("skew");
        let listener = UnixListener::bind(&path).expect("bind");
        let server = serve_one_handshake(
            listener,
            Event::Welcome {
                server_version: "0.1.0".into(),
                protocol: PROTOCOL_VERSION + 1,
                session_count: 0,
            },
        );

        let err = connect_error(&path, "a version-skewed Welcome");
        let _ = server.join();
        let _ = std::fs::remove_file(&path);

        assert!(
            err.to_string().contains("protocol skew"),
            "a version-skewed Welcome was accepted, or failed with {err}"
        );
    }

    #[test]
    fn connect_reports_a_goodbye_as_a_refusal() {
        let path = fixture_socket("goodbye");
        let listener = UnixListener::bind(&path).expect("bind");
        let server = serve_one_handshake(
            listener,
            Event::Goodbye {
                reason: "too many clients".into(),
            },
        );

        let err = connect_error(&path, "a Goodbye handshake");
        let _ = server.join();
        let _ = std::fs::remove_file(&path);

        assert!(
            err.to_string().contains("too many clients"),
            "a Goodbye handshake failed with {err}, which drops the daemon's reason"
        );
    }

    #[test]
    fn connect_reports_an_unexpected_first_event() {
        let path = fixture_socket("wrongevent");
        let listener = UnixListener::bind(&path).expect("bind");
        let server = serve_one_handshake(listener, Event::SessionList { sessions: vec![] });

        let err = connect_error(&path, "a non-Welcome first event");
        let _ = server.join();
        let _ = std::fs::remove_file(&path);

        assert!(
            err.to_string().contains("expected Welcome"),
            "a non-Welcome first event failed with {err}"
        );
    }

    #[test]
    fn connect_fails_when_nothing_is_listening() {
        let path = fixture_socket("absent");
        let err = connect_error(&path, "a path with no listener");
        assert!(
            err.to_string().contains("connect"),
            "connecting to a dead path failed with {err}"
        );
    }

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    #[test]
    fn socket_path_prefers_the_runtime_dir_over_home() {
        let got = compose_socket_path(Some(&os("/run/user/1000")), Some(&os("/home/u")));
        assert_eq!(
            got,
            Some(PathBuf::from("/run/user/1000/cmux/cmuxd.sock")),
            "XDG_RUNTIME_DIR should win over HOME"
        );
    }

    #[test]
    fn socket_path_falls_back_to_the_home_cache() {
        let got = compose_socket_path(None, Some(&os("/home/u")));
        assert_eq!(
            got,
            Some(PathBuf::from("/home/u/.cache/cmux/cmuxd.sock")),
            "without XDG_RUNTIME_DIR the path should sit under ~/.cache"
        );
    }

    #[test]
    fn socket_path_is_none_without_either_variable() {
        assert_eq!(
            compose_socket_path(None, None),
            None,
            "with neither variable set there is no socket path"
        );
    }

    /// `PathBuf::join` drops the base when the joined component is absolute,
    /// which once turned an absolute `$HOME` into the wrong directory.
    #[test]
    fn socket_path_never_discards_its_base_directory() {
        for base in ["/run/user/1000", "/home/u", "/", "relative/base"] {
            let runtime = compose_socket_path(Some(&os(base)), None).expect("runtime dir path");
            assert!(
                runtime.starts_with(base),
                "base {base:?} was discarded: got {runtime:?}"
            );
            assert!(
                runtime.ends_with("cmux/cmuxd.sock"),
                "base {base:?} produced {runtime:?}, which does not end in cmux/cmuxd.sock"
            );

            let home = compose_socket_path(None, Some(&os(base))).expect("home path");
            assert!(
                home.starts_with(base),
                "base {base:?} was discarded on the HOME branch: got {home:?}"
            );
            assert!(
                home.ends_with(".cache/cmux/cmuxd.sock"),
                "base {base:?} produced {home:?}, which does not end in .cache/cmux/cmuxd.sock"
            );
        }
    }

    #[test]
    fn socket_path_uses_whichever_variable_the_environment_actually_has() {
        let expected = compose_socket_path(
            std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
            std::env::var_os("HOME").as_deref(),
        );
        assert_eq!(
            socket_path(),
            expected,
            "socket_path disagrees with the composition it delegates to"
        );
    }

    #[test]
    fn probe_kind_survives_a_spawn_frame() {
        let sent = Request::SpawnSession {
            cwd: PathBuf::from("/tmp"),
            cmd: vec!["claude".into()],
            probe: ProbeKind::Claude {
                dangerous: true,
                resume_id: Some("abc".into()),
            },
            label: None,
            rows: 24,
            cols: 80,
        };
        let mut wire: Vec<u8> = Vec::new();
        write_frame(&mut wire, &sent).expect("write");
        let back: Request = read_frame(&mut framed(&wire)).expect("read");
        assert_eq!(
            format!("{back:?}"),
            format!("{sent:?}"),
            "a SpawnSession probe did not survive the frame round trip"
        );
    }
}
