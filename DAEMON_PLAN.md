# cmuxd — daemon-backed session persistence

Plan to evolve cmux from a single-process TUI into a thin client over a long-lived daemon (`cmuxd`) that owns every `claude` PTY. Sessions survive TUI exit, crash, and in-place upgrade.

---

## 1. Goals & non-goals

### Goals

- Sessions survive `cmux` exit / Ctrl-Q / window-manager crash. Closing the TUI does **not** kill `claude`.
- Multiple TUI clients can attach to the same daemon (and optionally the same session) over a local UNIX socket.
- Reconnect is instant: open `cmux`, see the same scrollback frame the daemon last rendered, no `claude --resume` cold start.
- Zero-config: TUI auto-spawns the daemon on first invocation; daemon auto-exits when the last session is closed (configurable).
- No new runtime deps beyond what the daemon strictly needs (target: tokio + serde + already-present alacritty_terminal/portable-pty/anyhow).
- Backwards compatible: cmux continues to work standalone if the daemon is disabled (`CMUX_NO_DAEMON=1` or `--no-daemon` flag) — falls back to today's in-process behaviour.

### Non-goals (deferred)

- Remote attach over SSH. Plan keeps the socket layer simple enough that `ssh -L unix:…` works out of the box, but no first-class remote story.
- Encrypted IPC. UNIX socket perms (0600) + `XDG_RUNTIME_DIR` ownership are the only access control.
- Daemon-driven scheduling, queueing, or job control of claude prompts.
- Multi-user / system-wide daemon. Each `$UID` owns one daemon.
- Survival of host reboot. Out of scope; daemon dies with the user session.

---

## 2. Current state (one paragraph)

cmux today is a single binary. Each session = one `claude` child spawned via `portable_pty`, parsed in-process by `alacritty_terminal::Term`, rendered by ratatui's `TermWidget`. On exit, every child dies. Persistence is *list-level only*: cwd / label / `resume_id` / sidebar state get serialized to `~/.config/cmux/state.json`; next launch respawns each entry with `claude --resume <id>`. Conversation reattaches at the claude application layer; the PTY process is fresh, any in-flight tool call is lost.

---

## 3. Target architecture

```
┌──────────────────────────────────┐         ┌──────────────────────────────────┐
│  cmux (TUI client)               │         │  cmux (TUI client #2)            │
│  - ratatui front-end             │         │  - same binary, different attach │
│  - mouse-select / OSC52          │         │                                  │
│  - terminal rendering only       │         │                                  │
└──────────────┬───────────────────┘         └──────────────┬───────────────────┘
               │                                            │
               │   length-prefixed framed JSON              │
               │   over $XDG_RUNTIME_DIR/cmux/cmuxd.sock    │
               │                                            │
               └─────────────┬──────────────────────────────┘
                             │
                             ▼
              ┌────────────────────────────────────────┐
              │  cmuxd (daemon)                        │
              │  - owns every PTY (portable_pty)       │
              │  - owns every alacritty Term + parser  │
              │  - tokio runtime, one task per session │
              │  - sub-task per attached client        │
              │  - periodic Term snapshot to disk      │
              │  - state.json reconciliation           │
              └────────────────────────────────────────┘
                             │
                             ▼
              claude child processes (PTY slaves)
```

Wire boundary lives between TUI and daemon. The daemon **never** renders; it only owns state + streams events. The TUI never owns a PTY; it only renders + dispatches input. Same alacritty `Term` lives on the daemon side, serialized over the wire to the TUI on attach, then patched by delta events.

---

## 4. Crates / packages layout

Restructure repo into a Cargo workspace:

```
cmux/
├── Cargo.toml                # [workspace]
├── crates/
│   ├── cmux-proto/           # shared message types, framing helpers (no I/O)
│   ├── cmuxd/                # daemon binary
│   └── cmux/                 # TUI client binary (current code base, slimmed)
├── README.md
└── DAEMON_PLAN.md
```

- `cmux-proto` is a tiny crate so the wire format has a single source of truth. Both binaries depend on it. Pure `serde` definitions, no async.
- `cmuxd` brings in `tokio` (rt-multi-thread + net + macros + io-util + sync), `portable-pty`, `alacritty_terminal`, `serde_json`, `anyhow`, and `chrono` only if metrics need it.
- `cmux` (TUI) keeps `ratatui`, `crossterm`, `alacritty_terminal` (for *rendering* a snapshot the daemon sends), `chrono` for clock. Drops `portable-pty` from its direct deps — only the daemon owns PTYs.

---

## 5. Lifecycle

### Spawning

1. `cmux` starts. Reads `$XDG_RUNTIME_DIR/cmux/cmuxd.sock`.
2. Tries `connect()`. If success → attach handshake.
3. If `ENOENT` / `ECONNREFUSED`:
   - Acquire `/run/user/<UID>/cmux/cmuxd.lock` via `fcntl(F_SETLK)`.
   - If lock acquired: this client owns the spawn. `fork()` + `setsid()` + `fork()` (double-fork, classic daemonize) → child closes all fds, opens `/dev/null` as 0/1/2, then exec self with `--daemon` arg. Parent waits up to 2 s for socket to appear (`inotify` watch on the parent dir or 50 ms polling).
   - If lock contended: another client is mid-spawn. Poll the socket for up to 2 s.
4. Connect, handshake, attach.

### Spawn failure handling

If the daemon panics mid-init, the lock releases, no socket appears, the client times out. Naive retry would loop forever.

Retry policy:
- Up to 3 spawn attempts, exponential backoff (250 ms, 500 ms, 1 s).
- Each attempt: re-check socket, then re-acquire lock if absent.
- After 3 failures: TUI prints `cmux: daemon failed to start (3/3 attempts). Run with --no-daemon for legacy mode or check ~/.local/state/cmux/cmuxd.log` and exits.
- Daemon binary writes a tiny ready-stamp file (`$XDG_RUNTIME_DIR/cmux/cmuxd.ready`) atomically right after `bind()` + `listen()` succeed. Client polls for that file rather than the socket itself, so half-started states are distinguishable from "starting".

### Daemon shutdown

- Daemon exits when:
  - Last session detached **and** `--idle-shutdown=Ns` configured (default: off — daemon stays).
  - `cmuxctl shutdown` sent (admin command, see §10).
  - SIGTERM / SIGINT received → graceful shutdown: snapshot all Term state, send `Goodbye` to clients, kill children, exit.
- Daemon does **not** exit just because clients disconnected. That's the whole point.

### TUI exit

- `Ctrl+Q` / quit chord → send `Detach { keep_session: true }` for each session. Daemon keeps PTYs alive. TUI exits cleanly.
- New chord `Ctrl+A Q` (capital) → send `KillAll`. Daemon kills every session and exits. Distinct from quit-TUI-only.

---

## 6. Protocol (cmux-proto crate)

### Framing

Length-prefixed: `u32_le payload_len || bytes payload` where payload is JSON. JSON over binary is slower but trivial to debug (`socat - UNIX-CONNECT:…sock` for live inspection). Bench later; switch to `postcard` if measurable cost.

### Message types

```rust
// client → daemon
enum Request {
    Hello { client_version: String, want_protocol: u32 },
    ListSessions,
    SpawnSession { cwd: PathBuf, dangerous: bool, resume_id: Option<String>, label: Option<String> },
    Attach { session_id: u64, want_history: bool },        // returns Term snapshot
    Detach { session_id: u64, keep_session: bool },         // false → kills child
    Input { session_id: u64, bytes: Vec<u8> },              // PTY write
    Resize { session_id: u64, rows: u16, cols: u16 },
    Rename { session_id: u64, label: String },
    Scroll { session_id: u64, scroll: ScrollOp },           // Delta(i32)|PageUp|...
    Subscribe { session_id: u64 },                          // start receiving FrameDelta events
    Unsubscribe { session_id: u64 },
    Shutdown,                                               // daemon-level
    KillAll,
}

// daemon → client
enum Event {
    Welcome { server_version: String, protocol: u32, session_count: usize },
    SessionList(Vec<SessionInfo>),
    SessionSpawned { id: u64, info: SessionInfo },
    SessionExited { id: u64, status: String },
    Snapshot { id: u64, term: SerializedTerm, size: (u16, u16) },   // alacritty Term via serde
    FrameDelta { id: u64, bytes: Vec<u8> },                          // raw PTY bytes since last delta
    StatusUpdate { id: u64, status: ClaudeStatus, label: Option<String>, permission_pending: bool },
    Error { request_id: Option<u64>, message: String },
    Goodbye { reason: String },
}

struct SessionInfo {
    id: u64,
    label: String,
    cwd: PathBuf,
    dangerous: bool,
    resume_id: Option<String>,
    rows: u16,
    cols: u16,
    spawned_at_ms: u64,
    last_active_ms: u64,
    status: ClaudeStatus,
    permission_pending: bool,
}
```

Each `Request` from a client gets an optional `request_id` (u64, monotonic per connection). Responses correlate via that id when relevant. Pure events (FrameDelta, StatusUpdate) carry no id.

### Input is the only PTY-write channel

The TUI never sends "Mouse" or "Wheel" or "Paste" as protocol variants. Every byte the PTY should receive — keystrokes, SGR mouse encodings, OSC paste brackets, raw escape sequences — is wrapped in `Input { bytes }` after the TUI has already encoded it locally.

The TUI knows the current `TermMode` because it runs its own `Processor` over the FrameDelta stream, so the term-mode-dependent encoding for mouse (SGR 1006 vs X10 vs alt-scroll arrows) is decided client-side. Daemon doesn't reason about input semantics; it just `master.write(bytes)`. This keeps the protocol surface small and avoids the daemon and TUI ever disagreeing on encoding strategy.

### Snapshot vs delta semantics

- On `Attach` the daemon sends one `Snapshot` carrying a fully serialized `alacritty_terminal::Term` (the crate already has the `serde` feature on by default — verified in `Cargo.toml:35: default = ["serde"]` and serde-derive on `Grid`, `Cell`, `Cursor`, `Term`).
- The TUI deserializes into its own local `Term` and renders.
- Daemon keeps a per-client byte queue of *raw PTY output emitted since snapshot*. Each `FrameDelta` carries a chunk; TUI feeds it through its local `Processor` so the local `Term` stays in lockstep.
- Re-sync trigger: if TUI detects a gap (sequence id mismatch) it requests a fresh `Attach`.

### Why bytes, not parsed events?

Snapshot+byte-stream is simpler than serializing parse events:
- Same byte stream the kernel sends to claude's PTY master.
- No risk of the daemon's parser diverging from the client's (one canonical state lives on the daemon, but the client also reproduces it locally → no per-frame full Term serialization).
- Compresses well if we ever want to.
- Allows the client to render at its own pace independent of daemon.

---

## 7. Daemon internals

### Threading model

Tokio multi-thread runtime. Per-session task graph:

```
session task (one per claude PTY)
├── reader task:  loop { master.read(buf) → broadcast::send(bytes) }
├── writer task:  loop { input_rx.recv() → master.write(bytes) }
├── parser task:  loop { byte_rx.recv() → term.lock().process(bytes); dirty.set() }
└── status task:  loop { sleep 500ms → poll claude session JSON, update SessionInfo }
```

Per-client connection task:

```
client task
├── inbound: read framed Request from socket → dispatch to session manager
└── outbound: subscribe broadcast channel per attached session, forward as FrameDelta
```

Concurrency primitives:
- `tokio::sync::broadcast::channel<Bytes>` per session: reader fans out PTY bytes to N attached clients **and** to the parser task. Bounded (e.g. 1024 chunks); lagging clients drop frames and get a `Resync` event triggering a fresh snapshot.
- `tokio::sync::Mutex<Term>` per session, locked **only** by the parser task during `Processor::advance(bytes)`. Snapshot task **does not** take the parser mutex (see below).
- `tokio::sync::watch::channel<SessionInfo>` per session for status updates.

### Snapshot concurrency

The parser task holds the `Mutex<Term>` continuously while processing incoming PTY bursts. Serializing the Term while holding that lock would stall parsing under load.

Strategy:

1. Parser task, after each `Processor::advance`, also sets a per-session `dirty: AtomicBool` and a `snapshot_buffer: tokio::sync::watch::Sender<Arc<Term>>` to a freshly-cloned Term (cheap because `Cell` is `Clone` and `Grid` storage is `Storage<T>` which is `Vec`-based; the clone is a memcpy of cell data).
2. Snapshot task wakes on a 5s interval. If `dirty` is set, it pulls the latest `Arc<Term>` from the watch channel, clears `dirty`, then serializes the clone off the hot path. Parser task never blocks on serialization.

`Term::clone` cost: ~12k cells × ~50 bytes × scrollback factor ≈ 4–40 MiB depending on history. Memcpy of contiguous storage, ~1 ms in practice for 4096-line scrollback. Acceptable. Benchmark in phase 5 if it regresses.

### PTY ownership

- Daemon spawns claude via `portable_pty::native_pty_system().openpty(...)` exactly as the current `Session::spawn` does.
- `MasterPty: Send` (verified: portable-pty 0.9 `lib.rs:88`), readers/writers are `Box<dyn Read+Send>` / `Box<dyn Write+Send>` (`lib.rs:97-102`). All `tokio::task::spawn_blocking`-friendly.
- Reader thread is the same blocking-read loop wrapped in `spawn_blocking`. (Tokio doesn't have async PTY reads on Linux without epoll on raw fd. spawn_blocking is the pragmatic answer.)
- Resize hits both `master.resize(PtySize {..})` and `term.resize(TermSize {..})` — unchanged from today.

### Status JSON polling

Daemon owns this entirely now. The `~/.claude/sessions/<pid>.json` reads move off the TUI thread. Daemon emits `StatusUpdate` events when fields change; TUI just renders.

---

## 8. Persistence & crash recovery

### Two layers

1. **Session manifest** — `~/.config/cmux/state.json`, same shape as today (`PersistedSession[]`). Daemon owns the writer (debounced, just like current TUI does). Survives daemon crash → next daemon start respawns each entry via `claude --resume <id>`. **This is the only persistence cmux has today.** Daemon inherits it.

2. **Optional Term snapshot** — `~/.cache/cmux/snapshots/<session_id>.term.bin` (postcard preferred for size). Written every 5 s by the snapshot task (see §7 "Snapshot concurrency"), or on graceful shutdown. On daemon restart, deserialize and seed the per-session `Term`; **then** spawn claude with `--resume`. End result: TUI client sees the last-known grid contents *and* claude resumes its conversation. Loss window = snapshot interval (default: 5 s).

### Snapshot size discipline

Worst case: 200 cols × (30 viewport + 4096 scrollback) lines × ~50 bytes per cell ≈ 40 MiB / session uncompressed. Times 5 sessions × 12 writes/min = 2.4 GB/min disk churn. Unacceptable.

Bounds applied:
- Snapshot only `scrollback_history.min(SNAPSHOT_HISTORY_CAP)` lines (default 512). Visible 30 lines + 512 history = 542 lines ≈ 5 MiB / session. Compresses to <1 MiB with postcard. The on-disk file replaces the previous (single file per session, not append-log).
- Snapshot only when `dirty` flag set since last write (idle sessions skip).
- One snapshot path per session, atomic rename (`write to .tmp, fsync, rename`).
- `Term::clone` skips alt-screen alt grid if not active (alacritty's `inactive_grid` is dead state when primary is shown).

### Decisions

- Snapshots are best-effort. If the binary format changes between cmux versions, daemon discards the snapshot and falls back to a fresh `Term`. Document `CMUX_SNAPSHOT_FORMAT_VERSION` baked into the file header.
- Daemon writes snapshots only when `Term::dirty` flag set since last snapshot (cheap).
- No file locking: one daemon = one writer.
- Schema migration: when format version bumps, walk a small upgrade table; if no path, delete + warn.

### What's NOT recovered

- The PTY process itself (claude child) is always fresh after a daemon restart — no equivalent of CRIU. The `--resume` behaviour of claude restores the conversation; cmux restores the grid above it. Any half-streamed tool invocation mid-PTY-write is lost. Acceptable.

---

## 9. Migration plan (current code → workspace)

Single-PR-friendly path:

### Phase 1 — workspace split (no behaviour change)

- `cargo new --lib crates/cmux-proto`, `cargo new --bin crates/cmuxd`, move existing src → `crates/cmux`.
- Update root `Cargo.toml` with `[workspace] members = [...]`.
- All current binaries build untouched (`cargo build -p cmux`).
- `cmuxd` is a stub `fn main() { eprintln!("not implemented"); }`.
- `cmux-proto` only contains the enum skeletons + `Frame` length-prefix codec.

### Phase 2 — proto + daemon skeleton

- Implement `Request` / `Event` types, framed codec.
- `cmuxd`: tokio runtime, listen on socket, parse one Hello, echo back Welcome, close. Zero session ownership yet.
- Unit tests round-trip every variant via `serde_json`.

### Phase 3 — single-session daemon

- Daemon can `SpawnSession`, hold PTY, parser, status, broadcast PTY bytes.
- TUI gains `--connect` flag (off by default) that wires the rendering layer to a daemon connection.
- Both code paths coexist; default still in-process.

### Phase 4 — feature parity

- All current operations (Detach, Rename, Reorder, Scroll, Resume picker, Spawn picker) wired through daemon.
- Mouse / OSC52 stay client-side. TUI encodes mouse events as SGR/X10/arrow bytes locally (using its own `Term::mode()` mirror) and sends via `Input`. The daemon never has a "Mouse" Request variant.
- Persistence moves to daemon; TUI's `flush_persist` is deleted.
- `--connect` becomes the default. `--no-daemon` retained as escape hatch for two more releases.
- **Dual code paths during phases 3+4:** maintaining both in-process and daemon-backed paths costs bookkeeping. Each new handler change must be applied to both branches until phase 5 drops `--no-daemon`. Plan budget: 3 dev-days for phase 3, 3 dev-days for phase 4 (revised up from original "2+2").

### Phase 5 — snapshot + reconnect

- Wire up periodic Term snapshot + restore on daemon start.
- Add `cmuxctl` admin binary (cli wrapper for daemon Requests: `cmuxctl list`, `cmuxctl shutdown`, `cmuxctl tail <id>`).
- Drop `--no-daemon` and remove the in-process code path.

Each phase ships independently. Phase 1 alone takes a couple of hours; phase 4 is the bulk of the work.

---

## 10. `cmuxctl` (admin CLI, phase 5)

Small companion binary in the workspace. Sends single Requests, prints JSON or pretty output. Useful for:

```
cmuxctl list                          # SessionInfo table
cmuxctl spawn ~/proj                  # returns id
cmuxctl kill 3
cmuxctl tail 1                        # stream FrameDelta as raw bytes
cmuxctl shutdown                      # daemon Goodbye + exit
cmuxctl status                        # uptime, mem, session count
```

Same crate as `cmuxd` to share types. Doesn't need ratatui.

---

## 11. Testing strategy

| Layer | Test |
|---|---|
| `cmux-proto` | round-trip every variant through `serde_json::to_vec` + `from_slice`; framing codec hands wraps + corrupt-length cases |
| `cmuxd` session task | spawn `bash -c 'printf foo; read; printf bar'`, attach virtual client, assert Snapshot+FrameDelta sequence reproduces `"foo"` then `"bar"` after writing input |
| `cmuxd` multi-client | two clients attach the same session, both receive identical FrameDelta stream within ε of arrival time |
| `cmuxd` restart | spawn 3 sessions, SIGTERM daemon, restart, verify snapshots restored + claude resumed |
| `cmuxd` snapshot bound | spawn session, fill scrollback, assert snapshot file on disk < 8 MiB and `Term::clone` measured < 5 ms |
| Integration | spawn an actual `claude --help`-style child, complete a roundtrip via the real socket |
| **Mouse forwarding** | TUI dispatches synthetic wheel/click/drag events; PTY receiver (under daemon) asserts exact byte sequence matches xterm SGR 1006 spec. Catches encoding bugs the property test misses because the property test generates bytes directly, skipping the TUI's mouse-to-SGR encoder. |
| Property test | random byte sequences fed in via `Input`, assert local TUI `Term` state equals daemon's after snapshot+delta replay (uses alacritty's `Term::grid` equality) |

CI changes: `cargo test --workspace`. Add `cargo build --release --workspace` to the build matrix.

---

## 12. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Tokio adds 50+ transitive deps and ~1MB binary | Use `tokio = { features = ["rt-multi-thread","net","io-util","macros","sync","time","fs"] }`; skip `full`. Daemon binary only. TUI stays sync. |
| Broadcast channel lag drops bytes on slow clients | Per-client byte queue with bounded capacity; on overflow send `Resync` causing fresh `Snapshot` (one-time cost, no permanent divergence) |
| Daemon crash with sessions running | Children become reparented to init. They survive but are orphaned (claude continues to print into a closed master). Next daemon start can't recover them — they're effectively dead. Mitigation: snapshot before any daemon code path that could panic; document that this scenario re-runs `--resume` on restart, losing in-flight state. |
| Socket permission leak | Open with `umask(0o077)`, then `chmod(sock_path, 0o600)`; verify owner equals euid on TUI connect. |
| Version skew between TUI 0.2 and daemon 0.1 | `Hello { want_protocol: u32 }` + `Welcome` server-side check; mismatch → daemon sends `Goodbye{reason:"protocol skew"}` and TUI offers to restart daemon (with prompt). |
| Two TUIs both try to bind on first launch | `fcntl(F_SETLK)` on lockfile; loser polls socket. |
| Alacritty `Term` serde representation changes between versions | Bake `alacritty_terminal::CRATE_VERSION` + cmux internal format version into snapshot header; mismatch → discard. |
| Daemon-spawned claude inherits weird env | Daemon strips terminal-specific env (TERM_PROGRAM, COLORTERM…) and sets a stable subset (TERM=xterm-256color, plus the cwd-relevant vars). Document and snapshot the env it uses. |
| Multiple users on one box | Daemon binds in `$XDG_RUNTIME_DIR/cmux/cmuxd.sock` which is `$UID`-scoped. No cross-user collision. |
| TUI hot reload (replace binary mid-attach) | Works naturally: TUI exits, daemon keeps sessions, new TUI binary connects. This is a *feature* of the design. |
| `claude --resume <id>` running outside daemon while daemon respawns same id | Two `claude` processes with the same session id would corrupt the transcript. Mitigation: daemon checks for an existing `claude` process holding `~/.claude/projects/<slug>/<id>.jsonl` (via `fuser` or simple PID file) before respawning. If found, mark session as `Detached(External)` and refuse local respawn until the external process exits. Verify claude's own locking behavior in phase 3 before committing to the respawn-on-restart path. |
| `Term::clone` cost regression after alacritty crate upgrade | Phase-5 microbenchmark gates the snapshot interval. If `Term::clone` exceeds 10 ms for 4096-line scrollback, raise snapshot interval to 15 s or shrink `SNAPSHOT_HISTORY_CAP`. |

---

## 13. Open questions (decide during phase 2)

1. **Wire format**: stick with framed JSON, or move to postcard once protocol stabilizes? JSON wins on debuggability; postcard wins on bandwidth (~5× smaller in worst case).
2. **One daemon per cwd workspace, or one per user?** Current plan: one per user. Workspaces don't matter — sessions carry cwd.
3. **`cmuxctl` separate binary vs `cmux ctl …` subcommand?** Subcommand is simpler; separate binary is clearer. Lean subcommand.
4. **Idle shutdown default**: never (current plan) vs after 24h with zero sessions. Never.
5. **Logs**: `~/.local/state/cmux/cmuxd.log` rotated daily? Or systemd journal? Keep it simple — log to stderr while attached to terminal, redirect to file when daemonized.
6. **Resize broadcast policy**: when client A resizes a session client B is also attached to, what does B see? Current plan: daemon honors the **last** resize; all attached clients agree to render at that geometry; mismatched clients see letterboxing. Document this.
7. **claude --resume lock semantics**: claude itself may not lock the transcript file. Verify in phase 3 with two concurrent `claude --resume <id>` instances on the same id — does claude detect, refuse, or silently corrupt? Answer determines whether the daemon needs its own session-id mutex layer (PID file in `~/.cache/cmux/sessions/<id>.pid`) or can rely on claude's behaviour.

---

## 14. Effort estimate

| Phase | Net change | Time |
|---|---|---|
| 1 – workspace split | ~50 lines moved, Cargo.tomls | 1–2 h |
| 2 – proto crate + daemon stub | ~400 lines | 1 day |
| 3 – single-session through daemon (with dual-path overhead) | ~800 lines | 3 days |
| 4 – feature parity & default-on (with dual-path bookkeeping) | ~600 lines + 300 deleted | 3 days |
| 5 – snapshot + reconnect + cmuxctl + benchmarks | ~500 lines | 1.5 days |
| **Total** | **~2300 new lines, ~300 removed** | **~9 dev-days** |

Real calendar time will be longer once edge cases (TUI hot-attach race, multi-client resize semantics, snapshot format) surface.

---

## 15. Acceptance criteria (definition of done)

- `cmux` opens — daemon auto-spawns. Lock-file + ready-stamp pattern verified by stracing the first launch.
- Spawn 3 sessions, type in each, close `cmux` window with Ctrl-Q.
- Re-run `cmux`. All 3 sessions present in sidebar, scrollback intact, claude conversation resumes via `--resume`.
- `pkill -9 cmuxd && cmux` — sessions list survives (claude conversations resume via the existing manifest path); recent scrollback may be a few seconds stale; cmux still works.
- Two `cmux` instances open simultaneously can both attach to the same session; both see each other's typing in real time. Resize from one updates the other (last-writer-wins per §13.6). Confirmed visually.
- Mouse drag-select on one client copies to that client's clipboard via OSC 52; the other client's view shows no spurious selection (selection state is client-local, not daemon-shared).
- `cmuxctl shutdown` cleanly stops everything; nothing left in `ps`.
- `cmuxctl status` reports per-session: `pid`, `uptime`, `last_snapshot_age_ms`, `rss_bytes`. Used to verify snapshot cadence and memory bounds.
- `--no-daemon` (phase 4 transitional flag) makes cmux behave exactly as today.
- Snapshot file for any session never exceeds 8 MiB (per §8 bounds).
- Mouse wheel inside focused tile produces well-formed SGR 1006 bytes at the PTY (verified via the integration test in §11).

When all of the above pass, ship it.
