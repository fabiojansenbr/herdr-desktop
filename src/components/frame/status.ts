// Status bar text (spec 010, AC-010-02): server version and connection state, branch, panes/tabs of
// the session and the release channel derived from the engine version. State is always text.
import { t } from "../../i18n/index.svelte";
import type { SurfacePhase } from "../../shell/controller";

export interface StatusInput {
  phase: SurfacePhase;
  hasEndpoint: boolean;
  serverVersion: string | null;
  /** Engine snapshot branch; null/undefined when the engine reports none. */
  branch: string | null | undefined;
  panes: number | null;
  tabs: number | null;
  /** Agents the engine listed for this session; null while the agents attachment is not live. */
  agents?: number | null;
  /** Working directory of the focused pane (AC-019-02); shown as-is. */
  cwd?: string | null;
}

export interface StatusModel {
  server: string;
  branch: string;
  counts: string | null;
  channel: string | null;
  agents: string | null;
  cwd: string | null;
}

/** Key of the phase's own word; `empty` and `disconnected` say the same thing. */
const STATE: Record<SurfacePhase, string> = {
  live: "frame.status.state.live",
  connecting: "frame.status.state.connecting",
  switching: "frame.status.state.switching",
  stale: "frame.status.state.stale",
  disconnected: "frame.status.state.disconnected",
  empty: "frame.status.state.disconnected",
};

export function statusBarModel(input: StatusInput): StatusModel {
  const version = input.serverVersion?.trim() || null;
  const server = input.hasEndpoint ? `herdr server${version ? ` ${version}` : ""} · ${t(STATE[input.phase])}` : t("frame.status.disconnected");
  const counts =
    input.hasEndpoint && input.panes !== null && input.tabs !== null
      ? `${t("frame.status.panes", { count: input.panes })} · ${t("frame.status.tabs", { count: input.tabs })}`
      : null;
  // Engine build_info: non-stable builds carry `-<channel>` after the base version.
  const channel = input.hasEndpoint && version ? t("frame.status.channel", { name: /-preview\b/.test(version) ? "preview" : "stable" }) : null;
  const agents =
    input.hasEndpoint && input.agents !== null && input.agents !== undefined
      ? t("frame.status.agents", { count: input.agents })
      : null;
  return {
    server,
    branch: input.hasEndpoint && input.branch ? input.branch : "—",
    counts,
    channel,
    agents,
    cwd: input.hasEndpoint && input.cwd ? input.cwd : null,
  };
}

