// Spec 013 — structure the native `visual-center` phase relies on and rules that no DOM test can
// see: the overlay never drives a render loop, the frames are placed from `inner_rect` (never from
// the engine's border box), the stage reserves the band the frames and the terminal toolbar use,
// and the toolbar no longer floats over row 0 (the debt inherited from 010).
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const sources = import.meta.glob(["./*.svelte", "../../terminal/TerminalView.svelte"], {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

const center = sources["./CenterRegion.svelte"]!;
const frames = sources["./PaneFrames.svelte"]!;
const header = sources["./WorkspaceHeader.svelte"]!;
const popover = sources["./NewAgentPopover.svelte"]!;
const tabs = sources["./WorkspaceTabs.svelte"]!;
const view = sources["../../terminal/TerminalView.svelte"]!;

const styleOf = (source: string) => source.slice(source.indexOf("<style>"), source.indexOf("</style>"));
const scriptOf = (source: string) => source.slice(0, source.indexOf("</script>"));
const markupOf = (source: string) => source.slice(source.indexOf("</script>"), source.indexOf("<style>"));

describe("pane frames overlay (AC-013-02, AC-013-03)", () => {
  // Would catch: an overlay that animates or polls, which would keep the window (and the hidden
  // terminal's probe) busy; the frames must move only when a frame/layout event says so.
  it("never runs an animation frame, a timer or a poll", () => {
    expect(scriptOf(frames)).not.toMatch(/requestAnimationFrame|setInterval|setTimeout|pane\.read/);
    expect(scriptOf(center)).not.toMatch(/requestAnimationFrame|setInterval|setTimeout/);
    // Measurements come from the metadata/full events and the layout observer, nothing else.
    expect(scriptOf(frames)).toContain("controllers.surface.subscribe");
    expect(scriptOf(frames)).toContain("new ResizeObserver");
  });

  // Would catch: frames drawn while the files/home layer is shown, or for a host with no metadata.
  it("frames exist only while the terminal view is shown and the metadata has panes", () => {
    expect(scriptOf(frames)).toMatch(/const shown = \$derived\(ctx\.view === "terminal"\)/);
    expect(scriptOf(frames)).toMatch(/shown && origin && metrics && panes\.length > 0/);
  });

  // Would catch: the frame box taking the engine's border box (`rect`) or the band drawn inside the
  // content, which is what covered the link cell in row 0 before.
  it("frame boxes come from inner_rect and the band from the shared definition", () => {
    expect(scriptOf(frames)).toContain("paneFrames(");
    // The patch metadata carries only the updated panes: it is merged, never assigned wholesale.
    expect(scriptOf(frames)).toContain("applySurfaceMetadata(layout, event)");
    expect(scriptOf(frames)).not.toMatch(/panes = event\.panes/);
    expect(frames).not.toContain("pane.rect");
    expect(markupOf(frames)).toContain("data-pane-frame=");
    expect(markupOf(frames)).not.toContain("data-pane-band=");
    expect(view).toContain('import { bandOffset } from "./pane-band"');
  });

  // Would catch: the edge collapsing into one colour, or a border that shrinks the measured box
  // (the AC measures the frame against `inner_rect × cell` with 1 px of tolerance).
  it("split panes use a 1 px divider; a single pane has no wrap border or radius", () => {
    const style = styleOf(frames);
    expect(style).toMatch(/\.frame \{[^}]*border:\s*none/);
    expect(style).toMatch(/\.frame \{[^}]*border-radius:\s*0/);
    expect(style).toMatch(/\.divider \{[^}]*background:\s*var\(--border\)/);
    expect(style).toMatch(/\.divider\[data-edge="accent"\] \{[^}]*background:\s*var\(--accent\)/);
  });

  // Would catch: an overlay that eats the terminal's pointer events (selection, links, mouse apps).
  it("the overlay does not take pointer events; only the split controls do", () => {
    const style = styleOf(frames);
    expect(style).toMatch(/\.overlay \{[^}]*pointer-events: none/);
    expect(style).toMatch(/\.splits \{[^}]*pointer-events: auto/);
  });

  // Would catch: the expand button wired to a local zoom instead of the engine's, or a click that
  // focuses the pane locally without asking the engine.
  it("the frame's actions are the engine's (focus, zoom, split) through the existing controller", () => {
    expect(scriptOf(frames)).toContain("ctx.controllers.agents.focusPane(pane_id)");
    expect(scriptOf(frames)).toContain("ctx.controllers.agents.zoomPane(pane_id)");
    expect(scriptOf(frames)).toContain('ctx.controllers.agents.split(direction)');
    expect(markupOf(frames)).toContain("data-zoom=");
    expect(markupOf(frames)).toContain("data-split-right");
    expect(markupOf(frames)).toContain("data-split-down");
  });

  // Would catch: a disconnected host still offering actions, or frames without the cache mark.
  it("a host that is not live disables split and zoom", () => {
    expect(scriptOf(frames)).toMatch(/const live = \$derived\(ctx\.surface\.phase === "live"\)/);
    expect(markupOf(frames)).toMatch(/aria-disabled=\{zoomReason !== null\}/);
    expect(markupOf(frames)).toMatch(/aria-disabled=\{splitDisabled\}/);
  });
});

describe("center region layout (AC-013-02)", () => {
  // Would catch: the band removed, so the frame header of a pane at row 0 (and the terminal's
  // toolbar) would go back over the first content row.
  it("the stage fills the tab with 8 px padding and no header band", () => {
    expect(styleOf(center)).toMatch(/padding:\s*8px/);
    expect(styleOf(center)).toMatch(/background:\s*var\(--bg\)/);
    expect(center).not.toContain("headerHeightPx");
    expect(markupOf(center)).not.toContain("--frame-band:");
  });

  // Spec 055 AC-055-01. Would catch: the tab row left in the center after the bar took the tabs,
  // which is the doubled strip the user asked to remove — the stage starts at the top instead.
  it("the region no longer owns the tab strip: the stage is its only row", () => {
    expect(center).not.toContain("WorkspaceTabs");
    expect(markupOf(center)).not.toContain("data-center-tabs");
    expect(markupOf(center)).toContain("data-center-stage");
    // The strip moved whole, it was not reimplemented in the bar.
    const bar = readFileSync(resolve("src/components/frame/TitleBar.svelte"), "utf8");
    expect(bar).toContain('import WorkspaceTabs from "../center/WorkspaceTabs.svelte"');
    expect(bar).toMatch(/<WorkspaceTabs\b/);
  });
});

describe("terminal action bar in the pane frame (010 debt)", () => {
  // Would catch: the toolbar back in the corner over row 0, where it swallowed the Ctrl+click of a
  // link (spec 010 r4) and covered cells of the focused pane.
  it("the toolbar is positioned in the band of its pane, not over the surface corner", () => {
    expect(view).toMatch(/const toolbarBand = \$derived\.by/);
    expect(view).toMatch(/<div class="actions" role="toolbar" class:in-band=\{toolbarBand !== null\}[^>]*style=\{toolbarBand\}/);
    expect(styleOf(view)).toMatch(/\.actions\.in-band \{[^}]*top: auto;[^}]*right: auto;/);
    // The container may not clip the band above the canvas.
    expect(styleOf(view)).toMatch(/\.terminal \{[^}]*overflow: visible/);
  });

  // Would catch: the bar bound to a pane it does not belong to (the link's pane is the one the user
  // acted on; the selection's and the focused pane follow).
  it("the bar follows the pane of the link, the selection or the focus", () => {
    expect(view).toContain("link?.request.pane_id ?? selection.paneId ?? pendingFocus ?? meta.focused()?.pane_id");
    // A pane of an older layout (a link kept from a previous phase) falls back to the focused
    // pane's band, never to the corner over the content cells.
    expect(view).toContain("const pane = (paneId ? meta.pane(paneId) : undefined) ?? meta.focused();");
  });
});

describe("project header and workspace tabs (AC-013-01)", () => {
  // Would catch: the header inventing actions, or the buttons bypassing the window's own actions.
  it("the header renders the model's crumbs, branch and path and calls the existing actions", () => {
    expect(scriptOf(header)).toContain("projectHeader(");
    expect(markupOf(header)).toContain("data-branch");
    expect(markupOf(header)).toContain("data-path");
    expect(markupOf(header)).toMatch(/data-action="split"[\s\S]*ctx\.actions\.split\(\)/);
    expect(markupOf(header)).toMatch(/data-action="newTab"[\s\S]*ctx\.actions\.newTab\(\)/);
    expect(markupOf(header)).toMatch(/data-action="newAgent"/);
    expect(markupOf(header)).toMatch(/<NewAgentPopover/);
    expect(scriptOf(popover)).toContain("startAgent()");
    // The engine refuses a name with a space or a colon: the name comes from the model's rule.
    expect(scriptOf(popover)).toContain("agentName(chosen, paneId)");
  });

  // Would catch (the gate r3 of spec 013 found it): buttons offered while the agents topology and
  // the surface disagree on the pane, so the action is addressed to a pane the engine does not have
  // and is refused in silence.
  it("the header actions are unavailable with a reason until the confirmed target exists", () => {
    expect(scriptOf(header)).toContain("confirmedTarget({ identity: ctx.surface.identity, agents: ctx.agents })");
    expect(scriptOf(header)).toContain("actionAvailability({ target, unavailable: ctx.unavailable })");
    expect(scriptOf(header)).toContain("const target = $derived(confirmedTarget({ identity: ctx.surface.identity, agents: ctx.agents }))");
    expect(scriptOf(tabs)).toContain("confirmedTarget({ identity: ctx.surface.identity, agents: ctx.agents })");
    // The control stays focusable (the a11y sweep of spec 007 tabs through every control and
    // requires a focus ring on each): unavailability is aria-disabled plus a refused action.
    for (const source of [header, tabs, frames]) {
      expect(markupOf(source)).not.toMatch(/\sdisabled=/);
      expect(markupOf(source)).toMatch(/aria-disabled=/);
    }
    expect(markupOf(header)).toMatch(/aria-disabled=\{reasons\.split !== null\}/);
    expect(markupOf(header)).toMatch(/title=\{reasons\.split \?\? t\("center\.header\.splitTitle"\)\}/);
    expect(markupOf(tabs)).toMatch(/aria-disabled=\{available\.newTab !== null\}/);
    // Every action checks its reason before running (a click on an unavailable control does nothing).
    expect(scriptOf(header)).toMatch(/function run\(action: HeaderAction, perform: \(\) => void\)/);
    expect(scriptOf(header)).toContain("if (reasons[action] !== null) return;");
  });

  // Would catch: a second agent started in a pane that already has one (the AC asks for one start).
  it("Novo agente stays the primary control on Local and never starts a second agent on an occupied pane", () => {
    expect(scriptOf(header)).toMatch(/localLive = \$derived\(ctx\.surface\.selection\?\.kind === "local" && ctx\.surface\.phase === "live"\)/);
    expect(markupOf(header)).toMatch(/aria-disabled=\{reasons\.newAgent !== null\}/);
    expect(markupOf(header)).toMatch(/class="primary"/);
    // Occupied panes are skipped: the popover looks for an empty pane (or splits) before start.
    expect(scriptOf(popover)).toMatch(/occupied\.has\(paneId\)/);
    expect(scriptOf(popover)).toContain("startAgent()");
  });

  // Would catch: tabs built from the global `focused` flag, or "+" wired to something other than
  // the engine's tab creation (spec 028: a new tab, never a split or the Novo agente popover).
  it("the tab bar renders the model and creates a tab from +", () => {
    expect(scriptOf(tabs)).toContain("workspaceTabs(");
    expect(scriptOf(tabs)).toContain("focusedTabId: agents?.tabFocus?.tab_id ?? null");
    expect(markupOf(tabs)).toContain("data-label");
    expect(markupOf(tabs)).toContain("data-more");
    expect(markupOf(tabs)).toContain("data-new-tab");
    expect(markupOf(tabs)).not.toContain("data-command");
    expect(markupOf(tabs)).not.toContain("<NewAgentPopover");
    expect(markupOf(tabs)).toMatch(/onclick=\{newTab\}/);
    expect(scriptOf(tabs)).toMatch(/function newTab\(\) \{\s*void ctx\.controllers\.agents\.createTab\(\);\s*\}/);
    expect(markupOf(tabs)).toMatch(/data-tab-icon=\{tab\.icon\}/);
    // More than eight tabs scroll (with arrows); none is dropped.
    expect(markupOf(tabs)).toMatch(/tabs\.length > 8/);
    expect(styleOf(tabs)).toMatch(/\.strip \{[^}]*overflow-x: auto/);
  });

  // Would catch: the pane-derived title (spec 026) rendered inside one truncating text node, so
  // the +N of a multi-pane tab disappears with the ellipsis, or a tab growing past 280 px.
  it("the tab title truncates with ellipsis while +N stays visible, up to 280 px", () => {
    expect(styleOf(tabs)).toMatch(/\.tab \{[^}]*max-width:\s*280px/);
    expect(styleOf(tabs)).toMatch(/\.label \{[^}]*display:\s*inline-flex/);
    expect(styleOf(tabs)).toMatch(/\.names \{[^}]*text-overflow:\s*ellipsis/);
    expect(styleOf(tabs)).toMatch(/\.more \{[^}]*flex:\s*none/);
    // Titles come only from the subscribed metadata: the bar never reads a pane.
    expect(scriptOf(tabs)).not.toMatch(/pane\.read|pane_read/);
    expect(scriptOf(tabs)).toContain("layoutPanes");
  });
});
