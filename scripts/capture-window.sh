#!/usr/bin/env bash
# Visual evidence for spec 001: opens the real window against a disposable session, prints
# a coloured line with á / 界 / 🦀 / combining mark through the engine (pane run), and
# captures the window with grim (Hyprland/Wayland). Output: <out_dir>/janela-linux.png.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out_dir="${1:-$root/evidencias/001}"
app="${2:-$root/target/debug/herdr-desktop}"
mkdir -p "$out_dir"
command -v grim >/dev/null || { echo "grim not installed" >&2; exit 1; }
command -v hyprctl >/dev/null || { echo "hyprctl not available" >&2; exit 1; }

eval "$("$root/scripts/e2e-session.sh" start | grep -E '^[A-Z_]+=')"
cleanup() {
  [ -n "${APP_PID:-}" ] && kill -TERM "$APP_PID" 2>/dev/null || true
  "$root/scripts/e2e-session.sh" stop "$SESSION" >/dev/null 2>&1 || true
}
trap cleanup EXIT

hs() {
  env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_WORKSPACE_ID -u HERDR_TAB_ID -u HERDR_PANE_ID \
    HERDR_SESSION="$SESSION" herdr "$@"
}

trace="$out_dir/capture-surface-trace.json"
rm -f "$trace"
env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_PANE_ID -u HERDR_SESSION \
  HERDR_DESKTOP_SESSION="$SESSION" HERDR_DESKTOP_SURFACE_TRACE="$trace" \
  HERDR_DESKTOP_LOCALE=pt \
  "$app" >/dev/null 2>"$out_dir/capture-app.stderr" &
APP_PID=$!
for _ in $(seq 1 100); do
  if [ -f "$trace" ] && grep -q '"state":"live"' "$trace"; then break; fi
  sleep 0.2
done
grep -q '"state":"live"' "$trace" || { echo "app did not reach live state" >&2; exit 1; }

hs pane run "$PANE_ID" "printf '\\e[1;31mred bold\\e[0m \\e[44mblue bg\\e[0m \\e[32má 界 🦀 e\\xcc\\x81\\e[0m \\e[4munderline\\e[0m GUI_OK\\n'" >/dev/null
for _ in $(seq 1 50); do
  grep -q 'GUI_OK' "$trace" && break
  sleep 0.1
done
sleep 1.5

geom="$(hyprctl clients -j | node -e '
  let s = ""; process.stdin.on("data", d => s += d).on("end", () => {
    const c = JSON.parse(s).find(w => w.title === "Herdr Desktop" || w.class === "herdr-desktop" || w.initialClass === "herdr-desktop");
    if (!c) { process.exit(1); }
    process.stdout.write(`${c.at[0]},${c.at[1]} ${c.size[0]}x${c.size[1]}`);
  });')"
grim -g "$geom" "$out_dir/janela-linux.png"
echo "captured $out_dir/janela-linux.png ($geom)"
node -e 'const t=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"));console.log(JSON.stringify({state:t.state,revision:t.revision,boot_id:t.boot_id,generation:t.generation,shell_pid:t.shell_pid,size:`${t.width}x${t.height}`,rows_with_marker:t.rows.filter(r=>r.includes("GUI_OK"))}))' "$trace" | tee "$out_dir/janela-linux.json"
