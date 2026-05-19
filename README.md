<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.svg">
    <img alt="cmux" src="assets/logo-light.svg" width="280">
  </picture>
</p>

# cmux

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

- `claude` CLI on `PATH`.
- A terminal with raw-mode + alternate-screen support (any modern terminal).

## Build / install

```
# both binaries, release
cargo build --release --workspace

# from workspace root
target/release/cmux           # TUI (local mode)
target/release/cmuxd          # daemon
```

Or with cargo:

```
cargo run -p cmux             # TUI
cargo run -p cmuxd            # daemon
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
| `cmux ctl list` | Print every session the daemon is hosting |
| `cmux ctl status` | Session count summary |
| `cmux ctl kill <id>` | Detach + kill the given session id |
| `cmux ctl shutdown` | Stop the daemon (kills every session it owns) |

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
