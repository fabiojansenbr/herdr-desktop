// Spec 053 — the UI font travels in the bundle: app.css declares @font-face "Inter" pointing at
// the vendored variable woff2, and --font-ui still asks for it first. Would catch: dropping the
// @font-face and silently falling back to system-ui (Liberation Sans on this host), pointing the
// src at a path that no longer exists, narrowing the variable weight range, or swapping the file
// for something that is not a woff2. Reads the CSS and the asset: the accepted exception for
// asset configuration (AC-053-01).
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const cssPath = resolve("src/app.css");
const css = readFileSync(cssPath, "utf8");

/** The @font-face block that declares the "Inter" family, without the surrounding braces. */
function interFace(): string {
  for (const match of css.matchAll(/@font-face\s*\{([^}]*)\}/g)) {
    const block = match[1] ?? "";
    if (/font-family:\s*"Inter"\s*;/.test(block)) return block;
  }
  throw new Error('app.css has no @font-face with font-family: "Inter"');
}

/** First capture of `pattern` in `text`, or "" when it does not match. */
function capture(pattern: RegExp, text: string): string {
  return pattern.exec(text)?.[1] ?? "";
}

describe("fonte Inter embutida", () => {
  it("declara o @font-face da Inter variável", () => {
    const block = interFace();
    expect(block).toMatch(/font-weight:\s*100\s+900\s*;/);
    expect(block).toMatch(/font-style:\s*normal\s*;/);
    expect(block).toMatch(/font-display:\s*block\s*;/);
    expect(block).toMatch(/format\("woff2"\)/);
  });

  it("aponta para o woff2 vendorizado, que existe e é um woff2", () => {
    const url = capture(/src:\s*url\(([^)]+)\)/, interFace()).replace(/^["']|["']$/g, "");
    expect(url, "o @font-face da Inter precisa de src: url(…)").not.toBe("");
    expect(url).toMatch(/InterVariable\.woff2$/);
    expect(url.startsWith("/"), "o src deve ser relativo ao app.css").toBe(false);
    expect(/^[a-z]+:/.test(url), "o src deve ser local, não remoto").toBe(false);

    const file = resolve(dirname(cssPath), url);
    expect(file).toBe(resolve("src/assets/fonts/InterVariable.woff2"));
    expect(readFileSync(file).subarray(0, 4).toString("latin1")).toBe("wOF2");
  });

  it("mantém a Inter na frente de --font-ui", () => {
    const fontUi = capture(/--font-ui:\s*([^;]+);/, css).trim();
    expect(fontUi, "app.css precisa de --font-ui").not.toBe("");
    expect(fontUi.startsWith('"Inter"')).toBe(true);
  });
});
