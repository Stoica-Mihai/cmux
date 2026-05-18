# cmux

A tmux-like TUI for running and watching many `claude` sessions side-by-side, each rooted in a different folder, with a per-session `--dangerously-skip-permissions` toggle.

Pure Rust. No tmux required.

Sessions are restored across TUI restarts: the session list (cwd, label, dangerous flag, resume id, sidebar state) is persisted to `$XDG_CONFIG_HOME/cmux/state.json` (fallback `~/.config/cmux/state.json`). On launch each saved session is respawned, using `claude --resume <id>` when a resume id is known so transcript history reattaches.

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
./target/release/cmux
```

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

- `portable-pty` spawns each `claude` instance into a real PTY in its chosen cwd.
- A dedicated reader thread per session blocks on `read()` and feeds bytes into an `alacritty_terminal::Term` via `vte::ansi::Processor::advance`.
- A custom `TermWidget` (`src/term_render.rs`) walks the term's `display_iter` and writes each `Cell` into ratatui's `Buffer`, mapping alacritty SGR flags (bold/dim/italic/underline/strikeout/inverse) and color (`Named` / `Spec(Rgb)` / `Indexed`) onto ratatui styles.
- After every draw, actual rendered tile sizes are pushed back to each PTY via `MasterPty::resize` and `Term::resize` so claude sees correct dimensions even as the grid reshapes.
- Scrollback is driven through `Term::scroll_display(Scroll::Delta | PageUp | PageDown | Top | Bottom)`.
- Sessions whose child exits are reaped on the next event-loop tick.
- Resume uses `claude --resume <session_id>`; the id is extracted from the transcript path under `~/.claude/projects/<slugified-cwd>/<id>.jsonl`.

## Dependencies

`alacritty_terminal` (via re-exported `vte` 0.15), `portable-pty`, `ratatui`, `crossterm`, `serde`, `serde_json`, `anyhow`.

## Known limitations

- `alacritty_terminal` parses VT-text-class sequences faithfully (full xterm SGR, OSC 8 hyperlinks, OSC 52 clipboard requests, synchronized output, bracketed paste, mouse SGR), but image-class protocols are out of scope: sixel, kitty graphics, iTerm2 inline images are silently dropped. Adding any of those requires a passthrough render path that bypasses the cell grid for the focused tile.
- Custom renderer collapses each cell into a single ratatui buffer cell. Wide CJK chars render correctly but combining marks beyond the base char (zerowidth extras) are dropped. OSC 8 hyperlink cells render but the link itself is not emitted via OSC 8 to the outer terminal.
- No mouse forwarding inside zoomed sessions.
- Small preview tiles (e.g. 4 sessions at 80×24 → ~38×10 each) — claude UI is not really readable at that size; preview is for "is it idle / waiting / running" awareness, then zoom in.
- Persisted sessions reattach by spawning a fresh `claude --resume <id>`; the PTY itself is not preserved across restarts, only the conversation.
