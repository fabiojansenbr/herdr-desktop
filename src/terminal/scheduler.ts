// Dirty-row repaint scheduling for one terminal canvas. At most one animation frame is pending;
// while hidden, rows accumulate and no frame is requested (zero periodic repaint), and a frame
// that fires after hiding paints nothing and keeps its rows for the next visible frame.

export class RepaintScheduler {
  private rows = new Set<number>();
  private handle = 0;
  private visible = true;

  constructor(
    private readonly request: (cb: () => void) => number,
    private readonly cancel: (handle: number) => void,
    private readonly paint: (rows: Set<number>) => void,
  ) {}

  get pendingCount(): number {
    return this.rows.size;
  }

  schedule(rows: Iterable<number>): void {
    for (const r of rows) this.rows.add(r);
    this.arm();
  }

  setVisible(visible: boolean): void {
    this.visible = visible;
    if (!visible && this.handle) {
      this.cancel(this.handle);
      this.handle = 0;
    }
    this.arm();
  }

  dispose(): void {
    if (this.handle) this.cancel(this.handle);
    this.handle = 0;
    this.rows.clear();
  }

  private arm(): void {
    if (!this.visible || this.handle || this.rows.size === 0) return;
    const handle = this.request(() => {
      if (this.handle !== handle || !this.visible) return;
      this.handle = 0;
      const rows = this.rows;
      this.rows = new Set();
      this.paint(rows);
    });
    this.handle = handle;
  }
}
