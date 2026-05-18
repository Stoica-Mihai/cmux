# tmux-claude

A tmux-like TUI for running and watching many `claude` sessions side-by-side, each rooted in a different folder, with a per-session `--dangerously-skip-permissions` toggle.

Pure Rust. No tmux required. Sessions are ephemeral — they die when the TUI exits.

## Build

```
cargo build --release
./target/release/tmux-claude
```

## Keys

Dashboard is always interactive — keystrokes are forwarded to the focused session's `claude` preview. Navigation, spawning, and quitting all go through a tmux-style prefix: **`Ctrl+A`** then a letter. Bare letters are never bindings, so typing words containing `d`/`q`/etc. is safe.

### Prefix chords (`Ctrl+A` then...)
| Chord | Action |
|---|---|
| `n` | Spawn a new `claude` session (opens dialog) |
| `l` | Open resume picker (past sessions from `~/.claude/projects`) |
| `↓` | Focus next session |
| `↑` | Focus previous session |
| `1`-`9` | Jump focus to session N |
| `r` | Rename focused session |
| `d` | Detach focused session (asks for confirm; terminates `claude`, removes tile) |
| `z` | Toggle sidebar (hide for maximum preview width) |
| `a` | Send a literal `Ctrl+A` to the focused session |
| `q` | Quit (terminates all sessions) |

`Ctrl+Q` = hard quit from anywhere.

### Spawn dialog
| Key | Action |
|---|---|
| typing | Edit cwd path |
| `Tab` | Path completion |
| `Space` | Toggle `--dangerously-skip-permissions` |
| `Enter` | Spawn |
| `Esc` | Cancel |

## Architecture

- `portable-pty` spawns each `claude` instance into a real PTY in its chosen cwd.
- A dedicated reader thread per session blocks on `read()` and feeds bytes into a `vt100::Parser`.
- `tui-term`'s `PseudoTerminal` widget renders each parser's `Screen` into a `ratatui` tile.
- After every draw, actual rendered tile sizes are pushed back to each PTY via `MasterPty::resize` and `Screen::set_size` so claude sees correct dimensions even as the grid reshapes.
- Sessions whose child exits are reaped on the next event-loop tick.

## Known limitations

- vt100 is a conservative parser. If claude uses sequences it doesn't model (kitty graphics, OSC clipboard, etc.) those will be dropped silently. Reported missing visuals → swap to `alacritty_terminal` + custom renderer.
- Sessions don't survive TUI restart. (Ephemeral by design — picked over a daemon backend.)
- No mouse forwarding inside zoomed sessions.
- Small preview tiles (e.g. 4 sessions at 80×24 → ~38×10 each) — claude UI is not really readable at that size; preview is for "is it idle / waiting / running" awareness, then zoom in.
