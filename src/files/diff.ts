// Pure line diff between the chosen base snapshot and the current buffer. No editor import:
// the diff view must work (and be tested) without loading CodeMirror.

export type DiffOp = "same" | "removed" | "added";

export interface DiffLine {
  op: DiffOp;
  text: string;
  /** 1-based line in the base snapshot, or null for an added line. */
  baseLine: number | null;
  /** 1-based line in the current buffer, or null for a removed line. */
  currentLine: number | null;
}

/** Avoids a quadratic blowup: above this many cells the changed block is summarised. */
const MAX_LCS_CELLS = 1_000_000;

function splitLines(text: string): string[] {
  const lines = text.split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

interface Op {
  op: DiffOp;
  aIndex: number;
  bIndex: number;
}

function commonOps(a: string[], b: string[], offset: number): Op[] {
  const n = a.length;
  const m = b.length;
  const width = m + 1;
  const dp = new Int32Array((n + 1) * width);
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i * width + j] =
        a[i] === b[j]
          ? dp[(i + 1) * width + j + 1]! + 1
          : Math.max(dp[(i + 1) * width + j]!, dp[i * width + j + 1]!);
    }
  }
  const ops: Op[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      ops.push({ op: "same", aIndex: offset + i, bIndex: offset + j });
      i++;
      j++;
    } else if (dp[(i + 1) * width + j]! >= dp[i * width + j + 1]!) {
      ops.push({ op: "removed", aIndex: offset + i, bIndex: -1 });
      i++;
    } else {
      ops.push({ op: "added", aIndex: -1, bIndex: offset + j });
      j++;
    }
  }
  while (i < n) {
    ops.push({ op: "removed", aIndex: offset + i, bIndex: -1 });
    i++;
  }
  while (j < m) {
    ops.push({ op: "added", aIndex: -1, bIndex: offset + j });
    j++;
  }
  return ops;
}

/** Line diff with 1-based origins; a huge fully-changed block is summarised, never dropped. */
export function diffLines(base: string, current: string): DiffLine[] {
  const a = splitLines(base);
  const b = splitLines(current);
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }

  const midA = a.slice(start, endA);
  const midB = b.slice(start, endB);
  let middle: Op[];
  if (midA.length * midB.length > MAX_LCS_CELLS) {
    middle = [
      ...midA.map((_, index) => ({ op: "removed" as const, aIndex: start + index, bIndex: -1 })),
      ...midB.map((_, index) => ({ op: "added" as const, aIndex: -1, bIndex: start + index })),
    ];
  } else {
    middle = commonOps(midA, midB, start);
  }

  const lines: DiffLine[] = [];
  for (let index = 0; index < start; index++) {
    lines.push({ op: "same", text: a[index]!, baseLine: index + 1, currentLine: index + 1 });
  }
  for (const op of middle) {
    lines.push({
      op: op.op,
      text: op.op === "added" ? b[op.bIndex]! : a[op.aIndex]!,
      baseLine: op.aIndex >= 0 ? op.aIndex + 1 : null,
      currentLine: op.bIndex >= 0 ? op.bIndex + 1 : null,
    });
  }
  for (let index = endA; index < a.length; index++) {
    lines.push({
      op: "same",
      text: a[index]!,
      baseLine: index + 1,
      currentLine: endB + (index - endA) + 1,
    });
  }
  return lines;
}

export function diffSummary(lines: DiffLine[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const line of lines) {
    if (line.op === "added") added++;
    if (line.op === "removed") removed++;
  }
  return { added, removed };
}
