#!/usr/bin/env bash
# End-to-end smoke test for cmuxd against real processes.
#
# Asserts the things unit tests cannot: that the daemon execs the argv it is
# given, that claude argv is built correctly, that sessions outlive their
# client, and that listing is ordered.
#
# Fully isolated — its own XDG dirs and a fake `claude` on PATH, so it never
# touches a real daemon, a real session, or your config.
#
#   ./scripts/smoke.sh            # uses target/debug (run cargo build first)
#   PROFILE=release ./scripts/smoke.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="${PROFILE:-debug}"
CMUX="$REPO/target/$PROFILE/cmux"
CMUXD="$REPO/target/$PROFILE/cmuxd"

for bin in "$CMUX" "$CMUXD"; do
  [ -x "$bin" ] || { echo "missing $bin — run: cargo build --workspace"; exit 2; }
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/cmux-smoke.XXXXXX")"
export XDG_RUNTIME_DIR="$WORK/run"
export XDG_STATE_HOME="$WORK/state"
export XDG_CONFIG_HOME="$WORK/config"
export PATH="$WORK/bin:$PATH"
mkdir -p "$XDG_RUNTIME_DIR/cmux" "$WORK/bin"

# Stand-in for claude, so the suite never launches the real CLI. It records
# the argv the daemon exec'd, which is the thing under test.
cat > "$WORK/bin/claude" <<'FAKE'
#!/usr/bin/env bash
echo "CLAUDE-ARGV:$*"
sleep 120
FAKE
chmod +x "$WORK/bin/claude"

PASS=0
FAIL=0
DAEMON_PID=""

ok()   { PASS=$((PASS + 1)); printf '  ok    %s\n' "$1"; }
bad()  { FAIL=$((FAIL + 1)); printf '  FAIL  %s\n' "$1"; [ $# -gt 1 ] && printf '        %s\n' "$2"; }
check() { # check <name> <expected-substring> <actual>
  case "$3" in
    *"$2"*) ok "$1" ;;
    *)      bad "$1" "wanted '$2' in: $3" ;;
  esac
}

cleanup() {
  [ -n "$DAEMON_PID" ] && {
    "$CMUX" ctl shutdown >/dev/null 2>&1
    sleep 0.4
    kill "$DAEMON_PID" 2>/dev/null
    wait "$DAEMON_PID" 2>/dev/null
  }
  rm -rf "$WORK"
}
trap cleanup EXIT

echo "cmux smoke test  (profile=$PROFILE, work=$WORK)"

# Port 0 lets the kernel pick, so the suite never collides with a real daemon
# or another run of itself. The daemon prints the address it actually bound.
"$CMUXD" --http 127.0.0.1:0 >"$WORK/cmuxd.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 50); do
  [ -S "$XDG_RUNTIME_DIR/cmux/cmuxd.sock" ] && break
  sleep 0.1
done
if [ ! -S "$XDG_RUNTIME_DIR/cmux/cmuxd.sock" ]; then
  echo "daemon never bound its socket; log:"; cat "$WORK/cmuxd.log"; exit 1
fi
ok "daemon bound \$XDG_RUNTIME_DIR/cmux/cmuxd.sock"

echo
echo "spawning"
check "spawns an arbitrary command" "spawned [1]" \
  "$("$CMUX" ctl spawn /tmp --label shell -- bash --norc --noprofile 2>&1)"
check "spawns claude by default" "spawned [2]" \
  "$("$CMUX" ctl spawn /tmp --label agent --dangerous 2>&1)"
check "spawns a third session" "spawned [3]" \
  "$("$CMUX" ctl spawn /tmp --label sleeper -- sleep 120 2>&1)"
check "rejects --dangerous with a custom command" "claude flag" \
  "$("$CMUX" ctl spawn /tmp --dangerous -- bash 2>&1)"
check "reports an unknown program instead of spawning" "definitely-not-a-real-binary" \
  "$("$CMUX" ctl spawn /tmp -- definitely-not-a-real-binary 2>&1)"
sleep 1

echo
echo "listing"
LIST="$("$CMUX" ctl list 2>&1)"
check "list reports the real argv"        "bash --norc --noprofile" "$LIST"
check "list reports the claude argv"      "claude --dangerously-skip-permissions" "$LIST"
IDS="$(printf '%s\n' "$LIST" | sed -n 's/^\[\([0-9]*\)\].*/\1/p' | tr '\n' ' ')"
if [ "$IDS" = "1 2 3 " ]; then ok "list is ordered by session id"
else bad "list is ordered by session id" "got: $IDS"; fi

echo
echo "exec"
CHILDREN="$(pgrep -P "$DAEMON_PID" 2>/dev/null | while read -r p; do tr '\0' ' ' < "/proc/$p/cmdline"; echo; done)"
check "daemon exec'd bash"   "bash --norc --noprofile"                "$CHILDREN"
check "daemon exec'd claude with the right flag" "--dangerously-skip-permissions" "$CHILDREN"
check "daemon exec'd sleep"  "sleep 120"                              "$CHILDREN"

echo
echo "lifetime"
# Every ctl call above already connected and disconnected. If sessions died
# with their client, nothing would be left to list.
check "sessions outlive the client that made them" "sleeper" "$("$CMUX" ctl list 2>&1)"
"$CMUX" ctl kill 1 >/dev/null 2>&1
sleep 0.4
AFTER="$("$CMUX" ctl list 2>&1)"
case "$AFTER" in
  *"bash --norc"*) bad "kill removes the session" "still listed: $AFTER" ;;
  *)               ok "kill removes the session" ;;
esac
check "the other sessions survive the kill" "sleep 120" "$AFTER"

echo
echo "http api"
HTTP="$(sed -n 's|^cmuxd http api on \(http://[0-9.:]*\)$|\1|p' "$WORK/cmuxd.log" | head -1)"
if ! command -v curl >/dev/null; then
  echo "  skip  no curl on PATH"
elif [ -z "$HTTP" ]; then
  bad "http api came up" "no address in cmuxd.log"
else
  ok "http api bound $HTTP"
  code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }

  # cmuxd does no authentication on purpose: access control belongs to
  # whatever fronts the port. These assert that, so re-adding a check inside
  # the daemon fails here rather than silently changing the contract.
  [ "$(code "$HTTP/api/health")" = "200" ] \
    && ok "serves without credentials, by design" \
    || bad "serves without credentials" "got $(code "$HTTP/api/health")"
  [ "$(code "$HTTP/api/sessions")" = "200" ] \
    && ok "listing needs no credentials" || bad "listing needs no credentials"
  if [ -e "$XDG_RUNTIME_DIR/cmux/http-token" ]; then
    bad "no token file is written" "found one; the daemon should hold no secret"
  else
    ok "no token file is written"
  fi

  NEW="$(curl -s -H 'Content-Type: application/json' \
    -d '{"cmd":["bash","--norc","--noprofile"],"cwd":"/tmp","label":"http"}' \
    "$HTTP/api/sessions")"
  check "spawns a session over HTTP" '"label":"http"' "$NEW"
  NEW_ID="$(printf '%s' "$NEW" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')"

  curl -s -o /dev/null --data-binary $'echo SMOKE-OVER-HTTP-$((6*7))\n' \
    "$HTTP/api/sessions/$NEW_ID/input"
  sleep 1
  check "input reaches the pty and shows on the screen" "SMOKE-OVER-HTTP-42" \
    "$(curl -s "$HTTP/api/sessions/$NEW_ID/screen")"

  [ "$(code "$HTTP/api/sessions/9999/screen")" = "404" ] \
    && ok "unknown session is a 404" || bad "unknown session is a 404"
  [ "$(code "$HTTP/")" = "200" ] \
    && ok "browser page is served" || bad "browser page is served"
fi

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
