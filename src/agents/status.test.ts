// Spec 004 — AC-004-02 at the presentation seam.
import { describe, expect, it } from "vitest";
import { AGENT_STATUSES, normalizeStatus, statusPresentation } from "./status";

describe("agent status presentation", () => {
  // Would catch: two states sharing text or icon (colour-only distinction), or a missing state.
  it("gives each of the five engine states its own text and icon", () => {
    expect(AGENT_STATUSES).toEqual(["working", "blocked", "idle", "done", "unknown"]);
    const labels = AGENT_STATUSES.map((s) => statusPresentation(s).label);
    const icons = AGENT_STATUSES.map((s) => statusPresentation(s).icon);
    expect(new Set(labels).size).toBe(5);
    expect(new Set(icons).size).toBe(5);
    for (const s of AGENT_STATUSES) {
      expect(statusPresentation(s).label.trim().length).toBeGreaterThan(0);
      expect(statusPresentation(s).icon.trim().length).toBeGreaterThan(0);
    }
  });

  // Would catch: unknown, missing or new wire values shown as done (or as idle).
  it("never turns unknown, missing or unexpected values into done", () => {
    const done = statusPresentation("done");
    for (const wire of ["unknown", undefined, null, "", "finished", "DONE", "completed", 3]) {
      const status = normalizeStatus(wire);
      expect(status).toBe("unknown");
      expect(statusPresentation(status).label).not.toBe(done.label);
      expect(statusPresentation(status).icon).not.toBe(done.icon);
      expect(statusPresentation(status).label).not.toBe(statusPresentation("idle").label);
    }
    expect(normalizeStatus("done")).toBe("done");
    expect(normalizeStatus("blocked")).toBe("blocked");
  });

  // Would catch: blocked presented as a passive state (no call to the user's attention).
  it("marks only blocked as needing the user", () => {
    expect(AGENT_STATUSES.filter((s) => statusPresentation(s).needsUser)).toEqual(["blocked"]);
  });
});
