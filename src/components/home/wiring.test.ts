// Spec 012 — static source probes of the home screen. They guard wiring a pure unit test cannot
// see (which route mounts the screen, which existing action each button calls, that no home
// source opens a frame loop or sends a key to an agent); the AC logic and the thumbnail read
// budget live in `home-model.test.ts`, which renders no Svelte component.

import { describe, expect, it } from "vitest";

const sources = import.meta.glob(
  ["./*.svelte", "./*Region.svelte", "../frame/ActivityBar.svelte", "../../App.svelte"],
  { query: "?raw", import: "default", eager: true },
) as Record<string, string>;

const home = sources["./HomeScreen.svelte"]!;
const region = sources["./HomeRegion.svelte"]!;
const card = sources["./ProjectCard.svelte"]!;
const strip = sources["./AttentionStrip.svelte"]!;
const server = sources["./ServerCard.svelte"]!;
const activityBar = sources["../frame/ActivityBar.svelte"]!;
const app = sources["../../App.svelte"]!;
const markup = (source: string) => source.slice(source.indexOf("</script>"));

describe("AC-012-01 the home screen and its three buttons", () => {
  // Would catch: the 010 slot left empty, or Início not selecting it.
  it("mounts HomeScreen from the Início route", () => {
    expect(markup(region)).toMatch(/<HomeScreen\s+\{ctx\}\s*\/>/);
    expect(region).toMatch(/import HomeScreen from "\.\/HomeScreen\.svelte"/);
    expect(activityBar).toMatch(/onSelect\("home"\)/);
    expect(activityBar).toMatch(/aria-label=\{t\("frame\.activity\.home"\)\}/);
    expect(activityBar).not.toMatch(/frame\.activity\.home\.unavailable/);
    expect(markup(activityBar)).toMatch(/class:active=\{active === "home"\}/);
    // Spec 018 unmounted the home screen from App; Início and the empty-state buttons stay on
    // the isolated HomeScreen / ActivityBar (this file), not on the composed window.
    expect(region).toMatch(/<HomeScreen\s+\{ctx\}\s*\/>/);
    expect(app).not.toMatch(/homeShown = true/);
  });

  it("renders the greeting, the summary and the empty state from the model", () => {
    expect(markup(home)).toMatch(/data-greeting/);
    expect(markup(home)).toMatch(/data-summary/);
    expect(markup(home)).toMatch(/data-home-empty/);
    expect(home).toMatch(/\{view\.greeting\}/);
    expect(home).toMatch(/\{view\.summary\.text\}/);
    expect(home).toMatch(/buildHomeView\(/);
    expect(home).toMatch(/bridge\s*\n?\s*\.systemUser\(\)/);
  });

  it("wires the three buttons to the existing actions", () => {
    // Would catch: a home-only dialog, a new collection form or a project picker built here.
    expect(markup(home)).toMatch(/data-home-connect[^>]*onclick=\{connectServer\}/);
    expect(markup(home)).toMatch(/data-home-new-group[^>]*onclick=\{newGroup\}/);
    expect(markup(home)).toMatch(/data-home-open-project[^>]*onclick=\{openProject\}/);
    expect(home).toMatch(/ctx\.controllers\.connections\.openDialog\(\)/);
    expect(home).toMatch(/ctx\.actions\.showProjects\(\)/);
    expect(home).toMatch(/\[data-slot='projects'\] \[data-new-group-input\]/);
    expect(home).toMatch(/ctx\.controllers\.projects\.openFolder\(pickProjectFolder,/);
    expect(home).toMatch(/pickProjectFolder/);
    expect(home).not.toMatch(/ctx\.actions\.openPalette\(\)/);
  });
});

describe("AC-012-02 cards, thumbnails and waiting chips", () => {
  it("renders the card pieces of the design through the components", () => {
    expect(markup(home)).toMatch(/<ProjectCard\s+\{card\}\s+cache=\{view\.cache\}\s*\/>/);
    expect(home).toMatch(/import ProjectCard from "\.\/ProjectCard\.svelte"/);
    expect(markup(home)).toMatch(/<AttentionStrip\s+chips=\{view\.attention\}/);
    expect(home).toMatch(/import AttentionStrip from "\.\/AttentionStrip\.svelte"/);
    expect(markup(card)).toMatch(/data-project-card=\{card\.id\}/);
    expect(card).toMatch(/\{card\.branch\}/);
    expect(card).toMatch(/\{card\.path\}/);
    expect(markup(card)).toMatch(/data-thumbnails/);
    expect(markup(card)).toMatch(/data-thumbnail-pane=\{thumb\.paneId\}/);
    expect(markup(card)).toMatch(/data-status=\{thumb\.dot\.status\}/);
    expect(markup(card)).toMatch(/data-thumbnail-text/);
    expect(card).toMatch(/\{card\.activity\.time\}/);
    expect(card).toMatch(/card\.activity\.avatars/);
  });

  it("takes one static snapshot per visit through the model, never a frame", () => {
    // Would catch: a repaint loop, a timer per card or a live terminal in the thumbnails.
    expect(home).toMatch(/createThumbnailSnapshots\(/);
    expect(home).toMatch(/snapshots\.beginVisit\(\)/);
    expect(home).toMatch(/snapshots\.ensure\(/);
    expect(home).toMatch(/bridge\.paneRead\(target, paneId, lines\)/);
    expect(home).toMatch(/ctx\.controllers\.agents\.target\(paneId\)/);
    for (const [file, source] of Object.entries(sources)) {
      expect([file, source.includes("requestAnimationFrame")]).toEqual([file, false]);
      expect([file, source.includes("setInterval(() => (now = Date.now()), 30_000)")]).toEqual([
        file,
        file === "./HomeScreen.svelte",
      ]);
    }
  });

  it("the waiting chips open the project and focus the pane once, sending no key", () => {
    expect(strip).toMatch(/data-attention-chip=\{chip\.paneId\}/);
    expect(markup(strip)).toMatch(/onclick=\{\(\) => onselect\(chip\)\}/);
    expect(home).toMatch(/createChipDispatcher\(/);
    expect(home).toMatch(/openProject: \(projectId\) => ctx\.controllers\.projects\.open\(projectId\)/);
    expect(home).toMatch(/focusPane: \(paneId\) => ctx\.controllers\.agents\.openAttention\(paneId\)/);
    for (const file of ["./HomeScreen.svelte", "./ProjectCard.svelte", "./AttentionStrip.svelte", "./ServerCard.svelte"]) {
      const source = sources[file]!;
      for (const forbidden of ["sendPrompt", "pane_input", "sendInput", "controller.input", "terminal/"]) {
        expect([file, source.includes(forbidden)]).toEqual([file, false]);
      }
    }
  });
});

describe("AC-012-03 groups, servers and their interactions", () => {
  it("the group header collapses by click and summarizes the collapsed group", () => {
    expect(markup(home)).toMatch(/data-group-toggle=\{group\.id\}/);
    expect(markup(home)).toMatch(/<button[\s\S]*?aria-expanded=\{!isCollapsed\(group\)\}/);
    expect(home).toMatch(/collapsedGroupText\(group\.count, group\.agentSummary\)/);
    expect(markup(home)).toMatch(/data-collapsed-text/);
    expect(home).toMatch(/onclick=\{\(\) => toggleGroup\(group\)\}/);
    // A native button expands on Enter as well; the model owns the toggle.
    expect(home).toMatch(/toggledCollapse\(collapsed, group\.id, isCollapsed\(group\)\)/);
  });

  it("the Servers section lists a card per host and selecting one uses the surface action", () => {
    expect(markup(home)).toMatch(/<ServerCard\s+\{server\}\s+onselect=\{selectServer\}\s*\/>/);
    expect(home).toMatch(/import ServerCard from "\.\/ServerCard\.svelte"/);
    expect(markup(server)).toMatch(/data-server-card=\{server\.endpoint\}/);
    expect(markup(server)).toMatch(/data-state/);
    expect(server).toMatch(/\{server\.detail\}/);
    expect(markup(server)).toMatch(/onclick=\{\(\) => onselect\(server\.endpoint\)\}/);
    expect(home).toMatch(/ctx\.controllers\.surface\.select\(endpoint\)/);
    expect(markup(home)).toMatch(/data-add-project=\{group\.id\}/);
    expect(home).toMatch(/ctx\.controllers\.projects\.openForm\(group\.id\)/);
  });

  it("wraps more than six cards per group and keeps the design's radii", () => {
    // Would catch: a single row of cards (no wrap) or radii that ignore the guide.
    expect(home).toMatch(/grid-template-columns: repeat\(auto-fill, minmax\(280px, 1fr\)\)/);
    expect(home).toMatch(/border-radius: 10px/);
    expect(strip).toMatch(/border-radius: 10px/);
    expect(server).toMatch(/border-radius: 10px/);
    expect(card).toMatch(/border-radius: 10px/);
    expect(card).toMatch(/border-radius: 8px/);
  });
});
