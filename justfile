# Herdr Desktop — validation recipes. Run from the worktree with `mise exec -- just <recipe>`.

set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

# Preflight: every tool the gates call must answer.
preflight:
    cargo --version
    just --version
    bun --version
    node --version
    pkg-config --modversion webkit2gtk-4.1
    cargo nextest --version | head -1
    herdr --version

# Full gate for one spec (scripts/gates/specs/NNN.json): frontend, lint, selected tests (>0), build, native Linux E2E; each once.
check-spec spec:
    node scripts/check-spec.mjs {{spec}}

# Cheap dry-run of a spec gate: tools, plan, selections listed (>0, disjoint from runtime), E2E test present.
preflight-spec spec:
    node scripts/check-spec.mjs {{spec}} --preflight

# Shared runtime contracts (scripts/gates/suites/runtime.json); evidence under the spec in doing.
check-runtime:
    node scripts/check-spec.mjs --suite runtime

# Contract selection of one feature, for diagnosis (fails while the feature has no implemented plan).
check-connections:
    node scripts/check-spec.mjs --feature connections

# Contract selection of the agents feature, for diagnosis (fails while it has no implemented plan).
check-agents:
    node scripts/check-spec.mjs --feature agents

# Every implemented plan plus the runtime suite: all stages once, every native E2E.
check:
    node scripts/check-spec.mjs --all

# Install locked frontend dependencies and build dist/, which the Tauri crates embed at compile time.
frontend:
    bun install --frozen-lockfile
    bunx vite build

# Rust protocol/contract suite (frozen v1 digests, fixtures, FrameStore, bootstrap). No E2E.
check-protocol: frontend
    cargo nextest run --workspace --all-targets --no-fail-fast \
        -E 'package(herdr-protocol) | package(herdr-client) | binary_id(herdr-desktop::contracts) | binary_id(herdr-desktop::registry)'

# Frontend unit tests only (renderer, grid, input).
test-web:
    bunx vitest run

# Lint everything the gate lints.
lint: frontend
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    bunx tsc --noEmit -p tsconfig.json
    bunx svelte-check --tsconfig ./tsconfig.json --fail-on-warnings

# Debug build of the app (frontend + Tauri host).
build:
    bunx vite build
    cargo build -p herdr-desktop

# Release build used only for resource measurement.
build-release:
    bunx vite build
    cargo build -p herdr-desktop --release

# Start a disposable named session and print its ids (see scripts/e2e-session.sh).
e2e-session-start:
    scripts/e2e-session.sh start

e2e-session-stop session:
    scripts/e2e-session.sh stop {{session}}

# Linux resource report (RSS/PSS/CPU of the whole GUI tree) against a disposable session.
measure-resources:
    scripts/measure-resources.sh

# Spec 007 live resource bench: release build of the fidelity_bench binary (composed window on a
# private display + disposable hd007 engine + bench/collector.py), 1 GUI / 1 PTY, visible and
# hidden windows. Smoke (<= 3 s) is insufficient for acceptance; the checkpoint also allows
# one idle 1-pane/1-client acceptance measurement. `out` must be a new absolute directory (e.g. under
# evidencias/007/). Never passes HERDR_DESKTOP_SURFACE_TRACE to the window.
bench-desktop out herdr_bin resources +args:
    bunx vite build
    cargo test --release -p herdr-desktop --test fidelity_bench --no-run
    env -u HERDR_DESKTOP_SURFACE_TRACE HERDR_DESKTOP_BENCH_ARGS="{{args}} --out {{out}}" HERDR_DESKTOP_HERDR_BIN="{{herdr_bin}}" HERDR_DESKTOP_NATIVE_RESOURCES="{{resources}}" cargo test --release -p herdr-desktop --test fidelity_bench bench_desktop_live -- --exact --ignored --nocapture --test-threads=1

# Spec 007 render scale (AC-007-03): 1/15 panes × 1/2 clients with visible+hidden output in the
# same run, plus idle 1 and 15 panes, one `bench-desktop` per topology; then the scale report
# (bench/render_scale_report.py: GUI/engine/children PSS, CPU per cell and client, Δ15p) printed
# and written to OUT/REPORT.md. mode=acceptance (--warmup 5 --duration 60) | smoke (--duration 3,
# insufficient for acceptance). Needs HERDR_DESKTOP_HERDR_BIN and HERDR_DESKTOP_NATIVE_RESOURCES;
# `out` must be new or empty (default evidencias/007/bench-render-scale/<mode>-<UTC stamp>).
# Exit != 0 if any topology or the report is incomplete. Run it under the native-measurement flock.
# Spec 007 render-scale matrix (1/15 panes × 1/2 clients × visible/hidden + idle) and scale report.
bench-render-scale mode="acceptance" out="":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{mode}}" in
      acceptance) timing="--mode acceptance --warmup 5 --duration 60" ;;
      smoke) timing="--mode smoke --duration 3" ;;
      *) echo "mode must be acceptance|smoke, got '{{mode}}'" >&2; exit 2 ;;
    esac
    : "${HERDR_DESKTOP_HERDR_BIN:?HERDR_DESKTOP_HERDR_BIN (absolute engine binary) is required}"
    : "${HERDR_DESKTOP_NATIVE_RESOURCES:?HERDR_DESKTOP_NATIVE_RESOURCES is required}"
    out="{{out}}"
    [ -n "$out" ] || out="{{justfile_directory()}}/evidencias/007/bench-render-scale/{{mode}}-$(date -u +%Y%m%dT%H%M%SZ)"
    case "$out" in /*) ;; *) out="{{justfile_directory()}}/$out" ;; esac
    if [ -e "$out" ] && [ -n "$(ls -A "$out")" ]; then echo "out $out must be new or empty" >&2; exit 2; fi
    mkdir -p "$out"
    leftovers() { { ls -d /tmp/hd7B-* /var/tmp/herdr-desktop-e2e/hd007-bench-* 2>/dev/null || true; }; }
    cells=(
      "p1c1-idle --panes 1 --clients 1 --load idle"
      "p1c1-output --panes 1 --clients 1 --load output"
      "p1c2-output --panes 1 --clients 2 --load output"
      "p15c1-output --panes 15 --clients 1 --load output"
      "p15c2-output --panes 15 --clients 2 --load output"
      "p15c1-idle --panes 15 --clients 1 --load idle"
    )
    failed=0
    for cell in "${cells[@]}"; do
      name="${cell%% *}"; topology="${cell#* }"; meta="$out/$name.runner"
      mkdir -p "$meta"
      leftovers >"$meta/leftover-dirs-before.txt"
      echo "$timing $topology --out $out/$name" >"$meta/bench-args.txt"
      echo "== $name: $timing $topology ($(date -Is))"
      set +e
      just --justfile "{{justfile()}}" bench-desktop "$out/$name" "$HERDR_DESKTOP_HERDR_BIN" "$HERDR_DESKTOP_NATIVE_RESOURCES" $timing $topology >"$meta/stdout.log" 2>"$meta/stderr.log"
      ec=$?
      set -e
      echo "$ec" >"$meta/exit.txt"
      leftovers >"$meta/leftover-dirs-after.txt"
      echo "   exit=$ec leftovers_after=$(wc -l <"$meta/leftover-dirs-after.txt") $(grep -E '^test result' "$meta/stdout.log" | tail -1)"
      [ "$ec" = 0 ] || { failed=1; tail -5 "$meta/stderr.log" >&2; }
    done
    set +e
    python3 "{{justfile_directory()}}/bench/render_scale_report.py" "$out"
    report=$?
    set -e
    echo "bench-render-scale mode={{mode}} out=$out topologies_failed=$failed report_exit=$report"
    [ "$failed" = 0 ] && [ "$report" = 0 ]

# Spec 007 live latency bench (release build, private display, wtype + grim).
bench-latency out herdr_bin resources +args:
    bunx vite build
    cargo test --release -p herdr-desktop --test fidelity_bench --no-run
    env -u HERDR_DESKTOP_SURFACE_TRACE HERDR_DESKTOP_BENCH_ARGS="--latency {{args}} --out {{out}}" HERDR_DESKTOP_HERDR_BIN="{{herdr_bin}}" HERDR_DESKTOP_NATIVE_RESOURCES="{{resources}}" cargo test --release -p herdr-desktop --test fidelity_bench bench_desktop_live -- --exact --ignored --nocapture --test-threads=1


# Run the app against a named session (development).
dev session:
    HERDR_DESKTOP_SESSION={{session}} bunx tauri dev
