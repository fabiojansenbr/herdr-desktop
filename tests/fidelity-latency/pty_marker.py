#!/usr/bin/env python3
"""Spec 007 latency PTY helper: runs inside the measured Herdr pane.

Raw, no-echo terminal. Paints the cell marker for seq 0 once, then for every received ASCII
'a' increments count/seq and paints the new marker exactly once. Any other byte is logged and
ignored (no marker, no advance). No timers, no prepaint: output is caused only by input.
Log lines (owned file, O_EXCL, ns = time.monotonic_ns() = CLOCK_MONOTONIC):
  hd-pty v1 ready cols=C rows=R seq=0 ns=N
  hd-pty v1 recv byte=0xHH count=C seq=S emitted=0|1 ns=N
Marker layout must match tests/fidelity-latency/marker.rs.
"""
import argparse
import os
import sys
import termios
import time
import tty

MAGIC, SUFFIX = 0xA5, 0x5A
ROW, COL, CELLS = 4, 3, 48
ON, OFF = (0xF8, 0xFA, 0xFC), (0x1E, 0x29, 0x3B)
MIN_COLS, MIN_ROWS = COL + CELLS + 1, ROW + 1


def marker(seq: int) -> bytes:
    inv = ~seq & 0xFFFF
    data = bytes([MAGIC, seq >> 8, seq & 0xFF, inv >> 8, inv & 0xFF, SUFFIX])
    out = [f"\x1b[{ROW + 1};{COL + 1}H"]
    for i in range(CELLS):
        r, g, b = ON if data[i // 8] & (0x80 >> (i % 8)) else OFF
        out.append(f"\x1b[48;2;{r};{g};{b}m ")
    out.append("\x1b[0m")
    return "".join(out).encode()


def write_all(fd: int, data: bytes) -> None:
    while data:
        data = data[os.write(fd, data):]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--log", required=True)
    args = ap.parse_args()
    log = os.open(args.log, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC, 0o600)

    def note(line: str) -> None:
        write_all(log, f"hd-pty v1 {line} ns={time.monotonic_ns()}\n".encode())

    cols, rows = os.get_terminal_size(1)
    if cols < MIN_COLS or rows < MIN_ROWS:
        note(f"geometry cols={cols} rows={rows}")
        return 3
    tty.setraw(0, termios.TCSANOW)
    attrs = termios.tcgetattr(0)
    attrs[3] &= ~termios.ECHO
    termios.tcsetattr(0, termios.TCSANOW, attrs)
    count = 0
    write_all(1, b"\x1b[?25l" + marker(0))
    note(f"ready cols={cols} rows={rows} seq=0")
    while True:
        chunk = os.read(0, 1)
        if not chunk:
            return 0
        byte = chunk[0]
        emitted = 0
        if byte == ord("a"):
            if count == 0xFFFF:
                note(f"recv byte=0x{byte:02x} count={count} seq={count} emitted=0")
                return 4
            count += 1
            write_all(1, marker(count))
            emitted = 1
        note(f"recv byte=0x{byte:02x} count={count} seq={count} emitted={emitted}")


if __name__ == "__main__":
    sys.exit(main())
