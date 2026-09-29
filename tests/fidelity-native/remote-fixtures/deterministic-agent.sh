#!/usr/bin/env bash
# hd007 deterministic fake agent (approved hd004 pattern): no network, no credentials, no model.
# Installed as `pi` in a fixture namespace before its engine starts; logs into that namespace.
log="${HD007_AGENT_LOG:?HD007_AGENT_LOG must point into the fixture namespace}"
case "${HERDR_SESSION:-}" in hd007-remote-*) ;; *) echo "refusing: not a disposable hd007-remote session" >&2; exit 3 ;; esac
[ -n "${HERDR_SOCKET_PATH:-}" ] && [ -n "${HERDR_PANE_ID:-}" ] || { echo "refusing: no pane socket" >&2; exit 3; }
now() { date +%s%3N; }
report() {
  "${HERDR_BIN_PATH:-herdr}" pane report-agent "$HERDR_PANE_ID" --source hd007-fake --agent pi --state "$1" >/dev/null 2>&1
  echo "state $1 rc=$? $(now)" >>"$log"
}
echo "start session=$HERDR_SESSION pane=$HERDR_PANE_ID pid=$$ argv=$* $(now)" >>"$log"
printf 'fake-agent hd007 pronto\n> '
report idle
while IFS= read -r line; do
  echo "prompt $line $(now)" >>"$log"
  report working
  printf 'trabalhando: %s\n> ' "$line"
  report idle
done
echo "exit $(now)" >>"$log"
