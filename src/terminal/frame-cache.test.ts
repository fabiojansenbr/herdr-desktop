import { describe, expect, it } from "vitest";
import { HostFrameCache } from "./frame-cache";
import type { CachedFrame } from "./frame-cache";

function sampleFrame(endpoint: string, overrides: Partial<CachedFrame> = {}): CachedFrame {
  return {
    endpoint,
    boot_id: "boot-1",
    connection_generation: 1,
    width: 80,
    height: 24,
    cells: [{ s: "A", fg: 0, bg: 0, m: 0 }],
    cursor: { x: 0, y: 0, visible: true, shape: 2 },
    panes: [
      {
        pane_id: "p1",
        content_revision: 1,
        rect: { x: 0, y: 0, width: 80, height: 24 },
        inner_rect: { x: 0, y: 0, width: 80, height: 24 },
        scroll: null,
        focused: true,
        mouse_reporting: false,
        sgr_pixel_mouse: false,
        alternate_screen_active: false,
        pixel_width: 720,
        pixel_height: 432,
      },
    ],
    links: [],
    revision: 10,
    ...overrides,
  };
}

describe("HostFrameCache (spec 037)", () => {
  it("stores at most one frame per host (endpoint + geração + boot)", () => {
    const cache = new HostFrameCache();
    const frameA1 = sampleFrame("local", { revision: 1 });
    cache.set(frameA1);
    expect(cache.has("local")).toBe(true);
    expect(cache.get("local")?.revision).toBe(1);

    const frameA2 = sampleFrame("local", { revision: 2 });
    cache.set(frameA2);
    // Overwrites previous frame for local (at most 1 frame)
    expect(cache.get("local")?.revision).toBe(2);

    const frameB = sampleFrame("ssh-dev", { boot_id: "boot-b", connection_generation: 5, revision: 10 });
    cache.set(frameB);
    expect(cache.get("ssh-dev")?.revision).toBe(10);
    expect(cache.get("local")?.revision).toBe(2);
  });

  it("discards frame if connection drops (drop)", () => {
    const cache = new HostFrameCache();
    cache.set(sampleFrame("local"));
    cache.set(sampleFrame("ssh-dev"));

    cache.drop("ssh-dev");
    expect(cache.get("ssh-dev")).toBeNull();
    expect(cache.get("local")).not.toBeNull();
  });

  it("discards frame if geometry changes (cols/rows mismatch)", () => {
    const cache = new HostFrameCache();
    cache.set(sampleFrame("local", { width: 80, height: 24 }));
    cache.set(sampleFrame("ssh-dev", { width: 80, height: 24 }));

    // Resize to 100x30
    cache.invalidateGeometry(100, 30);
    expect(cache.get("local")).toBeNull();
    expect(cache.get("ssh-dev")).toBeNull();
  });

  it("keeps frame if geometry is identical", () => {
    const cache = new HostFrameCache();
    cache.set(sampleFrame("local", { width: 80, height: 24 }));

    cache.invalidateGeometry(80, 24);
    expect(cache.get("local")).not.toBeNull();
  });

  it("discards frame if boot_id or connection_generation differs", () => {
    const cache = new HostFrameCache();
    cache.set(sampleFrame("local", { boot_id: "boot-old", connection_generation: 1 }));

    // Identity check on get
    expect(cache.get("local", { boot_id: "boot-new", connection_generation: 1 })).toBeNull();
    expect(cache.has("local")).toBe(false);

    cache.set(sampleFrame("local", { boot_id: "boot-1", connection_generation: 1 }));
    expect(cache.get("local", { boot_id: "boot-1", connection_generation: 2 })).toBeNull();
    expect(cache.has("local")).toBe(false);

    cache.set(sampleFrame("local", { boot_id: "boot-1", connection_generation: 1 }));
    expect(cache.get("local", { boot_id: "boot-1", connection_generation: 1 })).not.toBeNull();
  });
});
