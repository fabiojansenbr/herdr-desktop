#!/usr/bin/env bash
# Disposable Herdr session for the 001 Linux E2E (follows ../herdr/.agents/skills/herdr-throwaway-repro).
#
#   scripts/e2e-session.sh start   -> prints KEY=VALUE lines (SESSION, PANE_ID, WORKSPACE_ID, DIR, CONFIG)
#   scripts/e2e-session.sh stop <session>
#
# Never touches the default session. Only stops/deletes the session it created.
set -euo pipefail

herdr_bin="${HERDR_DESKTOP_HERDR_BIN:-herdr}"

# Every command addressing the disposable session clears inherited overrides and selects it explicitly.
hs() {
  local session="$1"; shift
  env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_WORKSPACE_ID -u HERDR_TAB_ID -u HERDR_PANE_ID \
    HERDR_SESSION="$session" HERDR_CONFIG_PATH="$CONFIG_PATH" "$herdr_bin" "$@"
}

# `pane list` answers JSON; pick the focused pane (or the first one).
focused_pane() {
  hs "$1" pane list 2>/dev/null | node -e '
    let s = "";
    process.stdin.on("data", (d) => (s += d)).on("end", () => {
      try {
        const panes = JSON.parse(s).result.panes;
        const pane = panes.find((p) => p.focused) || panes[0];
        process.stdout.write(pane ? pane.pane_id : "");
      } catch { process.stdout.write(""); }
    });'
}

start() {
  local stamp session dir config
  stamp="$(date +%s)-$$"
  session="hd001-${stamp}"
  dir="/var/tmp/herdr-desktop-e2e/${session}"
  config="${dir}/config.toml"
  mkdir -p "${dir}/work"
  cat > "$config" <<'TOML'
# Test-only configuration for the herdr-desktop 001 disposable session.
[experimental]
allow_nested = true
TOML
  CONFIG_PATH="$config"
  hs "$session" config check >"${dir}/config-check.log" 2>&1 || { cat "${dir}/config-check.log" >&2; exit 1; }

  # Detached headless server for the named session (own session id, stdio to /dev/null).
  env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_SESSION -u HERDR_WORKSPACE_ID -u HERDR_TAB_ID -u HERDR_PANE_ID \
    HERDR_CONFIG_PATH="$config" HERDR_STARTUP_CWD="${dir}/work" \
    setsid "$herdr_bin" --session "$session" server </dev/null >"${dir}/server.log" 2>&1 &

  local ready=0
  for _ in $(seq 1 100); do
    if hs "$session" pane list >/dev/null 2>&1; then ready=1; break; fi
    sleep 0.1
  done
  if [ "$ready" != 1 ]; then
    echo "disposable session ${session} did not become ready" >&2
    cat "${dir}/server.log" >&2 || true
    exit 1
  fi

  local pane_id workspace_id
  pane_id="$(focused_pane "$session")"
  if [ -z "$pane_id" ]; then
    hs "$session" workspace create --cwd "${dir}/work" --focus >"${dir}/workspace-create.log" 2>&1
    pane_id="$(focused_pane "$session")"
  fi
  if [ -z "$pane_id" ]; then
    echo "disposable session ${session} has no pane" >&2
    exit 1
  fi
  workspace_id="${pane_id%%:*}"

  # Give the shell prompt a moment before the E2E starts typing.
  hs "$session" pane wait-output --timeout 10000 --regex '[$#>%] ?$' "$pane_id" >/dev/null 2>&1 || true

  printf 'SESSION=%s\nPANE_ID=%s\nWORKSPACE_ID=%s\nDIR=%s\nCONFIG=%s\nHERDR_BIN=%s\n' \
    "$session" "$pane_id" "$workspace_id" "$dir" "$config" "$herdr_bin"
}

stop() {
  local session="$1"
  case "$session" in
    hd001-*) ;;
    *) echo "refusing to stop session '${session}': not created by this script" >&2; exit 1 ;;
  esac
  CONFIG_PATH="/var/tmp/herdr-desktop-e2e/${session}/config.toml"
  env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_WORKSPACE_ID -u HERDR_TAB_ID -u HERDR_PANE_ID \
    HERDR_CONFIG_PATH="$CONFIG_PATH" "$herdr_bin" session stop "$session" || true
  for _ in $(seq 1 50); do
    if ! "$herdr_bin" session list 2>/dev/null | awk -v s="$session" '$1==s && $2=="running" {found=1} END {exit !found}'; then break; fi
    sleep 0.1
  done
  env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH HERDR_CONFIG_PATH="$CONFIG_PATH" "$herdr_bin" session delete "$session" || true
  "$herdr_bin" session list | grep -v "^${session} " >/dev/null || true
}

case "${1:-}" in
  start) start ;;
  stop) stop "${2:?session name required}" ;;
  *) echo "usage: $0 start | stop <session>" >&2; exit 2 ;;
esac
