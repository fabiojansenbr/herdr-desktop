// IME/keyboard/paste contract for the terminal's editable target (spec 007, AC-007-02 slice).
// Every sequence below replays the exact event order observed in the native WebKitGTK logs kept
// read-only in the main checkout:
//   .local/native-input/out/20260916T210031/{plain,ime}/summary.txt (GTK dead key, fcitx5 hello)
//   .local/prep-cjk/out/20260916T233319/summary.txt (fcitx5 Pinyin: nihao/pinyin/zhongwen)
// The replay feeds those events to the same InputRouter TerminalView uses, so the assertions
// cover the real GTK/fcitx5 order instead of an idealized one. This contract does not prove the
// native PTY path; the composed window E2E belongs to spec 007 itself.
import { describe, expect, it } from "vitest";
import { InputRouter, isClipboardPasteChord, isNativePasteChord, nativePasteRoute } from "./composition";
import type { KeyLike } from "./input";
import { MOD_ALT, MOD_CONTROL, MOD_SHIFT, type InputDto } from "./types";

type Step =
  | { type: "keydown"; event: KeyLike }
  | { type: "start" }
  | { type: "update"; data: string }
  | { type: "end"; data: string }
  | { type: "paste"; text: string };

const chord = (key: string, mods: Partial<Omit<KeyLike, "key">> = {}): KeyLike => ({
  key,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...mods,
});
const down = (key: string, mods: Partial<Omit<KeyLike, "key">> = {}): Step => ({ type: "keydown", event: chord(key, mods) });
const start = (): Step => ({ type: "start" });
const update = (data: string): Step => ({ type: "update", data });
const end = (data: string): Step => ({ type: "end", data });
const paste = (text: string): Step => ({ type: "paste", text });

function replay(steps: Step[], enabled = true) {
  const router = new InputRouter();
  const sent: InputDto[] = [];
  const preedits: string[] = [];
  for (const step of steps) {
    let input: InputDto | null = null;
    switch (step.type) {
      case "keydown":
        input = router.keydown(step.event, enabled);
        break;
      case "start":
        router.compositionStart();
        break;
      case "update":
        router.compositionUpdate(step.data);
        break;
      case "end":
        input = router.compositionEnd(step.data, enabled);
        break;
      case "paste":
        input = router.paste(step.text, enabled);
        break;
    }
    if (input) sent.push(input);
    preedits.push(router.preedit);
  }
  return { sent, preedits, composing: router.composing, preedit: router.preedit };
}

describe("plain keys (native plain log: keysym accents and Return)", () => {
  // Would catch: accents split into Dead+base, Return committed as text, or the browser's
  // follow-up insertText event forwarded again.
  it("sends printable keys, accents and Enter exactly once each", () => {
    const { sent } = replay([
      down("a"),
      down("ç"),
      down("ã"),
      down("o"),
      down("Enter"),
      down("Backspace"),
    ]);
    expect(sent).toEqual([
      { kind: "text", text: "a" },
      { kind: "text", text: "ç" },
      { kind: "text", text: "ã" },
      { kind: "text", text: "o" },
      { kind: "key", code: "Enter", modifiers: 0 },
      { kind: "key", code: "Backspace", modifiers: 0 },
    ]);
  });

  // Would catch: modifier-only/dead/composing keydowns leaking to the PTY.
  it("never sends Dead, Unidentified, modifier-only or isComposing keydowns", () => {
    const { sent } = replay([
      down("Dead"),
      down("Unidentified"),
      down("Shift"),
      down("Control"),
      down("a", { isComposing: true }),
      down("D", { ctrlKey: true, shiftKey: true, isComposing: true }),
    ]);
    expect(sent).toEqual([]);
  });
});

describe("GTK native dead key (plain log, deadkey_compose)", () => {
  // Would catch: input sent during preedit; only the 'é' insertFromComposition/commit forwarded;
  // both compositionend events ('', then 'é') emitting; the empty end resetting dedupe.
  it("keeps '´' as preedit and commits 'é' exactly once after the empty compositionend", () => {
    const { sent, preedits, composing, preedit } = replay([
      down("Unidentified"),
      start(),
      update("´"),
      down("Unidentified", { isComposing: true }),
      end(""),
      down("Unidentified"),
      end("é"),
    ]);
    expect(sent).toEqual([{ kind: "text", text: "é" }]);
    expect(preedits).toContain("´");
    expect(composing).toBe(false);
    expect(preedit).toBe("");
  });

  // Would catch: an empty-only composition producing input.
  it("does not commit an empty compositionend", () => {
    expect(replay([down("Unidentified"), start(), update("´"), down("Unidentified", { isComposing: true }), end("")]).sent).toEqual([]);
  });
});

describe("fcitx5 IME (ime log)", () => {
  // Would catch: any preedit update (h→hello) reaching the PTY or the commit being dropped.
  it("commits 'hello' on compositionend and still sends the following space", () => {
    const { sent, preedits } = replay([
      down("Unidentified"),
      start(),
      update("h"),
      down("Unidentified", { isComposing: true }),
      update("he"),
      down("Unidentified", { isComposing: true }),
      update("hel"),
      down("Unidentified", { isComposing: true }),
      update("hell"),
      down("Unidentified", { isComposing: true }),
      update("hello"),
      down("Unidentified", { isComposing: true }),
      end("hello"),
      down(" "),
    ]);
    expect(sent).toEqual([
      { kind: "text", text: "hello" },
      { kind: "text", text: " " },
    ]);
    expect(preedits).toContain("hello");
  });

  // The ime log registers `´ → é` without any compositionend inside ~600ms: the text stays
  // preedit. Would catch: committing text that the IME never confirmed.
  it("sends nothing when the IME leaves the dead key as preedit", () => {
    const { sent, composing, preedit } = replay([
      down("Unidentified"),
      start(),
      update("´"),
      down("Unidentified", { isComposing: true }),
      update("é"),
      down("Unidentified", { isComposing: true }),
    ]);
    expect(sent).toEqual([]);
    expect(composing).toBe(true);
    expect(preedit).toBe("é");
  });
});

describe("CJK Pinyin (prep-cjk log: candidates selected by Space or number)", () => {
  const preedit = (...letters: string[]): Step[] => {
    const steps: Step[] = [down("Unidentified"), start()];
    for (const [i, data] of letters.entries()) {
      if (i > 0) steps.push(down("Unidentified", { isComposing: true }));
      steps.push(update(data));
    }
    steps.push(down("Unidentified", { isComposing: true }));
    return steps;
  };

  // Would catch: preedit pinyin reaching the PTY, or Space/2 forwarded as a key.
  it("commits '你好' on Space with zero bytes during preedit", () => {
    const { sent, preedits } = replay([...preedit("n", "ni", "ni h", "ni ha", "ni hao"), end("你好"), down("Enter")]);
    expect(sent).toEqual([
      { kind: "text", text: "你好" },
      { kind: "key", code: "Enter", modifiers: 0 },
    ]);
    expect(preedits).toContain("ni hao");
  });

  // Same phonetic input, different candidate: the fixture must not be hard-coded to one result.
  it("commits the candidate text the IME confirms", () => {
    expect(replay([...preedit("pin", "pin y", "pin yi", "pin yin"), end("品饮")]).sent).toEqual([{ kind: "text", text: "品饮" }]);
    expect(replay([...preedit("pin", "pin y", "pin yi", "pin yin"), end("拼音")]).sent).toEqual([{ kind: "text", text: "拼音" }]);
    expect(replay([...preedit("zhon", "zhong", "zhong w", "zhong wen"), end("中文")]).sent).toEqual([{ kind: "text", text: "中文" }]);
  });
});

describe("composition sessions", () => {
  // Negative case for the duplication contract: some WebKit keys arrive as a printable keydown
  // with isComposing=false before the compositionend. The preedit still owns the keyboard, so
  // the key must not be sent once as text and again as the commit.
  it("does not send a printable keydown while the composition owns the keyboard", () => {
    const { sent } = replay([start(), update("he"), down("e"), update("hello"), down("l"), end("hello")]);
    expect(sent).toEqual([{ kind: "text", text: "hello" }]);
  });

  // Would catch: dedupe leaking across sessions (second dead key swallowed).
  it("commits once per session", () => {
    const { sent } = replay([start(), update("é"), end("é"), start(), update("ã"), end("ã")]);
    expect(sent).toEqual([
      { kind: "text", text: "é" },
      { kind: "text", text: "ã" },
    ]);
  });

  // Would catch: a duplicated compositionend of the same commit emitting twice.
  it("ignores a repeated non-empty end until a new session starts", () => {
    const { sent } = replay([start(), update("x"), end("x"), end("x")]);
    expect(sent).toEqual([{ kind: "text", text: "x" }]);
  });
});

describe("GTK commits without compositionstart (composed window, evidencias/007/native-live/run3)", () => {
  // Exact order of run3/phase-reports.json native-keys.event_log: dead keys ´e and ~a, Return,
  // then wtype "🙂👍🏽": WebKitGTK sends each emoji as keydown Unidentified + insertFromComposition
  // + compositionend with no compositionstart.
  const deadKeys: Step[] = [
    down("Unidentified"), start(), update("´"), down("Unidentified"), end(""), down("Unidentified"), end("é"),
    down("Unidentified"), start(), update("~"), down("Unidentified"), end(""), down("Unidentified"), end("ã"),
  ];
  const emoji: Step[] = [down("Unidentified"), end("🙂"), down("Unidentified"), end("👍"), down("Unidentified"), end("🏽")];
  const texts = (sent: InputDto[]) => sent.filter((i) => i.kind === "text").map((i) => (i as { text: string }).text);

  // Would catch: the dead key's committed flag swallowing every later start-less commit (0 PTY bytes).
  it("commits each emoji once after dead keys and Return", () => {
    const { sent } = replay([...deadKeys, down("Enter"), ...emoji]);
    expect(texts(sent)).toEqual(["é", "ã", "🙂", "👍", "🏽"]);
    expect(sent).toHaveLength(6);
  });

  // Would catch: only the first start-less commit of a fresh router being sent.
  it("commits consecutive start-less commits, including equal ones, once per key", () => {
    expect(texts(replay(emoji).sent)).toEqual(["🙂", "👍", "🏽"]);
    expect(texts(replay([down("Unidentified"), end("🙂"), down("Unidentified"), end("🙂")]).sent)).toEqual(["🙂", "🙂"]);
  });

  // Would catch: the fix turning a duplicated end (no new key) into a second commit.
  it("still ignores a repeated start-less end without a new key", () => {
    expect(texts(replay([down("Unidentified"), end("🙂"), end("🙂")]).sent)).toEqual(["🙂"]);
    expect(texts(replay([...deadKeys, end("ã")]).sent)).toEqual(["é", "ã"]);
  });

  // Would catch: a start-less commit dropped while disabled being replayed once enabled.
  it("does not replay a start-less commit dropped while disabled", () => {
    const router = new InputRouter();
    expect(router.keydown({ key: "Unidentified", ctrlKey: false, altKey: false, shiftKey: false, metaKey: false }, false)).toBeNull();
    expect(router.compositionEnd("🙂", false)).toBeNull();
    expect(router.compositionEnd("🙂", true)).toBeNull();
  });
});

describe("paste and duplication (keydown vs paste)", () => {
  // Would catch: Ctrl+V keydown forwarded as a control char and the paste event as well.
  // Ctrl+Shift+V is no longer here: WebKitGTK emits no ClipboardEvent for it, so it belongs to
  // the native paste chord (describe below), never to the browser-resolved paste.
  it("delivers Ctrl+V/Cmd+V/Cmd+Shift+V/Shift+Insert text once, through the paste event only", () => {
    for (const browserChord of [
      down("v", { ctrlKey: true }),
      down("V", { metaKey: true }),
      down("V", { metaKey: true, shiftKey: true }),
      down("Insert", { shiftKey: true }),
    ]) {
      const { sent } = replay([browserChord, paste("echo A\necho B\n")]);
      expect(sent).toEqual([{ kind: "paste", text: "echo A\necho B\n" }]);
    }
  });

  // Would catch: the chord predicate swallowing terminal keys that are not paste.
  it("keeps Ctrl+Alt+V, plain v and Ctrl+X as terminal input", () => {
    expect(replay([down("v", { ctrlKey: true, altKey: true })]).sent).toEqual([
      { kind: "key", code: "Char", modifiers: MOD_CONTROL | MOD_ALT, ch: "v" },
    ]);
    expect(replay([down("v")]).sent).toEqual([{ kind: "text", text: "v" }]);
    expect(replay([down("x", { ctrlKey: true })]).sent).toEqual([{ kind: "key", code: "Char", modifiers: MOD_CONTROL, ch: "x" }]);
    expect(replay([down("Insert")]).sent).toEqual([{ kind: "key", code: "Insert", modifiers: 0 }]);
  });

  // Would catch: empty clipboard or whitespace-only text producing paste/key events.
  it("ignores an empty paste and keeps multi-line text in one event", () => {
    expect(replay([paste("")]).sent).toEqual([]);
    expect(replay([paste("echo A\necho B\n")]).sent).toEqual([{ kind: "paste", text: "echo A\necho B\n" }]);
  });
});

describe("inputEnabled=false", () => {
  const everything: Step[] = [
    down("a"),
    down("Enter"),
    down("v", { ctrlKey: true }),
    paste("echo A"),
    down("Unidentified"),
    start(),
    update("ni hao"),
    down("Unidentified", { isComposing: true }),
    end("你好"),
  ];

  // Would catch: any route bypassing the inputEnabled gate, including IME commits and paste.
  it("never sends input from keydown, composition or paste", () => {
    expect(replay(everything, false).sent).toEqual([]);
  });

  // Would catch: a commit dropped while disabled being replayed when input is re-enabled.
  it("does not replay a commit dropped while disabled", () => {
    const router = new InputRouter();
    router.compositionStart();
    router.compositionUpdate("x");
    expect(router.compositionEnd("x", false)).toBeNull();
    expect(router.compositionEnd("x", true)).toBeNull();
  });

  // The preedit is local presentation: it stays tracked even while input is gated.
  it("still tracks the preedit for display", () => {
    const { composing, preedit } = replay([down("Unidentified"), start(), update("你好"), down("Unidentified", { isComposing: true })], false);
    expect(composing).toBe(true);
    expect(preedit).toBe("你好");
  });
});

describe("modifier bits preserved", () => {
  // Would catch: the router losing crossterm bits on the way to the PTY.
  it("keeps Shift/Control/Alt bits from input.ts", () => {
    expect(replay([down("ArrowUp", { ctrlKey: true })]).sent).toEqual([{ kind: "key", code: "Up", modifiers: MOD_CONTROL }]);
    expect(replay([down("Tab", { shiftKey: true })]).sent).toEqual([{ kind: "key", code: "BackTab", modifiers: 0 }]);
    expect(replay([down("C", { ctrlKey: true, shiftKey: true })]).sent).toEqual([{ kind: "key", code: "Char", modifiers: MOD_CONTROL | MOD_SHIFT, ch: "c" }]);
  });
});

// Spec 028 r4a: every paste chord (Ctrl+V, Cmd+V, Shift+Insert and Ctrl+Shift+V) is native — the
// view prevents the browser default and asks the host once; the host decides from the clipboard
// content. Ctrl+Shift+V has no ClipboardEvent in WebKitGTK 2.52.6 (fidelity_native_paste.rs doc),
// which is why it was the first chord on this path; no chord may ever reach the PTY as a control
// Char.
describe("native paste chords (spec 028 r4a)", () => {
  it("classifies Ctrl+Shift+V as native and keeps Ctrl+V/Cmd+V/Shift+Insert on the paste event", () => {
    expect(isNativePasteChord(chord("v", { ctrlKey: true, shiftKey: true }))).toBe(true);
    expect(isNativePasteChord(chord("V", { ctrlKey: true, shiftKey: true }))).toBe(true);
    for (const other of [
      chord("v", { ctrlKey: true }),
      chord("v", { ctrlKey: true, altKey: true, shiftKey: true }),
      chord("v", { metaKey: true, shiftKey: true }),
      chord("Insert", { shiftKey: true }),
      chord("c", { ctrlKey: true, shiftKey: true }),
    ]) {
      const label = `native ${other.key} ctrl=${other.ctrlKey} alt=${other.altKey} shift=${other.shiftKey} meta=${other.metaKey}`;
      expect([label, isNativePasteChord(other)]).toEqual([label, false]);
    }

    expect(isClipboardPasteChord(chord("v", { ctrlKey: true, shiftKey: true }))).toBe(false);
    expect(isClipboardPasteChord(chord("v", { ctrlKey: true }))).toBe(true);
    expect(isClipboardPasteChord(chord("V", { metaKey: true }))).toBe(true);
    expect(isClipboardPasteChord(chord("V", { metaKey: true, shiftKey: true }))).toBe(true);
    expect(isClipboardPasteChord(chord("Insert", { shiftKey: true }))).toBe(true);
    expect(isClipboardPasteChord(chord("v", { ctrlKey: true, altKey: true }))).toBe(false);
    expect(isClipboardPasteChord(chord("Insert", { ctrlKey: true, shiftKey: true }))).toBe(false);
  });

  // Would catch: a key repeat pasting twice, a paste with input blocked or during preedit, and a
  // missing callback becoming a fabricated ClipboardEvent instead of the explicit notice.
  it("routes one gesture per chord: repeat/blocked ignored, preedit left alone, missing callback explicit", () => {
    const active = { composing: false, enabled: true, hasHandler: true };
    expect(nativePasteRoute(chord("v", { ctrlKey: true, shiftKey: true }), active)).toBe("paste");
    expect(nativePasteRoute(chord("v", { ctrlKey: true, shiftKey: true, repeat: true }), active)).toBe("ignore");
    expect(nativePasteRoute(chord("v", { ctrlKey: true, shiftKey: true }), { ...active, enabled: false })).toBe("ignore");
    expect(nativePasteRoute(chord("v", { ctrlKey: true, shiftKey: true }), { ...active, composing: true })).toBe(null);
    expect(nativePasteRoute(chord("v", { ctrlKey: true, shiftKey: true }), { ...active, hasHandler: false })).toBe("unavailable");
    expect(nativePasteRoute(chord("v", { ctrlKey: true }), active)).toBe("paste");
    expect(nativePasteRoute(chord("a", { ctrlKey: true, shiftKey: true }), active)).toBe(null);
  });

  // Spec 028 r4a. Would catch: the native route gated by mouse_reporting/alt-screen again, which
  // left a mouse-aware app (Claude Code, grok) with no path to the host command.
  it("routes every paste chord natively in every pane, mouse-reporting included", () => {
    const active = { composing: false, enabled: true, hasHandler: true };
    const shell = { mouse_reporting: false, alternate_screen_active: false };
    const app = { mouse_reporting: true, alternate_screen_active: false };
    const fullscreen = { mouse_reporting: false, alternate_screen_active: true };
    const both = { mouse_reporting: true, alternate_screen_active: true };
    const chords = [
      chord("v", { ctrlKey: true }),
      chord("V", { metaKey: true }),
      chord("V", { metaKey: true, shiftKey: true }),
      chord("Insert", { shiftKey: true }),
      chord("v", { ctrlKey: true, shiftKey: true }),
    ];
    for (const e of chords) {
      for (const pane of [shell, app, fullscreen, both, null, undefined]) {
        const label = `chord ${e.key} ctrl=${e.ctrlKey} shift=${e.shiftKey} meta=${e.metaKey} pane=${JSON.stringify(pane)}`;
        expect([label, nativePasteRoute(e, { ...active, pane })]).toEqual([label, "paste"]);
      }
    }
    // Non-chords and the gated states keep their routes.
    expect(nativePasteRoute(chord("a", { ctrlKey: true, shiftKey: true }), active)).toBe(null);
    expect(nativePasteRoute(chord("v", { ctrlKey: true }), { ...active, composing: true })).toBe(null);
  });

  // Would catch: the native chord (or its repeat) reaching the PTY as a control Char.
  it("never sends Ctrl+Shift+V or its repeat as terminal input and keeps typing afterwards", () => {
    const { sent } = replay([
      down("v", { ctrlKey: true, shiftKey: true }),
      down("v", { ctrlKey: true, shiftKey: true, repeat: true }),
      down("a"),
    ]);
    expect(sent).toEqual([{ kind: "text", text: "a" }]);
  });

  // Spec 028 r4a: one gesture, one paste. The native command reads the clipboard on the chord's
  // keydown; if WebKitGTK still emits the same gesture's ClipboardEvent (observed with empty
  // text/plain for an image), the router consumes it as a no-op. Would catch: the browser event
  // pasting a second time after a handled chord.
  it("makes the browser paste event of a handled chord a no-op, then restores the route", () => {
    const router = new InputRouter();
    const app = { mouse_reporting: true, alternate_screen_active: false };
    expect(nativePasteRoute(chord("v", { ctrlKey: true }), { composing: false, enabled: true, hasHandler: true, pane: app })).toBe("paste");
    router.pasteChordHandled();
    expect(router.paste("texto fantasma", true)).toBeNull();
    expect(router.paste("echo ok", true)).toEqual({ kind: "paste", text: "echo ok" });
  });

  // Would catch: the handled marker surviving the gesture and swallowing a later clipboard paste
  // that has no chord (middle click), after the key is released or the target loses focus.
  it("clears the handled chord on key release and on focus change", () => {
    for (const clear of [(router: InputRouter) => router.pasteChordEnded(), (router: InputRouter) => router.focusChanged(false)]) {
      const router = new InputRouter();
      router.pasteChordHandled();
      clear(router);
      expect(router.paste("texto", true)).toEqual({ kind: "paste", text: "texto" });
    }
  });

  // Would catch: the view routing Ctrl+Shift+V through the browser (fabricating a paste) or
  // calling the handler on key repeat; no DOM in this environment, so the probe guards the wiring
  // of the same rules the native window uses (the composed E2E remains the real proof). Spec 028
  // r4a: every chord marks the gesture in the router and the key release ends it.
  it("TerminalView preventDefaults the chord and asks the surface once through onNativePaste", () => {
    const source = Object.values(
      import.meta.glob("./TerminalView.svelte", { query: "?raw", import: "default", eager: true }) as Record<string, string>,
    )[0]!;
    expect(source).toMatch(/import\s*\{[^}]*\bnativePasteRoute\b[^}]*\}\s*from\s*"\.\/composition"/);
    expect(source).toMatch(/onNativePaste\?:/);
    expect(source).toMatch(/nativePasteRoute\(e, \{\s*composing: router\.composing,\s*enabled: inputAllowed\(\),\s*hasHandler: onNativePaste !== undefined,\s*pane: focused \? \{ mouse_reporting: focused\.mouse_reporting, alternate_screen_active: focused\.alternate_screen_active \} : null,\s*\}\)/);
    expect(source).toMatch(/if \(decision\) \{\s*e\.preventDefault\(\);\s*router\.pasteChordHandled\(\);/);
    expect(source).toMatch(/if \(decision === "paste"\) \{\s*void Promise\.resolve\(onNativePaste\?\.\(\)\)/);
    expect(source).toMatch(/onkeyup=\{onKeyUp\}/);
    expect(source).toMatch(/function onKeyUp\(\) \{\s*router\.pasteChordEnded\(\);\s*\}/);
    expect(source).toMatch(/t\("terminal\.unavailable\.paste"\)/);
    expect(source).not.toMatch(/new ClipboardEvent/);
    expect(source).toMatch(/herdr-pane-menu/);
    expect(source).not.toMatch(/dispatchEvent\(\s*new (ClipboardEvent|CustomEvent\("paste")/);
  });

  // Spec 028 AC-028-03. Would catch: a right click always going to the app, or the pane menu
  // never reaching the center region that renders it.
  it("TerminalView routes right clicks through rightClickRoute and emits herdr-pane-menu", () => {
    const source = Object.values(
      import.meta.glob("./TerminalView.svelte", { query: "?raw", import: "default", eager: true }) as Record<string, string>,
    )[0]!;
    expect(source).toMatch(/import\s*\{[^}]*\brightClickRoute\b[^}]*\}\s*from\s*"\.\/interaction"/);
    expect(source).toMatch(/rightClickRoute\(hit, \{ shift: e\.shiftKey \}, \{ focusedPaneId: focusedPaneId\(\), passthrough: readRightClickPassthrough\(hit\.pane\.pane_id\) \}\)/);
    expect(source).toMatch(/new CustomEvent\("herdr-pane-menu"/);
    expect(source).toMatch(/readRightClickPassthrough/);
  });

  // Spec 028 r3 — the debug trace of the right click and of the paste shortcut (the window test
  // reads the host's stderr). Would catch: a call site removed, renamed or traced with other
  // fields, or a trace driving the route instead of reporting it.
  it("TerminalView traces the contextmenu entry, the route, the paste chord and its result", () => {
    const source = Object.values(
      import.meta.glob("./TerminalView.svelte", { query: "?raw", import: "default", eager: true }) as Record<string, string>,
    )[0]!;
    expect(source).toMatch(/import\s*\{[^}]*\buiTrace\b[^}]*\}\s*from\s*"\.\.\/shell\/ui-trace"/);
    expect(source).toMatch(/uiTrace\(contextMenuLine\(\{ clientX: e\.clientX, clientY: e\.clientY, target: targetName\(e\.target\), paneId: hit\?\.pane\.pane_id \?\? null \}\)\)/);
    expect(source).toMatch(/uiTrace\(rightClickLine\(hit\.pane\.pane_id, route\)\)/);
    expect(source).toMatch(/uiTrace\(pasteLine\(\{ chord: chordName\(e\), route: decision, paneId: focused\?\.pane_id \?\? null \}\)\)/);
    expect(source).toMatch(/uiTrace\(pasteResultLine\(paste\)\)/);
  });
});
