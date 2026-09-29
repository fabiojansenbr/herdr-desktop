// AC-007-02 keyboard order around the terminal (contract; the native a11y-navigation sweep is the
// proof). The IME textarea keeps Tab/Shift+Tab for the PTY and Ctrl+Shift+F6 focuses the exit
// marker (tabindex -1), so the next Tab moves to the first tabbable AFTER the marker in DOM order.
// GUI r7: the actions toolbar ("Copiar seleção") sat between the textarea and the marker, so no
// forward Tab ever reached it (c36 expected, never focused). No DOM here: guards the markup order.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const view = readFileSync(new URL("./TerminalView.svelte", import.meta.url), "utf8");
const markup = view.slice(view.indexOf("</script>"), view.indexOf("<style>"));

describe("TerminalView exit order", () => {
  // Would catch: the chord focusing something other than the marker (e.g. the next panel), which
  // would skip the terminal's own actions.
  it("the exit chord still focuses the exit marker when no onFocusExit is given", () => {
    expect(view).toMatch(/if \(onFocusExit\) onFocusExit\(\);\s*else exitMarker\?\.focus\(\);/);
  });

  // Would catch (r7): the toolbar, link field or any other control placed between the textarea and
  // the marker, where forward Tab cannot reach it (textarea keeps Tab; Tab from the marker skips back).
  it("the marker follows the textarea with no control between them, and the actions come after it", () => {
    const textarea = markup.indexOf('<textarea\n    class="ime-target"');
    const textareaEnd = markup.indexOf("</textarea>", textarea);
    const marker = markup.indexOf('<span class="exit-marker" tabindex="-1" bind:this={exitMarker}>');
    const actions = markup.indexOf('<div class="actions" role="toolbar"');
    expect(textarea).toBeGreaterThan(-1);
    expect(marker).toBeGreaterThan(textareaEnd);
    expect(actions).toBeGreaterThan(marker);
    const between = markup.slice(textareaEnd, marker);
    expect(between).not.toMatch(/<(button|input|select|textarea|a|summary)\b|tabindex="?0/);
  });
});
