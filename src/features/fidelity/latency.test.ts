// Latency page observation contract (spec 007): trusted keydown accounting per attempt and the
// page geometry the parent needs to locate the cell marker. No JS time is a presentation time.
import { describe, expect, it } from "vitest";
import { MARKER, createKeyObserver, observePage, attemptPayload, type KeyEventLike, type PageMetrics } from "./latency";

const key = (over: Partial<KeyEventLike> = {}): KeyEventLike => ({ type: "keydown", key: "a", isTrusted: true, repeat: false, timeStamp: 12.5, ...over });

const metrics = (over: Partial<PageMetrics> = {}): PageMetrics => ({
  devicePixelRatio: 1.25,
  canvasRect: { left: 12, top: 40, width: 630, height: 493 },
  cellWidth: 8.4,
  cellHeight: 17,
  cols: 75,
  rows: 29,
  ...over,
});

describe("createKeyObserver", () => {
  it("counts one trusted a per attempt and resets on take", () => {
    const obs = createKeyObserver(1_000_000.25);
    obs.onKeydown(key());
    const first = obs.take();
    expect(first).toEqual({ trusted: 1, untrusted: 0, other: 0, errors: [], js_audit: [{ key: "a", time_origin_ms: 1_000_000.25, time_stamp_ms: 12.5 }] });
    expect(obs.take()).toMatchObject({ trusted: 0, errors: ["expected exactly one trusted a, got 0"] });
  });

  it("rejects synthetic, repeated, duplicated and foreign keys", () => {
    const cases: [KeyEventLike[], string][] = [
      [[key({ isTrusted: false })], "untrusted keydown"],
      [[key(), key({ repeat: true })], "repeat or foreign key"],
      [[key(), key()], "expected exactly one trusted a, got 2"],
      [[key({ key: "A" })], "repeat or foreign key"],
      [[key({ type: "keyup" }), key()], "repeat or foreign key"],
    ];
    for (const [events, needle] of cases) {
      const obs = createKeyObserver(0);
      events.forEach((e) => obs.onKeydown(e));
      expect(obs.take().errors.join("; ")).toContain(needle);
    }
  });
});

describe("observePage", () => {
  it("reports CSS metrics with the keys the Rust parser reads", () => {
    expect(observePage(metrics())).toEqual({
      ok: true,
      page: { dpr: 1.25, canvas_left_css: 12, canvas_top_css: 40, cell_w_css: 8.4, cell_h_css: 17, cols: 75, rows: 29 },
    });
  });

  it("refuses geometry that cannot hold the marker inside the real canvas", () => {
    expect(MARKER).toEqual({ row: 4, col: 3, cells: 48, minCols: 52, minRows: 5 });
    const bad: [Partial<PageMetrics>, string][] = [
      [{ cols: 51 }, "cols 51"],
      [{ rows: 4 }, "rows 4"],
      [{ devicePixelRatio: 0 }, "devicePixelRatio"],
      [{ devicePixelRatio: Number.NaN }, "devicePixelRatio"],
      [{ cellWidth: 0 }, "cell metrics"],
      // 52 cols of 8.4 px need 436.8 px: a 400 px canvas cannot contain the marker.
      [{ canvasRect: { left: 12, top: 40, width: 400, height: 493 } }, "canvas"],
      [{ canvasRect: { left: 12, top: 40, width: 630, height: 80 } }, "canvas"],
    ];
    for (const [over, needle] of bad) {
      const r = observePage(metrics(over));
      expect(r.ok).toBe(false);
      if (!r.ok) expect(r.error).toContain(needle);
    }
  });
});

describe("attemptPayload", () => {
  it("carries JS times only as audit, never as a presentation timestamp", () => {
    const obs = createKeyObserver(5);
    obs.onKeydown(key());
    const page = observePage(metrics());
    if (!page.ok) throw new Error(page.error);
    const payload = attemptPayload(7, page.page, obs.take());
    expect(payload).toEqual({
      attempt: 7,
      page: page.page,
      trusted_keydowns: 1,
      untrusted_keydowns: 0,
      errors: [],
      js_audit: [{ key: "a", time_origin_ms: 5, time_stamp_ms: 12.5 }],
    });
    expect(JSON.stringify(payload)).not.toMatch(/present|paint|raf/i);
  });

  it("counts foreign keys as untrusted so the parent evaluator rejects them", () => {
    const obs = createKeyObserver(0);
    obs.onKeydown(key());
    obs.onKeydown(key({ key: "b" }));
    const page = observePage(metrics());
    if (!page.ok) throw new Error(page.error);
    expect(attemptPayload(0, page.page, obs.take())).toMatchObject({ trusted_keydowns: 1, untrusted_keydowns: 1 });
  });
});
