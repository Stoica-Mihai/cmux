<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.svg">
    <img alt="cmux" src="assets/logo-light.svg" width="280">
  </picture>
</p>

A tmux-like TUI for running and watching many `claude` sessions side-by-side, each rooted in a different folder, with a per-session `--dangerously-skip-permissions` toggle.

Pure Rust. No tmux required.

Two backends:

- **Local (default)** — `cmux` owns each `claude` PTY itself. When `cmux` exits, sessions die. The session list (cwd, label, dangerous flag, resume id, sidebar state) is persisted to `$XDG_CONFIG_HOME/cmux/state.json`; on next launch each saved entry is respawned with `claude --resume <id>` so the conversation reattaches at the application layer.
- **Daemon (`cmux --connect`)** — a long-lived `cmuxd` process owns the PTYs. `cmux` becomes a thin client: it renders, forwards keys/mouse/resize, and copies on selection. Close `cmux` and the `claude` processes stay running inside `cmuxd`. Reopen `cmux --connect` and every session reattaches with its live scrollback intact. Title bar shows a green `cmuxd` chip whenever you're in this mode.

## Repo layout (Cargo workspace)

```
crates/
├── cmux/         # TUI binary
├── cmux-proto/   # wire types + framed JSON codec
└── cmuxd/        # daemon binary
```

## Requirements

- `claude` CLI on `PATH` — required by the TUI, and by `cmux ctl spawn` when you
  don't pass your own command. `cmuxd` itself has no such requirement.
- A terminal with raw-mode + alternate-screen support (any modern terminal).

## Install

```
make install
```

Builds in release, then installs `cmux` and `cmuxd` into `$CARGO_HOME/bin`
(usually `~/.cargo/bin`) — make sure that directory is on your `PATH`.
`make uninstall` removes them again.

Both binaries matter: `cmux --connect` auto-spawns `cmuxd`, looking next to its
own executable first and then on `PATH`.

Note that `cargo install --path .` does **not** work from the workspace root —
that manifest is virtual, with no package of its own — so each binary crate is
installed by its own path:

```
cargo install --path crates/cmux  --locked
cargo install --path crates/cmuxd --locked
```

### Make targets

| Target | Action |
|---|---|
| `make build` | release binaries into `target/release` |
| `make install` | `build`, then install both binaries |
| `make uninstall` | remove both binaries |
| `make test` | `cargo test --workspace` |
| `make check` | everything CI runs: fmt, clippy, build, test |
| `make smoke` | end-to-end test against a real daemon |
| `make demo` | rendered walkthrough of the TUI |
| `make clean` | `cargo clean` |

## Run from a checkout

```
cargo build --release --workspace

target/release/cmux              # TUI (local mode)
target/release/cmuxd             # daemon
```

Or without building first:

```
cargo run -p cmux                # TUI
cargo run -p cmuxd               # daemon
cargo run -p cmux -- --connect   # TUI in daemon-backed mode
cargo run -p cmux -- ctl list    # admin CLI
```

## Daemon mode

Persistent sessions across `cmux` exits.

```
cmux --connect
```

That's it — if no `cmuxd` is running, `cmux --connect` auto-spawns one in the background (binary next to `cmux`, then `$PATH`) and waits up to 2 s for its socket. To run the daemon manually, just `cmuxd` in a separate terminal.

`cmux` connects to `$XDG_RUNTIME_DIR/cmux/cmuxd.sock` (fallback `/tmp/cmux-<uid>/cmux/`), lists existing sessions, and hydrates the sidebar. New sessions you spawn (`Ctrl+A n`) go through the daemon. Quitting `cmux` (`Ctrl+Q`) sends `Detach { keep_session: true }` for every session — `cmuxd` keeps them alive.

### Visual cues

- Title bar shows a green `cmuxd` chip whenever the TUI is daemon-backed; dim `local` text otherwise.
- If `cmuxd` dies mid-session, the TUI dims, shows a centered "Daemon disconnected" modal, and exits on any keypress. Sessions remain on disk for the next `cmux --connect`.

### `cmux ctl` admin commands

| Command | Action |
|---|---|
| `cmux ctl list` | Print every session the daemon is hosting, with its argv and grid size |
| `cmux ctl status` | Session count summary |
| `cmux ctl spawn <dir>` | Start a `claude` session in `<dir>` (`--dangerous`, `--label`) |
| `cmux ctl spawn <dir> -- <cmd...>` | Start any command instead of `claude` |
| `cmux ctl kill <id>` | Detach + kill the given session id |
| `cmux ctl shutdown` | Stop the daemon (kills every session it owns) |

`cmuxd` is command-agnostic: it execs whatever argv it is handed, so a shell,
a REPL, or a long-running job are all valid sessions.

```
cmux ctl spawn ~/src/app -- bash -l
cmux ctl spawn ~/src/app -- cargo watch -x test
```

Status reporting is per-session. A `claude` session gets a probe that reads
`~/.claude/sessions/<pid>.json` and watches for permission prompts (the `⚠` in
`ctl list`); a session started with your own command gets no probe and no
polling.

## HTTP API

The unix socket speaks a length-prefixed JSON protocol that only `cmux`
implements, so reading a session or driving it meant writing a client. Pass
`--http` and the same daemon is reachable over plain HTTP: a script, a browser
or an agent can list sessions, read what is on a screen, send input and stream
output without touching that protocol.

```
cmuxd --http                  # 127.0.0.1:7070
cmuxd --http 127.0.0.1:9000
cmuxd --http 127.0.0.1:0      # let the kernel pick; the daemon prints the port
```

It prints the address it bound:

```
cmuxd http api on http://127.0.0.1:7070
  no authentication: whoever reaches this port runs commands as you.
  front it with a tunnel or an authenticating proxy before exposing it.
```

### The terminal and the browser mirror each other

They are not two views of two things. `cmux --connect` and a browser tab attach
to the *same* PTYs in the same daemon, and both read and write: type in the
terminal and it appears in the browser, type in the browser and it appears in
the terminal. Leave your desk mid-session, open the page on a phone, keep going.

If you want the browser available without planning ahead, have `cmux` start the
daemon with the API already on:

```sh
cmux --connect --http               # 127.0.0.1:7070
cmux --connect --http 127.0.0.1:9000
```

That flag only applies when this command is the one that starts the daemon. A
daemon already running keeps whatever it was launched with, so for an
always-available API start `cmuxd --http` from your shell profile or a systemd
user unit and let `cmux --connect` find it.

**Grid size with more than one client attached.** Each client reports its own
size and the PTY runs at the smallest, exactly as tmux does. A phone at 24x80
beside a wide terminal pins the session to 24x80 while it is attached, and the
session grows back when it detaches. Without this the two clients fight, last
writer winning, and whichever lost renders a clipped grid.

### Access control is yours, not the daemon's

**cmuxd does no authentication.** A session is an arbitrary command, so anything
that can reach the port runs code as your user. The daemon's job is brokering
PTYs; deciding who may reach it belongs to whatever you put in front, which does
that job properly — real credentials, TLS, revocation — instead of a second,
weaker copy inside the daemon.

The default bind is loopback. That is not a security control, it is the address
a tunnel connects to.

**An SSH tunnel**, if you already have a login on the machine:

```sh
# on cmuxd's machine
cmuxd --http 127.0.0.1:7070

# from your laptop
ssh -N -L 7070:127.0.0.1:7070 you@that-machine
# then open http://127.0.0.1:7070
```

**A peer-to-peer tunnel**, with no public IP or port forwarding:

```sh
npm i -g holesail
holesail --live 7070          # on cmuxd's machine; prints a connector key
holesail --connect <key>      # anywhere else
```

**A reverse proxy**, to put more than one person on it: terminate TLS and
authenticate there, then proxy `/api`, `/ws` and `/`. The WebSocket route needs
the `Upgrade` and `Connection` headers passed through.

Whichever you pick, leave cmuxd bound to loopback so the only way in is the one
you authenticated.

| Method | Path | Does |
|---|---|---|
| `GET` | `/api/health` | version, protocol, session count |
| `GET` | `/api/sessions` | every session, ordered by id |
| `POST` | `/api/sessions` | spawn; body `{cmd, cwd?, label?, probe?, rows?, cols?}` |
| `GET` | `/api/sessions/{id}` | one session's info |
| `DELETE` | `/api/sessions/{id}` | kill it |
| `GET` | `/api/sessions/{id}/screen` | the visible grid as **plain text** |
| `GET` | `/api/sessions/{id}/buffer` | the raw replay ring, escapes included |
| `POST` | `/api/sessions/{id}/input` | body bytes go to the PTY verbatim |
| `POST` | `/api/sessions/{id}/resize` | body `{rows, cols}`; sets the size used while nothing is attached, and answers `409` when a client is, since the minimum governs then |
| `GET` | `/ws/sessions/{id}` | WebSocket: raw bytes out; in, a command byte then payload |
| `GET` | `/` | browser terminal |

The WebSocket sends raw PTY bytes to the client. Messages the other way start
with a command byte, so a resize is never mistaken for something to type:

| Message | Means |
|---|---|
| `0` + bytes | input, passed to the PTY verbatim |
| `1` + `{"rows":R,"cols":C}` | this client's grid size |

### From a shell

```sh
curl localhost:7070/api/sessions

curl -H 'Content-Type: application/json' \
  -d '{"cmd":["python3","-q"],"label":"repl"}' \
  localhost:7070/api/sessions

curl --data-binary $'print(2**10)\n' localhost:7070/api/sessions/1/input
curl localhost:7070/api/sessions/1/screen
```

Reach for `screen` first — it hands back the terminal as text, with no escape
sequences to parse.

### In a browser

Open the address the daemon printed. The page lists sessions, attaches to one
over the WebSocket, forwards keystrokes and pushes the rendered size back to the
PTY. `?session=N` opens a specific one; otherwise it attaches to the first. The
renderer is xterm.js from a CDN, so that page needs internet even though the
daemon does not.

## Selecting text

Click and drag inside the focused tile to select claude's output. On release, the selection is copied to your system clipboard via OSC 52. Most modern terminals (kitty, ghostty, wezterm, foot, alacritty, recent gnome-terminal, iTerm2) respect OSC 52 by default.

cmux captures the mouse, so the outer terminal's native drag-select is disabled while cmux is running. Hold **Shift** while dragging to bypass cmux and fall back to the outer terminal's selection (useful when OSC 52 isn't honored).

## Keys

The dashboard is always interactive — keystrokes are forwarded to the focused session's `claude` preview. Navigation, spawning, and quitting all go through a tmux-style prefix: **`Ctrl+A`** then a letter. Bare letters are never bindings, so typing words containing `d`/`q`/etc. is safe.

### Prefix chords (`Ctrl+A` then...)
| Chord | Action |
|---|---|
| `n` | Spawn a new `claude` session (opens directory browser) |
| `l` | Open resume picker (past sessions from `~/.claude/projects`) |
| `↓` | Focus next session |
| `↑` | Focus previous session |
| `1`-`9` | Jump focus to session N |
| `0` | Jump focus to session 10 |
| `r` | Rename focused session |
| `m` | Enter reorder mode (move focused session up/down) |
| `d` | Detach focused session (asks for confirm; terminates `claude`, removes tile) |
| `z` | Toggle sidebar (hide for maximum preview width) |
| `[` | Enter scrollback mode on focused session |
| `a` | Send a literal `Ctrl+A` to the focused session |
| `?` | Help overlay |
| `q` | Quit (terminates all sessions) |

`Ctrl+Q` = hard quit from anywhere.

### Spawn dialog (directory browser)
| Key | Action |
|---|---|
| `↑` / `k` | Select previous directory |
| `↓` / `j` | Select next directory |
| `→` / `l` / `Enter` | Descend into selected directory |
| `←` / `h` / `Backspace` | Ascend to parent |
| `PageUp` / `PageDown` / `Home` / `End` | Bulk navigation |
| `Space` | Toggle `--dangerously-skip-permissions` |
| `Enter` | Spawn `claude` in the highlighted directory |
| `Esc` | Cancel |

Hidden directories (names starting with `.`) are filtered from the listing.

### Resume picker
| Key | Action |
|---|---|
| typing | Filter by cwd substring (case-insensitive) |
| `Backspace` | Delete filter character |
| `↑` / `↓` | Move selection |
| `PageUp` / `PageDown` | Move by 10 |
| `Home` / `End` | Jump to first / last |
| `Tab` | Toggle `--dangerously-skip-permissions` |
| `Enter` | Resume selected transcript via `claude --resume <id>` |
| `Esc` | Cancel |

Selected transcript shows a preview pane (first ~40 lines).

### Scrollback mode (`Ctrl+A` `[`)
| Key | Action |
|---|---|
| `↑` / `k` | Up one line |
| `↓` / `j` | Down one line |
| `PageUp` / `b` | Up one screen |
| `PageDown` / `f` / `Space` | Down one screen |
| `Home` / `g` | Top |
| `End` / `G` | Bottom (live) |
| `Esc` / `Enter` / `q` | Exit, snap back to live tail |

### Reorder mode (`Ctrl+A` `m`)
| Key | Action |
|---|---|
| `↑` / `k` | Move focused session up |
| `↓` / `j` | Move focused session down |
| `Esc` / `Enter` / `q` | Exit |

### Detach confirm
| Key | Action |
|---|---|
| `y` / `Y` / `Enter` | Confirm detach |
| `n` / `N` / `Esc` | Cancel |

### Rename
| Key | Action |
|---|---|
| typing | Edit label |
| `Backspace` | Delete character |
| `Enter` | Commit (sets `manually_renamed` so future restarts keep this label) |
| `Esc` | Cancel |

## State file

Path: `$XDG_CONFIG_HOME/cmux/state.json` (fallback `~/.config/cmux/state.json`).

Schema:

```json
{
  "show_sidebar": true,
  "sessions": [
    {
      "cwd": "/path/to/project",
      "label": "project",
      "dangerous": false,
      "resume_id": "abc123...",
      "manually_renamed": false
    }
  ]
}
```

Delete the file to start clean.

## Environment

| Variable | Effect |
|---|---|
| `CMUX_DEBUG=1` | Append every key event + chord transition to `/tmp/cmux-keys.log` |

## Architecture

### Local mode (default)
- `portable-pty` spawns each `claude` instance into a real PTY in its chosen cwd.
- A dedicated reader thread per session blocks on `read()` and feeds bytes into an `alacritty_terminal::Term` via `vte::ansi::Processor::advance`.
- A custom `TermWidget` (`crates/cmux/src/term_render.rs`) walks the term's `display_iter` and writes each `Cell` into ratatui's `Buffer`, mapping alacritty SGR flags (bold/dim/italic/underline/strikeout/inverse) and color (`Named` / `Spec(Rgb)` / `Indexed`) onto ratatui styles.
- After every draw, actual rendered tile sizes are pushed back to each PTY via `MasterPty::resize` and `Term::resize` so claude sees correct dimensions even as the grid reshapes.
- Scrollback is driven through `Term::scroll_display(Scroll::Delta | PageUp | PageDown | Top | Bottom)`.
- Sessions whose child exits are reaped on the next event-loop tick.
- Resume uses `claude --resume <session_id>`; the id is extracted from the transcript path under `~/.claude/projects/<slugified-cwd>/<id>.jsonl`.

### Daemon mode (`--connect`)
- `cmuxd` owns every PTY, parser, and alacritty `Term` instance. Runs on a `tokio` multi-thread runtime.
- The daemon knows nothing about `claude`. `Request::SpawnSession` carries the argv to exec plus the client's grid size, and a `ProbeKind` selecting how status is derived. `ProbeKind::Claude` installs the probe in `crates/cmuxd/src/probe.rs`; `ProbeKind::None` runs no probe and starts no polling task. Adding support for another program means adding a `StatusProbe` impl, not touching the session plumbing.
- Communication over a UNIX socket at `$XDG_RUNTIME_DIR/cmux/cmuxd.sock` with file mode `0o600`. Per-message framing is `u32_le length || serde_json payload`.
- Per session inside the daemon: blocking PTY reader → fans bytes into `tokio::sync::broadcast::Sender<Vec<u8>>` so every attached client gets the same byte stream as a `FrameDelta` event.
- `cmux --connect` runs the same TUI / renderer / mouse selection / OSC 52 path as local mode. The only difference is per-Session backend: `Session::Backend::Daemon` routes `write()` / `resize()` / `kill()` / `detach_keep()` to `Request::Input` / `Request::Resize` / `Request::Detach`.
- A connection-side reader thread distributes `FrameDelta` events into per-session `DaemonSlot` Arcs (parser + ring + dirty + alive). UI code reads these unchanged from local mode.

## Dependencies

Workspace: `alacritty_terminal` (via re-exported `vte` 0.15), `portable-pty`, `ratatui`, `crossterm`, `serde`, `serde_json`, `chrono`, `anyhow`. Daemon adds `tokio`, `thiserror`.

## Known limitations

- Image-class terminal protocols are out of scope: sixel, kitty graphics, iTerm2 inline images are silently dropped. Adding any of those requires a passthrough render path that bypasses the cell grid for the focused tile. Text-class sequences (xterm SGR, OSC 8 hyperlinks, OSC 52 clipboard, synchronized output, bracketed paste, mouse SGR) are parsed faithfully.
- OSC 8 hyperlink cells render their visible text but the link itself is not re-emitted to the outer terminal — clicking won't open it.
- Daemon mode survives `cmux` exit but does **not** survive `cmuxd` exit. Snapshot-based restore across daemon restarts is not yet shipped.
