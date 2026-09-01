#!/usr/bin/env bash
# Visual walkthrough: one daemon hosting four different programs, and the TUI
# driving each of them. Renders cmux inside tmux so frames can be captured to
# stdout — useful in a pipe, a log, or a terminal.
#
# Fully isolated: own XDG dirs, own tmux socket, own fake `claude`.
#
#   ./scripts/demo.sh             # uses target/debug (run cargo build first)
#   PROFILE=release ./scripts/demo.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="${PROFILE:-debug}"
CMUX="$REPO/target/$PROFILE/cmux"
CMUXD="$REPO/target/$PROFILE/cmuxd"
TMUX="tmux -L cmuxdemo"

for bin in "$CMUX" "$CMUXD"; do
  [ -x "$bin" ] || { echo "missing $bin — run: cargo build --workspace"; exit 2; }
done
command -v tmux >/dev/null || { echo "this demo renders through tmux; install tmux"; exit 2; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/cmux-demo.XXXXXX")"
export XDG_RUNTIME_DIR="$WORK/run"
export XDG_STATE_HOME="$WORK/state"
export XDG_CONFIG_HOME="$WORK/config"
export PATH="$WORK/bin:$PATH"
mkdir -p "$XDG_RUNTIME_DIR/cmux" "$WORK/bin"

cat > "$WORK/bin/claude" <<'FAKE'
#!/usr/bin/env bash
echo "Claude Code (stand-in, so the demo never starts a real session)"
echo "argv: $*"
echo
while true; do read -r -p "> " line || exit 0; echo "  you said: $line"; done
FAKE
chmod +x "$WORK/bin/claude"

DAEMON_PID=""
cleanup() {
  $TMUX kill-server 2>/dev/null
  [ -n "$DAEMON_PID" ] && {
    "$CMUX" ctl shutdown >/dev/null 2>&1
    sleep 0.4
    kill "$DAEMON_PID" 2>/dev/null
    wait "$DAEMON_PID" 2>/dev/null
  }
  rm -rf "$WORK"
}
trap cleanup EXIT

banner() { printf '\n=== %s %s\n' "$1" "$(printf '=%.0s' $(seq $((70 - ${#1}))))"; }
frame()  { $TMUX capture-pane -p -t demo; }

"$CMUXD" >"$WORK/cmuxd.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 50); do [ -S "$XDG_RUNTIME_DIR/cmux/cmuxd.sock" ] && break; sleep 0.1; done

banner "1. four different programs, one daemon"
"$CMUX" ctl spawn /tmp   --dangerous --label agent
"$CMUX" ctl spawn "$REPO" --label shell -- bash --norc --noprofile
"$CMUX" ctl spawn /tmp    --label repl  -- python3 -q
"$CMUX" ctl spawn /tmp    --label top   -- top -d 2
sleep 1.2

banner "2. what the daemon sees"
"$CMUX" ctl list

banner "3. what the TUI sees (cmux --connect)"
$TMUX new-session -d -s demo -x 158 -y 40 "$CMUX --connect"
sleep 2.5
frame

banner "4. drive the bash session: Ctrl+A 2, then type"
$TMUX send-keys -t demo C-a 2; sleep 0.6
$TMUX send-keys -t demo 'echo "cmux is driving a plain bash session"; ls crates' Enter
sleep 1.2
frame

banner "5. drive the python REPL: Ctrl+A 3, then evaluate"
$TMUX send-keys -t demo C-a 3; sleep 0.6
$TMUX send-keys -t demo 'import sys; print("python", sys.version_info[:3], "inside cmux")' Enter
sleep 1.2
frame

banner "6. a full-screen TUI as a session: Ctrl+A 4"
$TMUX send-keys -t demo C-a 4; sleep 2.0
frame

banner "7. quit the TUI — do the sessions survive?"
$TMUX send-keys -t demo C-a q
sleep 1.5
"$CMUX" ctl list
echo "(still hosted by the daemon, with no client attached)"
