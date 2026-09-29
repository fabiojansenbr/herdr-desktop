#!/usr/bin/env bash
# Linux resource report for spec 001 (AC-001-02): RSS/PSS and CPU of the whole GUI process
# tree (Tauri host + WebKitGTK web/network processes) while idle on one terminal, against a
# disposable session. Engine and shell are reported separately; nothing here touches the
# default session.
#
# Method: release build; app launched with HERDR_DESKTOP_SESSION; after the surface is live
# and a coloured line was printed, sample every 1 s for $SAMPLE_SECONDS: PSS/RSS from
# /proc/<pid>/smaps_rollup for every process whose ancestor chain reaches the app pid,
# CPU from the utime+stime delta of the same set divided by wall time.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
SAMPLE_SECONDS="${SAMPLE_SECONDS:-30}"
out_dir="${1:-$root/evidencias/001}"
mkdir -p "$out_dir"

app="$root/target/release/herdr-desktop"
if [ ! -x "$app" ]; then
  echo "release binary missing; run: mise exec -- just build-release" >&2
  exit 1
fi

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

trace="$out_dir/measure-surface-trace.json"
rm -f "$trace"
env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_PANE_ID -u HERDR_SESSION \
  HERDR_DESKTOP_SESSION="$SESSION" HERDR_DESKTOP_SURFACE_TRACE="$trace" \
  HERDR_DESKTOP_LOCALE=pt \
  "$app" >/dev/null 2>"$out_dir/measure-app.stderr" &
APP_PID=$!

for _ in $(seq 1 100); do
  if [ -f "$trace" ] && grep -q '"state":"live"' "$trace"; then break; fi
  sleep 0.2
done
grep -q '"state":"live"' "$trace" || { echo "app did not reach live state" >&2; exit 1; }

hs pane run "$PANE_ID" "printf '\\e[1;31mred\\e[0m \\e[44mblue\\e[0m \\e[32má 界 🦀 e\\xcc\\x81\\e[0m GUI_OK\\n'" >/dev/null
sleep 2

# --- process tree helpers -------------------------------------------------------------
tree_pids() {
  local roots="$1"
  local all=""
  local next="$roots"
  while [ -n "$next" ]; do
    all="$all $next"
    local children=""
    for p in $next; do
      children="$children $(cat /proc/"$p"/task/*/children 2>/dev/null | tr '\n' ' ')"
    done
    next="$(echo "$children" | tr ' ' '\n' | grep -E '^[0-9]+$' | sort -u | tr '\n' ' ')"
  done
  echo "$all" | tr ' ' '\n' | grep -E '^[0-9]+$' | sort -u
}
cpu_ticks() {
  local pidlist="$1"
  local total=0
  local p f rest utime stime
  for p in $pidlist; do
    f="$(cat /proc/"$p"/stat 2>/dev/null || true)"
    [ -z "$f" ] && continue
    # fields after the command name: utime is the 12th, stime the 13th (1-based)
    rest="${f##*) }"
    utime="$(echo "$rest" | awk '{print $12}')"
    stime="$(echo "$rest" | awk '{print $13}')"
    total=$((total + utime + stime))
  done
  echo "$total"
}
mem_kb() {
  local pidlist="$1"
  local pss=0 rss=0 p
  for p in $pidlist; do
    local r
    r="$(grep -E '^(Pss|Rss):' /proc/"$p"/smaps_rollup 2>/dev/null | awk '{print $1 $2}' | tr '\n' ' ' || true)"
    [ -z "$r" ] && continue
    pss=$((pss + $(echo "$r" | grep -o 'Pss:[0-9]*' | cut -d: -f2)))
    rss=$((rss + $(echo "$r" | grep -o 'Rss:[0-9]*' | cut -d: -f2)))
  done
  echo "$pss $rss"
}

# Engine pid for the disposable session (reported separately, never killed here).
engine_pid="$(pgrep -f "herdr --session $SESSION server" | head -1 || true)"
shell_pid="$(hs pane process-info --pane "$PANE_ID" | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{try{process.stdout.write(String(JSON.parse(s).result.process_info.shell_pid||""))}catch{}})')"

clk_tck="$(getconf CLK_TCK)"
pids="$(tree_pids "$APP_PID")"
start_ticks="$(cpu_ticks "$pids")"
start_wall="$(date +%s.%N)"
samples="$out_dir/recursos-linux-samples.tsv"
printf 'second\tprocesses\tpss_kb\trss_kb\n' > "$samples"
max_pss=0; max_rss=0; sum_pss=0
for i in $(seq 1 "$SAMPLE_SECONDS"); do
  sleep 1
  pids="$(tree_pids "$APP_PID")"
  read -r pss rss <<<"$(mem_kb "$pids")"
  n="$(echo "$pids" | wc -l)"
  printf '%s\t%s\t%s\t%s\n' "$i" "$n" "$pss" "$rss" >> "$samples"
  [ "$pss" -gt "$max_pss" ] && max_pss=$pss
  [ "$rss" -gt "$max_rss" ] && max_rss=$rss
  sum_pss=$((sum_pss + pss))
done
end_ticks="$(cpu_ticks "$pids")"
end_wall="$(date +%s.%N)"
cpu_pct="$(awk -v a="$start_ticks" -v b="$end_ticks" -v t="$clk_tck" -v s="$start_wall" -v e="$end_wall" 'BEGIN { printf "%.2f", ((b-a)/t)/(e-s)*100 }')"
avg_pss=$((sum_pss / SAMPLE_SECONDS))

engine_mem="n/a"; shell_mem="n/a"
[ -n "$engine_pid" ] && engine_mem="$(mem_kb "$engine_pid")"
[ -n "$shell_pid" ] && shell_mem="$(mem_kb "$shell_pid")"
procs="$(for p in $pids; do printf '%s %s\n' "$p" "$(tr '\0' ' ' < /proc/"$p"/cmdline 2>/dev/null | cut -c1-80)"; done)"

host="$(uname -srm); $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs); WebKitGTK $(pkg-config --modversion webkit2gtk-4.1); Hyprland/Wayland ($XDG_SESSION_TYPE)"
cat > "$out_dir/recursos-linux.json" <<JSON
{
  "date": "$(date -Iseconds)",
  "host": "$host",
  "build": "release (lto=thin, codegen-units=1)",
  "scenario": "idle, 1 terminal (80x24 → window geometry), editor closed, one coloured line printed",
  "sample_seconds": $SAMPLE_SECONDS,
  "gui_tree": { "processes": $(echo "$pids" | wc -l), "pss_kb_avg": $avg_pss, "pss_kb_max": $max_pss, "rss_kb_max": $max_rss, "cpu_percent_of_one_core": $cpu_pct },
  "engine": { "pid": "${engine_pid:-}", "pss_rss_kb": "$engine_mem" },
  "shell": { "pid": "${shell_pid:-}", "pss_rss_kb": "$shell_mem" },
  "method": "PSS/RSS summed from /proc/<pid>/smaps_rollup over every descendant of the app pid (Tauri host + WebKitGTK WebProcess/NetworkProcess); CPU = Σ(utime+stime) delta / wall time over the window; engine and shell measured separately and not attributed to the GUI"
}
JSON
{
  echo "# Recursos — Linux (spec 001)"
  echo
  echo "Data: $(date -Iseconds). Host: $host."
  echo
  echo "Cenário: idle, 1 terminal, editor fechado, uma linha colorida impressa; build release; amostragem de ${SAMPLE_SECONDS}s a 1 Hz."
  echo
  echo "| Métrica | Valor |"
  echo "|---|---|"
  echo "| Processos na árvore da GUI | $(echo "$pids" | wc -l) |"
  echo "| PSS médio da árvore | $((avg_pss / 1024)) MiB |"
  echo "| PSS máximo da árvore | $((max_pss / 1024)) MiB |"
  echo "| RSS máximo (soma, conta páginas compartilhadas mais de uma vez) | $((max_rss / 1024)) MiB |"
  echo "| CPU média da árvore (fração de um núcleo) | ${cpu_pct}% |"
  echo "| Engine (pid ${engine_pid:-?}) PSS/RSS kB | $engine_mem |"
  echo "| Shell do pane (pid ${shell_pid:-?}) PSS/RSS kB | $shell_mem |"
  echo
  echo "Processos medidos:"
  echo
  echo '```'
  echo "$procs"
  echo '```'
  echo
  echo "Método: PSS/RSS somados de /proc/<pid>/smaps_rollup para todos os descendentes do pid do app (host Tauri + WebProcess/NetworkProcess do WebKitGTK). CPU = Σ(utime+stime) da árvore / tempo de parede. Engine e shell medidos à parte e não atribuídos à GUI. Amostras em recursos-linux-samples.tsv; latência input→frame comprometido (harness, sem pintura no WebView) em latency-gateway.json (E2E)."
} > "$out_dir/recursos-linux.md"
cat "$out_dir/recursos-linux.md"
