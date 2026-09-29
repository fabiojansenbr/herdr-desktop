// Spec 005 — line diff between the chosen base snapshot and the current buffer.
import { describe, expect, it } from "vitest";
import { diffLines, diffSummary } from "./diff";

describe("diffLines", () => {
  // Would catch: a diff that loses line order, numbers lines from 0 or swaps removals/additions.
  it("classifies same, removed and added lines with 1-based origins", () => {
    const lines = diffLines("a\nb\nc\n", "a\nx\nc\ny\n");
    expect(lines.map((line) => [line.op, line.text, line.baseLine, line.currentLine])).toEqual([
      ["same", "a", 1, 1],
      ["removed", "b", 2, null],
      ["added", "x", null, 2],
      ["same", "c", 3, 3],
      ["added", "y", null, 4],
    ]);
    expect(diffSummary(lines)).toEqual({ added: 2, removed: 1 });
  });

  // Would catch: an empty diff reported as changed, or the identical buffers producing ops.
  it("reports identical content as a single unchanged line", () => {
    const lines = diffLines("linha\n", "linha\n");
    expect(lines.every((line) => line.op === "same")).toBe(true);
    expect(diffSummary(lines)).toEqual({ added: 0, removed: 0 });
  });

  // Would catch: a middle-only diff algorithm that reports the whole file as changed when only
  // the end changed (prefix/suffix trimming must keep untouched lines).
  it("keeps untouched prefix and suffix out of the changed block", () => {
    const base = ["1", "2", "3", "4", "5", "6", "7"].join("\n");
    const current = ["1", "2", "3", "novo", "5", "6", "7"].join("\n");
    const lines = diffLines(base, current);
    expect(lines.filter((line) => line.op === "same").map((line) => line.text)).toEqual([
      "1",
      "2",
      "3",
      "5",
      "6",
      "7",
    ]);
    expect(diffSummary(lines)).toEqual({ added: 1, removed: 1 });
  });

  // Would catch: a quadratic blowup on a big replacement (the editor must stay responsive).
  it("summarises a large replacement without hanging", () => {
    const base = Array.from({ length: 4000 }, (_, i) => `antiga ${i}`).join("\n");
    const current = Array.from({ length: 4000 }, (_, i) => `nova ${i}`).join("\n");
    const lines = diffLines(base, current);
    expect(diffSummary(lines)).toEqual({ added: 4000, removed: 4000 });
  });
});
