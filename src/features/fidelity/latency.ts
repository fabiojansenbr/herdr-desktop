// Latency page observation contract of spec 007 (parent: src-tauri/tests/fidelity_latency.rs,
// tests/fidelity-latency/). Not wired to the native window yet. The page only (1) accounts
// trusted keydowns per attempt and (2) reports the real canvas/cell metrics the parent combines
// with its MEASURED viewport offset (window + GTK CSD) to sample the PTY cell marker in a
// compositor capture. JS event times are audit only: presentation time comes from the capture.

/** Must match tests/fidelity-latency/marker.rs (0-based row/col). */
export const MARKER = { row: 4, col: 3, cells: 48, minCols: 52, minRows: 5 } as const;

export interface KeyEventLike {
  type: string;
  key: string;
  isTrusted: boolean;
  repeat: boolean;
  timeStamp: number;
}

export interface JsAudit {
  key: string;
  time_origin_ms: number;
  time_stamp_ms: number;
}

export interface KeyLog {
  trusted: number;
  untrusted: number;
  /** Trusted but not a single non-repeat keydown of "a". */
  other: number;
  errors: string[];
  js_audit: JsAudit[];
}

export function createKeyObserver(timeOrigin: number) {
  let log: Omit<KeyLog, "errors"> = { trusted: 0, untrusted: 0, other: 0, js_audit: [] };
  return {
    onKeydown(e: KeyEventLike): void {
      log.js_audit.push({ key: e.key, time_origin_ms: timeOrigin, time_stamp_ms: e.timeStamp });
      if (!e.isTrusted) log.untrusted += 1;
      else if (e.type !== "keydown" || e.key !== "a" || e.repeat) log.other += 1;
      else log.trusted += 1;
    },
    take(): KeyLog {
      const errors: string[] = [];
      if (log.untrusted > 0) errors.push(`untrusted keydown x${log.untrusted}`);
      if (log.other > 0) errors.push(`repeat or foreign key x${log.other}`);
      if (log.trusted !== 1) errors.push(`expected exactly one trusted a, got ${log.trusted}`);
      const out = { ...log, errors };
      log = { trusted: 0, untrusted: 0, other: 0, js_audit: [] };
      return out;
    },
  };
}

export interface PageMetrics {
  devicePixelRatio: number;
  /** Terminal canvas getBoundingClientRect(), CSS px relative to the viewport. */
  canvasRect: { left: number; top: number; width: number; height: number };
  /** Cell size in CSS px as the mounted renderer lays out cells. */
  cellWidth: number;
  cellHeight: number;
  cols: number;
  rows: number;
}

/** Keys read by PageObservation::from_json in tests/fidelity-latency/marker.rs. */
export interface LatencyPage {
  dpr: number;
  canvas_left_css: number;
  canvas_top_css: number;
  cell_w_css: number;
  cell_h_css: number;
  cols: number;
  rows: number;
}

export type PageResult = { ok: true; page: LatencyPage } | { ok: false; error: string };

export function observePage(m: PageMetrics): PageResult {
  const pos = (x: number) => Number.isFinite(x) && x > 0;
  if (!pos(m.devicePixelRatio)) return { ok: false, error: `devicePixelRatio ${m.devicePixelRatio}` };
  if (!pos(m.cellWidth) || !pos(m.cellHeight)) return { ok: false, error: `cell metrics ${m.cellWidth}x${m.cellHeight}` };
  if (!Number.isInteger(m.cols) || m.cols < MARKER.minCols) return { ok: false, error: `cols ${m.cols} < ${MARKER.minCols}` };
  if (!Number.isInteger(m.rows) || m.rows < MARKER.minRows) return { ok: false, error: `rows ${m.rows} < ${MARKER.minRows}` };
  const r = m.canvasRect;
  if (![r.left, r.top].every(Number.isFinite) || r.width < MARKER.minCols * m.cellWidth || r.height < MARKER.minRows * m.cellHeight) {
    return { ok: false, error: `canvas ${JSON.stringify(r)} cannot contain marker cells` };
  }
  return {
    ok: true,
    page: { dpr: m.devicePixelRatio, canvas_left_css: r.left, canvas_top_css: r.top, cell_w_css: m.cellWidth, cell_h_css: m.cellHeight, cols: m.cols, rows: m.rows },
  };
}

/** Per-attempt page report; `untrusted_keydowns` includes foreign/repeat keys (parent requires 0). */
export function attemptPayload(attempt: number, page: LatencyPage, keys: KeyLog) {
  return {
    attempt,
    page,
    trusted_keydowns: keys.trusted,
    untrusted_keydowns: keys.untrusted + keys.other,
    errors: keys.errors,
    js_audit: keys.js_audit,
  };
}
