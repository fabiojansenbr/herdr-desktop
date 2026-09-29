#!/usr/bin/env python3
"""Tests for scripts/measure_resources.py.

Regression target (spec 009, TASK-009-01): the `wakeups_s` column went negative in the
2026-09-21 baseline (evidencias/009/baseline-2026-09-21/desktop.tsv, row `4-15panes-idle`:
-333.3 for the window and -1483.8 for the WebProcess). Cause: `voluntary_ctxt_switches` is a
per-thread counter, and the script summed it over the threads alive at each end of the window.
The WebProcess went from 38 to 27 threads inside that window, so the counters of the 11 exited
threads vanished from the second sum and the difference underflowed.

The fix samples the counters per tid and aggregates only over tids still alive at the end, so
the result can never be negative; when the thread set shrank (or a tid was recycled) the value
is a deterministic lower bound and is flagged as such instead of being silently fabricated.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import measure_resources as mr


def write(path: str, text: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)


def fake_proc(root: str, pid: int, tids: dict[int, int], *, name: str = "fake",
              utime: int = 0, stime: int = 0, pss: int = 0, children: str = "") -> None:
    """Builds a minimal /proc/<pid> tree: stat, cmdline, smaps_rollup and one task per tid."""
    shutil.rmtree(f"{root}/{pid}/task", ignore_errors=True)
    stat_rest = " ".join(["0"] * 11 + [str(utime), str(stime)] + ["0"] * 30)
    write(f"{root}/{pid}/stat", f"{pid} (fake name) S {stat_rest}\n")
    write(f"{root}/{pid}/cmdline", f"/usr/bin/{name}\0--flag\0")
    write(f"{root}/{pid}/smaps_rollup",
          f"Pss:  {pss} kB\nPss_Anon:  {pss} kB\nPss_File:  0 kB\n"
          "Private_Clean:  0 kB\nPrivate_Dirty:  0 kB\n")
    for tid, wakeups in tids.items():
        write(f"{root}/{pid}/task/{tid}/status",
              f"Name:\t{name}\nvoluntary_ctxt_switches:\t{wakeups}\n"
              "nonvoluntary_ctxt_switches:\t7\n")
        write(f"{root}/{pid}/task/{tid}/children", children if tid == pid else "")


class WakeupsDeltaTest(unittest.TestCase):
    """The pure aggregation over per-tid counters."""

    def test_stable_threads_give_the_exact_delta(self):
        before = {1: 100, 2: 50, 3: 0}
        after = {1: 180, 2: 50, 3: 12}
        self.assertEqual(mr.wakeups_delta(before, after), (92, False))

    def test_exited_threads_never_produce_a_negative_delta(self):
        # The WebProcess row of the 2026-09-21 baseline: 38 threads down to 27, and the
        # threads that went away carried most of the accumulated counter.
        before = {tid: 5000 for tid in range(1, 39)}
        after = {tid: 5000 + 3 for tid in range(1, 28)}
        delta, lower_bound = mr.wakeups_delta(before, after)
        self.assertEqual(delta, 27 * 3)
        self.assertTrue(lower_bound)
        # The old whole-process formula is what produced the negative number.
        self.assertLess(sum(after.values()) - sum(before.values()), 0)

    def test_a_new_thread_counts_its_whole_counter_and_is_not_a_lower_bound(self):
        # A thread created inside the window starts its counter at 0, so its current value is
        # exactly what it accumulated in the window.
        delta, lower_bound = mr.wakeups_delta({1: 10}, {1: 10, 2: 40})
        self.assertEqual((delta, lower_bound), (40, False))

    def test_a_recycled_tid_counts_the_new_thread_and_flags_the_lower_bound(self):
        delta, lower_bound = mr.wakeups_delta({1: 10, 2: 900}, {1: 10, 2: 4})
        self.assertEqual((delta, lower_bound), (4, True))

    def test_every_thread_gone_yields_zero_not_a_negative_number(self):
        self.assertEqual(mr.wakeups_delta({1: 900, 2: 700}, {}), (0, True))

    def test_empty_window_is_zero_and_exact(self):
        self.assertEqual(mr.wakeups_delta({}, {}), (0, False))


class WakesByTidTest(unittest.TestCase):
    """Reading the per-thread counters out of /proc."""

    def test_reads_one_counter_per_task(self):
        with tempfile.TemporaryDirectory() as root:
            fake_proc(root, 42, {42: 11, 43: 22, 44: 0})
            with mr.proc_root(root):
                self.assertEqual(mr.wakes_by_tid(42), {42: 11, 43: 22, 44: 0})

    def test_a_task_that_disappears_while_reading_is_skipped(self):
        with tempfile.TemporaryDirectory() as root:
            fake_proc(root, 42, {42: 11, 43: 22})
            os.remove(f"{root}/42/task/43/status")
            with mr.proc_root(root):
                self.assertEqual(mr.wakes_by_tid(42), {42: 11})


class RowsTest(unittest.TestCase):
    """The sampled rows, end to end, against a synthetic /proc."""

    def test_a_shrinking_thread_set_reports_a_flagged_non_negative_rate(self):
        with tempfile.TemporaryDirectory() as root:
            fake_proc(root, 42, {42: 5000, 43: 5000, 44: 5000}, pss=1024)
            with mr.proc_root(root):
                start = mr.snapshot([42])
                fake_proc(root, 42, {42: 5010}, pss=1024)
                rows, total = mr.sample_rows("lbl", start, elapsed=2.0)
        self.assertEqual(len(rows), 1)
        row = dict(zip(mr.COLS, rows[0]))
        self.assertEqual(row["wakeups_s"], "5.0")
        self.assertEqual(row["wakeups_lb"], "1")
        self.assertEqual(row["pss_mib"], "1.0")
        self.assertEqual(total[mr.COLS.index("wakeups_s") - 3], 5.0)

    def test_a_stable_tree_is_not_flagged_as_a_lower_bound(self):
        with tempfile.TemporaryDirectory() as root:
            fake_proc(root, 42, {42: 100, 43: 200}, pss=2048)
            with mr.proc_root(root):
                start = mr.snapshot([42])
                fake_proc(root, 42, {42: 130, 43: 200}, pss=2048)
                rows, _ = mr.sample_rows("lbl", start, elapsed=3.0)
        row = dict(zip(mr.COLS, rows[0]))
        self.assertEqual(row["wakeups_s"], "10.0")
        self.assertEqual(row["wakeups_lb"], "0")

    def test_the_total_row_flags_the_lower_bound_when_any_process_does(self):
        with tempfile.TemporaryDirectory() as root:
            fake_proc(root, 42, {42: 10}, pss=100, children="43")
            fake_proc(root, 43, {43: 10, 44: 10}, pss=100)
            with mr.proc_root(root):
                start = mr.snapshot(mr.tree(42))
                self.assertEqual(sorted(start), [42, 43])
                fake_proc(root, 42, {42: 20}, pss=100, children="43")
                fake_proc(root, 43, {43: 10}, pss=100)
                rows, total = mr.sample_rows("lbl", start, elapsed=1.0)
        table = {row[mr.COLS.index("pid")]: dict(zip(mr.COLS, row)) for row in rows}
        self.assertEqual(table["42"]["wakeups_lb"], "0")
        self.assertEqual(table["43"]["wakeups_lb"], "1")
        total_row = mr.total_row("lbl", total, lower_bound=True)
        self.assertEqual(dict(zip(mr.COLS, total_row))["wakeups_lb"], "1")
        self.assertEqual(dict(zip(mr.COLS, total_row))["wakeups_s"], "10.0")


class CliTest(unittest.TestCase):
    """The script still runs against real /proc and writes a well-formed table."""

    def test_measuring_a_real_short_lived_tree_writes_only_non_negative_wakeups(self):
        script = os.path.join(os.path.dirname(os.path.abspath(__file__)), "measure_resources.py")
        # A python child that starts and joins 24 threads inside the measured window: the
        # thread set shrinks exactly like the WebProcess did in the baseline.
        child_src = (
            "import threading,time\n"
            "ts=[threading.Thread(target=lambda: time.sleep(0.2)) for _ in range(24)]\n"
            "[t.start() for t in ts]\n"
            "[t.join() for t in ts]\n"
            "time.sleep(1.2)\n"
        )
        with tempfile.TemporaryDirectory() as out_dir:
            out = os.path.join(out_dir, "out.tsv")
            child = subprocess.Popen([sys.executable, "-c", child_src])
            try:
                proc = subprocess.run(
                    [sys.executable, script, "cli-smoke", str(child.pid), "1.0", out],
                    capture_output=True, text=True, timeout=60)
            finally:
                child.wait(timeout=30)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            with open(out) as f:
                lines = [line.rstrip("\n").split("\t") for line in f]
        self.assertEqual(lines[0], mr.COLS)
        self.assertGreaterEqual(len(lines), 3)
        for row in lines[1:]:
            cells = dict(zip(mr.COLS, row))
            self.assertGreaterEqual(float(cells["wakeups_s"]), 0.0, row)
            self.assertIn(cells["wakeups_lb"], ("0", "1"), row)
            self.assertGreaterEqual(float(cells["cpu_pct"]), 0.0, row)


if __name__ == "__main__":
    unittest.main()
