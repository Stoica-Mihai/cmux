# tmux-claude

A tmux-like TUI for running and watching many `claude` sessions side-by-side, each rooted in a different folder, with a per-session `--dangerously-skip-permissions` toggle.

Pure Rust. No tmux required.

Sessions are restored across TUI restarts: the session list (cwd, label, dangerous flag, resume id, sidebar state) is persisted to `$XDG_CONFIG_HOME/tmux-claude/state.json` (fallback `~/.config/tmux-claude/state.json`). On launch each saved session is respawned, using `claude --resume <id>` when a resume id is known so transcript history reattaches.

## Requirements

- `claude` CLI on `PATH`.
- A terminal with raw-mode + alternate-screen support (any modern terminal).

## Install

```
cargo install --path .
```

Or build directly:

```
cargo build --release
./target/release/tmux-claude
```

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

Path: `$XDG_CONFIG_HOME/tmux-claude/state.json` (fallback `~/.config/tmux-claude/state.json`).

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
| `TMUX_CLAUDE_DEBUG=1` | Append every key event + chord transition to `/tmp/tmux-claude-keys.log` |

## Architecture

- `portable-pty` spawns each `claude` instance into a real PTY in its chosen cwd.
- A dedicated reader thread per session blocks on `read()` and feeds bytes into a `vt100::Parser`.
- `tui-term`'s `PseudoTerminal` widget renders each parser's `Screen` into a `ratatui` tile.
- After every draw, actual rendered tile sizes are pushed back to each PTY via `MasterPty::resize` and `Screen::set_size` so claude sees correct dimensions even as the grid reshapes.
- Sessions whose child exits are reaped on the next event-loop tick.
- Resume uses `claude --resume <session_id>`; the id is extracted from the transcript path under `~/.claude/projects/<slugified-cwd>/<id>.jsonl`.

## Dependencies

`portable-pty`, `vt100`, `tui-term`, `ratatui`, `crossterm`, `serde`, `serde_json`, `anyhow`.

## Known limitations

- `vt100` is a conservative parser. If claude uses sequences it doesn't model (kitty graphics, OSC clipboard, sixel, etc.) those will be dropped silently. Reported missing visuals → swap to `alacritty_terminal` + custom renderer.
- No mouse forwarding inside zoomed sessions.
- Small preview tiles (e.g. 4 sessions at 80×24 → ~38×10 each) — claude UI is not really readable at that size; preview is for "is it idle / waiting / running" awareness, then zoom in.
- Persisted sessions reattach by spawning a fresh `claude --resume <id>`; the PTY itself is not preserved across restarts, only the conversation.
