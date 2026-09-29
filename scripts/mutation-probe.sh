#!/usr/bin/env bash
# Mutation probes for spec 001 (AC-001-03 connection failure / connection loss paths).
#
# Each named mutant is applied to a temporary, isolated copy of this worktree (the real
# tree is never edited, no git stash), the single test that must catch it runs inside the
# copy, and the copy is removed. A probe PASSES when that test FAILS for the expected
# reason (mutant killed) and FAILS when the test still passes (mutant survived) or fails
# for another reason (e.g. the copy did not build).
#
#   scripts/mutation-probe.sh <mutant>|all [--keep]
#
# Mutants (file → change → test expected to fail):
#   connect-exit        src-tauri/src/terminal.rs   connect error → std::process::exit(2)
#                       → e2e_linux_terminal_flow (step 9: GUI opened without a server)
#   disconnect-exit     src-tauri/src/terminal.rs   GatewayEvent::Disconnected → exit(2)
#                       → e2e_linux_terminal_flow (step 8: connection lost after attach)
#   auto-bootstrap      src-tauri/src/terminal.rs   connect error → start the engine itself
#                       → e2e_linux_terminal_flow (step 9: no engine/socket may appear)
#   eof-silent          crates/herdr-client/src/local.rs   EOF → reader ends without Disconnected
#                       → contracts::server_closing_the_socket_after_attach_yields_recoverable_disconnect
#   loss-not-retryable  crates/herdr-client/src/contracts.rs   connection_lost → retryable=false
#                       → contracts::server_closing_the_socket_after_attach_yields_recoverable_disconnect
#
# Requires the same host tools as `just check-spec 001` (cargo nextest, herdr, node) and, for
# the e2e mutants, a disposable session created by scripts/e2e-session.sh inside the copy.
# Logs: ${PROBE_LOG_DIR:-evidencias/001/probes}/<mutant>.log. Never runs the full gate.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
log_dir="${PROBE_LOG_DIR:-$root/evidencias/001/probes}"
keep=0
selection="${1:-}"
[ -n "$selection" ] || { sed -n '2,25p' "$0"; exit 2; }
[ "${2:-}" = "--keep" ] && keep=1
mkdir -p "$log_dir"

E2E_TEST='e2e_linux_terminal_flow'
CONTRACT_TEST='server_closing_the_socket_after_attach_yields_recoverable_disconnect'

# name -> kind|file|expected failure pattern (regex, grep -E)
mutant_kind()    { case "$1" in connect-exit|disconnect-exit|auto-bootstrap) echo e2e ;; eof-silent|loss-not-retryable) echo contract ;; *) return 1 ;; esac; }
mutant_file()    { case "$1" in connect-exit|disconnect-exit|auto-bootstrap) echo src-tauri/src/terminal.rs ;; eof-silent) echo crates/herdr-client/src/local.rs ;; loss-not-retryable) echo crates/herdr-client/src/contracts.rs ;; esac; }
mutant_pattern() {
  case "$1" in
    connect-exit)       echo 'window exited \(exit status: 2\) while waiting for window reporting the absent server' ;;
    disconnect-exit)    echo 'window exited \(exit status: 2\) while waiting for window reporting the lost connection' ;;
    auto-bootstrap)     echo 'GUI started an engine without a server|session socket present without a server|something accepts connections for the session without a server' ;;
    eof-silent)         echo 'expected Disconnected after the server closed, got Err\(Disconnected\)' ;;
    loss-not-retryable) echo 'loss after attach must be recoverable' ;;
  esac
}

# Exact single-anchor replacement; fails if the anchor is not found exactly once.
mutate() {
  node -e '
    const fs = require("node:fs");
    const [file, from, to] = process.argv.slice(1);
    const src = fs.readFileSync(file, "utf8");
    const n = src.split(from).length - 1;
    if (n !== 1) { console.error(`mutation anchor found ${n} times in ${file}`); process.exit(3); }
    fs.writeFileSync(file, src.replace(from, to));
  ' "$@"
}

apply_mutant() {
  local name="$1" copy="$2"
  case "$name" in
    connect-exit)
      mutate "$copy/src-tauri/src/terminal.rs" \
'        Err(error) => {
            // Not fatal: the window stays open in `disconnected` with a retryable error and
            // never starts an engine by itself (that is the explicit `session_start`).
            inner.last_error = Some(error.clone());' \
'        Err(error) => {
            // MUTANT connect-exit: a connection failure treated as a fatal bootstrap failure.
            std::process::exit(2);
            inner.last_error = Some(error.clone());' ;;
    disconnect-exit)
      mutate "$copy/src-tauri/src/terminal.rs" \
'            GatewayEvent::Disconnected(error) => {
                // Loss after attach is recoverable: drop the gateway, keep the window and the
                // retryable error; nothing is restarted here.
                inner.store.mark_stale(StaleReason::Disconnected);' \
'            GatewayEvent::Disconnected(error) => {
                // MUTANT disconnect-exit: a lost connection kills the executable.
                std::process::exit(2);
                inner.store.mark_stale(StaleReason::Disconnected);' ;;
    auto-bootstrap)
      mutate "$copy/src-tauri/src/terminal.rs" \
'        Err(error) => {
            // Not fatal: the window stays open in `disconnected` with a retryable error and
            // never starts an engine by itself (that is the explicit `session_start`).
            inner.last_error = Some(error.clone());' \
'        Err(error) => {
            // MUTANT auto-bootstrap: the GUI starts the engine by itself when absent.
            let _ = start_session_detached(
                &inner.config.herdr_bin,
                inner.config.session.as_ref().expect("checked above"),
                None,
            );
            inner.last_error = Some(error.clone());' ;;
    eof-silent)
      mutate "$copy/crates/herdr-client/src/local.rs" \
'            ReadStep::Closed => {
                if !stop.load(Ordering::Acquire) {
                    let error = framing_error(
                        FramingError::UnexpectedEof,
                        "conexão com o servidor encerrada",
                    );
                    let _ = tx.send((0, GatewayEvent::Disconnected(error)));
                }
                return;
            }' \
'            ReadStep::Closed => {
                // MUTANT eof-silent: the reader ends without reporting the loss.
                return;
            }' ;;
    loss-not-retryable)
      mutate "$copy/crates/herdr-client/src/contracts.rs" \
'            K::ConnectionReset | K::BrokenPipe | K::ConnectionAborted | K::UnexpectedEof => {
                ("connection_lost", true)
            }' \
'            K::ConnectionReset | K::BrokenPipe | K::ConnectionAborted | K::UnexpectedEof => {
                ("connection_lost", false) // MUTANT loss-not-retryable
            }' ;;
    *) echo "unknown mutant: $name" >&2; return 2 ;;
  esac
}

session=""
copy=""
current_log=""

session_listed() { herdr session list 2>/dev/null | awk -v s="$1" '$1==s {found=1} END {exit !found}'; }

# Only the resources this probe created: the disposable session it started, the exact
# absent-session name the E2E noted in this probe's log (the E2E's own guard normally
# already removed it; never a prefix sweep), and windows launched from this copy's path,
# by exact PID after confirming their cmdline (no pkill/pgrep patterns).
release_own_resources() {
  local log="$1"
  if [ -n "$session" ]; then
    "$root/scripts/e2e-session.sh" stop "$session" >>"$log" 2>&1 || true
    session=""
  fi
  local name
  name="$(sed -n 's/^absent session name: \([^ ]*\).*/\1/p' "$log" | head -1)"
  if [ -z "$name" ]; then
    echo "[cleanup] no absent session name noted by the E2E in this probe" | tee -a "$log"
  else
    if session_listed "$name"; then
      echo "[cleanup] session $name still present; stopping and deleting exactly this name" | tee -a "$log"
      env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_SESSION herdr session stop "$name" >>"$log" 2>&1 || true
      env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_SESSION herdr session delete "$name" >>"$log" 2>&1 || true
    fi
    if session_listed "$name"; then
      echo "[cleanup] session $name STILL PRESENT after cleanup" | tee -a "$log"
    else
      echo "[cleanup] session $name absent after the probe" | tee -a "$log"
    fi
  fi
  if [ -n "$copy" ]; then
    # Windows launched from this copy: exact PIDs only. Every /proc entry is confirmed by its
    # cmdline argv[0] being this copy's unique binary path before a single kill -TERM <pid>.
    local exe="$copy/target/debug/herdr-desktop" pid argv0 found=0
    for pid in $(ls /proc | grep -E '^[0-9]+$'); do
      argv0="$(tr '\0' '\n' <"/proc/$pid/cmdline" 2>/dev/null | head -n 1 || true)"
      if [ "$argv0" = "$exe" ]; then
        found=1
        echo "[cleanup] window pid $pid still running (cmdline: $argv0); kill -TERM $pid" | tee -a "$log"
        kill -TERM "$pid" 2>/dev/null || true
      fi
    done
    if [ "$found" = 0 ]; then
      echo "[cleanup] no window from this copy left running" | tee -a "$log"
    fi
    if [ "$keep" = 0 ]; then
      rm -rf "$copy"
      echo "[cleanup] copy removed: $copy" | tee -a "$log"
    fi
    copy=""
  fi
}

cleanup() {
  if [ -n "$copy" ] || [ -n "$session" ]; then
    release_own_resources "${current_log:-/dev/null}"
  fi
}
trap cleanup EXIT

run_mutant() {
  local name="$1" kind file pattern log started
  kind="$(mutant_kind "$name")" || { echo "unknown mutant: $name" >&2; return 2; }
  file="$(mutant_file "$name")"
  pattern="$(mutant_pattern "$name")"
  log="$log_dir/$name.log"
  current_log="$log"
  started="$(date -Is)"
  : >"$log"
  {
    echo "# mutation probe: $name ($kind) — $started"
    echo "# real tree: $root (never modified); copy: temporary, removed at the end"
  } | tee -a "$log"

  copy="$(mktemp -d /var/tmp/herdr-desktop-mutant.XXXXXX)"
  echo "\$ rsync -a --exclude .git --exclude node_modules --exclude target --exclude evidencias $root/ $copy/" | tee -a "$log"
  rsync -a --exclude .git --exclude node_modules --exclude target --exclude evidencias "$root/" "$copy/"
  # Reuse compiled dependencies (reflink on btrfs, plain copy elsewhere); workspace crates rebuild.
  echo "\$ cp -a --reflink=auto $root/target $copy/target" | tee -a "$log"
  cp -a --reflink=auto "$root/target" "$copy/target"

  apply_mutant "$name" "$copy"
  {
    echo "\$ diff -u $file (real) $file (mutated copy)"
    diff -u "$root/$file" "$copy/$file" || true
  } | tee -a "$log"

  local status=0
  if [ "$kind" = contract ]; then
    echo "\$ (cd copy && cargo nextest run -p herdr-desktop --test contracts -E 'test($CONTRACT_TEST)')" | tee -a "$log"
    (cd "$copy" && cargo nextest run -p herdr-desktop --test contracts -E "test($CONTRACT_TEST)") >>"$log" 2>&1 || status=$?
  else
    echo "\$ (cd copy && cargo build -p herdr-desktop)" | tee -a "$log"
    if ! (cd "$copy" && cargo build -p herdr-desktop) >>"$log" 2>&1; then
      echo "copy did not build; MUTANT $name probe FAILED (see $log)" | tee -a "$log"
      release_own_resources "$log"
      return 2
    fi
    local out
    out="$("$copy/scripts/e2e-session.sh" start)"
    session="$(printf '%s\n' "$out" | sed -n 's/^SESSION=//p')"
    local pane
    pane="$(printf '%s\n' "$out" | sed -n 's/^PANE_ID=//p')"
    echo "disposable session $session pane $pane" | tee -a "$log"
    echo "\$ (cd copy && cargo nextest run -p herdr-desktop --test e2e_linux --run-ignored ignored-only --no-capture -E 'test($E2E_TEST)')" | tee -a "$log"
    (cd "$copy" && HERDR_DESKTOP_E2E_SESSION="$session" HERDR_DESKTOP_E2E_PANE="$pane" \
      HERDR_DESKTOP_E2E_APP_BIN="$copy/target/debug/herdr-desktop" HERDR_DESKTOP_E2E_REPORT="$copy/report" \
      HERDR_DESKTOP_HERDR_BIN=herdr \
      cargo nextest run -p herdr-desktop --test e2e_linux --run-ignored ignored-only --no-capture -E "test($E2E_TEST)") >>"$log" 2>&1 || status=$?
  fi
  echo "[test exit] $status" | tee -a "$log"
  release_own_resources "$log"
  grep -E "panicked at|FAIL \[|PASS \[|Summary" "$log" | tail -6

  local verdict
  if [ "$status" = 0 ]; then
    verdict="MUTANT $name SURVIVED (test passed; probe FAILED)"
  elif grep -Eq "$pattern" "$log"; then
    verdict="MUTANT $name KILLED (test failed for the expected reason: /$pattern/)"
  else
    verdict="MUTANT $name FAILED FOR ANOTHER REASON (expected /$pattern/; probe FAILED)"
  fi
  echo "$verdict" | tee -a "$log"
  current_log=""
  case "$verdict" in *KILLED*) return 0 ;; *) return 1 ;; esac
}

overall=0
if [ "$selection" = all ]; then
  for m in eof-silent loss-not-retryable connect-exit disconnect-exit auto-bootstrap; do
    run_mutant "$m" || overall=1
  done
else
  run_mutant "$selection" || overall=1
fi
exit $overall
