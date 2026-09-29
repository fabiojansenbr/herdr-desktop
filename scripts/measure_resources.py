#!/usr/bin/env python3
"""measure.py <label> <root-pid> [seconds] [out.tsv]

Samples the process tree under <root-pid>: CPU (tick delta), wakeups (voluntary
context switches per second) and memory (PSS split) per process, plus a TOTAL row.
Read-only: it only reads /proc.

`voluntary_ctxt_switches` lives per thread and disappears with the thread, so the window
delta is taken per tid over the threads still alive at the end. When the thread set shrank
(or a tid was recycled) the counters of the threads that went away cannot be recovered: the
rate is then a deterministic lower bound, flagged in the `wakeups_lb` column, never a
negative number.
"""
import contextlib
import os
import sys
import time

HZ = os.sysconf("SC_CLK_TCK")
COLS = ["label", "pid", "name", "cpu_pct", "wakeups_s", "wakeups_lb", "pss_mib",
        "pss_anon_mib", "pss_file_mib", "private_mib", "threads"]
PROC = "/proc"


@contextlib.contextmanager
def proc_root(root):
    """Reads a synthetic /proc instead of the real one (tests only)."""
    global PROC
    previous, PROC = PROC, root
    try:
        yield
    finally:
        PROC = previous


def children(pid):
    out = []
    try:
        for tid in os.listdir(f"{PROC}/{pid}/task"):
            with open(f"{PROC}/{pid}/task/{tid}/children") as f:
                out += [int(c) for c in f.read().split()]
    except OSError:
        pass
    return out


def tree(pid):
    pids = [pid]
    for c in children(pid):
        pids += tree(c)
    return pids


def ticks(pid):
    with open(f"{PROC}/{pid}/stat") as f:
        rest = f.read().rsplit(") ", 1)[1].split()
    return int(rest[11]) + int(rest[12])


def wakes_by_tid(pid):
    """voluntary_ctxt_switches of every thread of <pid>, keyed by tid."""
    counters = {}
    for tid in os.listdir(f"{PROC}/{pid}/task"):
        try:
            with open(f"{PROC}/{pid}/task/{tid}/status") as f:
                for line in f:
                    if line.startswith("voluntary_ctxt_switches"):
                        counters[int(tid)] = int(line.split()[1])
                        break
        except OSError:
            pass
    return counters


def wakeups_delta(before, after):
    """(wakeups in the window, whether the value is only a lower bound).

    A thread alive at both ends contributes its own monotonic delta. A tid seen only at the
    end started its counter at zero inside the window, so its whole value belongs to the
    window and is exact. A tid whose counter went backwards was recycled, and a tid that is
    gone took its tail with it: both lose counts that cannot be read back, so the total is
    reported as a lower bound.
    """
    delta, lower_bound = 0, bool(set(before) - set(after))
    for tid, end in after.items():
        start = before.get(tid)
        if start is None:
            delta += end
        elif end < start:
            delta += end
            lower_bound = True
        else:
            delta += end - start
    return delta, lower_bound


def rollup(pid):
    vals = {}
    with open(f"{PROC}/{pid}/smaps_rollup") as f:
        for line in f:
            parts = line.split()
            if len(parts) >= 2 and parts[0].endswith(":") and parts[1].isdigit():
                vals[parts[0][:-1]] = int(parts[1])
    return vals


def name(pid):
    with open(f"{PROC}/{pid}/cmdline", "rb") as f:
        first = f.read().split(b"\0")[0].decode(errors="replace")
    return os.path.basename(first) or "?"


def snapshot(pids):
    """Opening reading of every process of the tree: CPU ticks and per-tid wakeups."""
    start = {}
    for pid in pids:
        try:
            start[pid] = (ticks(pid), wakes_by_tid(pid))
        except OSError:
            pass
    return start


def sample_rows(label, start, elapsed):
    """Closing reading against `start`: one row per surviving process, plus the running total."""
    rows, total = [], [0.0] * 6
    for pid, (tk, wk) in start.items():
        try:
            r = rollup(pid)
            wakeups, lower_bound = wakeups_delta(wk, wakes_by_tid(pid))
            vals = [
                (ticks(pid) - tk) / HZ / elapsed * 100,
                wakeups / elapsed,
                r.get("Pss", 0) / 1024,
                r.get("Pss_Anon", 0) / 1024,
                r.get("Pss_File", 0) / 1024,
                (r.get("Private_Clean", 0) + r.get("Private_Dirty", 0)) / 1024,
            ]
            rows.append([label, str(pid), name(pid), f"{vals[0]:.1f}", f"{vals[1]:.1f}",
                         "1" if lower_bound else "0"]
                        + [f"{v:.1f}" for v in vals[2:]]
                        + [str(len(os.listdir(f"{PROC}/{pid}/task")))])
            total = [a + b for a, b in zip(total, vals)]
        except OSError:
            pass
    return rows, total


def total_row(label, total, lower_bound):
    return ([label, "-", "TOTAL", f"{total[0]:.1f}", f"{total[1]:.1f}",
             "1" if lower_bound else "0"]
            + [f"{v:.1f}" for v in total[2:]] + ["-"])


def main():
    label, root = sys.argv[1], int(sys.argv[2])
    secs = float(sys.argv[3]) if len(sys.argv) > 3 else 60.0
    out = sys.argv[4] if len(sys.argv) > 4 else "measure.tsv"

    start = snapshot(tree(root))
    t0 = time.monotonic()
    time.sleep(secs)
    elapsed = time.monotonic() - t0

    lb = COLS.index("wakeups_lb")
    rows, total = sample_rows(label, start, elapsed)
    rows.append(total_row(label, total, any(row[lb] == "1" for row in rows)))

    new = not os.path.exists(out) or os.path.getsize(out) == 0
    with open(out, "a") as f:
        if new:
            f.write("\t".join(COLS) + "\n")
        for row in rows:
            f.write("\t".join(row) + "\n")
    widths = [max(len(x) for x in col) for col in zip(COLS, *rows)]
    for row in [COLS] + rows:
        print("  ".join(x.ljust(w) for x, w in zip(row, widths)))


if __name__ == "__main__":
    main()
