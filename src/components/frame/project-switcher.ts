// Project switcher label and menu (spec 018, AC-018-01/03). Same collections as the 011 tree;
// without a bound project the control shows `sessão <nome>`, never a fabricated group.

import { t } from "../../i18n/index.svelte";
import type { HostDto } from "../../connections/types";
import type { CollectionDto, ProjectDto } from "../../projects/types";

/** Last item of the menu: the 016 native folder picker. Read per call, like every label. */
export function openProjectItem(): string {
  return t("frame.switcher.openProject");
}

export interface SwitcherProject {
  id: string;
  label: string;
}

export interface SwitcherGroup {
  id: string;
  name: string;
  projects: SwitcherProject[];
}

export function projectSwitcherLabel(input: {
  group?: string | null;
  project?: string | null;
  /** Spec 025: `<host> › <workspace> ⎇ <branch>` when the workspace belongs to a host. */
  host?: string | null;
  workspace?: string | null;
  branch: string | null;
  session: string | null;
}): string {
  const name = input.workspace?.trim() || input.project?.trim() || null;
  if (name) {
    const prefix = (input.host?.trim() || input.group?.trim()) ?? null;
    const branch = input.branch?.trim() && input.branch !== "—" ? input.branch.trim() : null;
    const left = prefix ? `${prefix} › ${name}` : name;
    return branch ? `${left} ⎇ ${branch}` : left;
  }
  return t("frame.switcher.session", { name: input.session?.trim() || "default" });
}

export interface SwitcherWorkspace {
  id: string;
  label: string;
  branch: string | null;
}

export interface SwitcherHost {
  endpoint: string;
  name: string;
  workspaces: SwitcherWorkspace[];
}

/** Spec 025: the selector lists the engine workspaces per host, in the tree's own order. */
export function workspaceSwitcherMenu(hosts: readonly HostDto[]): SwitcherHost[] {
  const local = hosts.filter((host) => host.kind === "local");
  const ssh = hosts.filter((host) => host.kind !== "local");
  return [...local, ...ssh].map((host) => ({
    endpoint: host.endpoint,
    name: host.kind === "local" ? t("frame.switcher.thisComputer") : host.label || host.endpoint,
    workspaces: [...(host.workspaces ?? [])]
      .sort((a, b) => a.number - b.number)
      .map((workspace) => ({
        id: workspace.workspace_id,
        label: workspace.label,
        branch: workspace.branch?.trim() || null,
      })),
  }));
}

/** Trailing-slash-insensitive root, used only to hide roots that are open (AC-025-05). */
function normalizedRoot(root: string): string {
  return root.replace(/[\\/]+$/, "") || root;
}

/**
 * Menu groups of the saved catalog. `liveRoots` (spec 025 AC-025-05) hides a saved project whose
 * cwd is an open workspace: it is listed as a live workspace of its host instead, so the menu
 * never offers a "closed" duplicate.
 */
export function projectSwitcherMenu(
  collections: readonly CollectionDto[],
  projects: readonly ProjectDto[],
  liveRoots: readonly string[] = [],
): SwitcherGroup[] {
  const byId = new Map(projects.map((p) => [p.id, p]));
  const live = new Set(liveRoots.map(normalizedRoot));
  return collections
    .map((collection) => ({
      id: collection.id,
      name: collection.name,
      projects: collection.project_ids.flatMap((id) => {
        const project = byId.get(id);
        if (!project || live.has(normalizedRoot(project.root))) return [];
        return [{ id: project.id, label: project.label }];
      }),
    }))
    .filter((group) => group.projects.length > 0);
}
