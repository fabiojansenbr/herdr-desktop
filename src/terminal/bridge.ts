// Thin IPC wrapper. The WebView only ever calls these named commands.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { FrameEvent, GeometryDto, InputDto, StatusDto } from "./types";

export interface TerminalBridge {
  status(): Promise<StatusDto>;
  connect(geometry: GeometryDto, onEvent: (event: FrameEvent) => void): Promise<StatusDto>;
  input(events: InputDto[]): Promise<void>;
  resize(geometry: GeometryDto): Promise<void>;
  focus(focused: boolean): Promise<void>;
  detach(): Promise<StatusDto>;
  startSession(): Promise<StatusDto>;
}

export function tauriBridge(): TerminalBridge {
  return {
    status: () => invoke<StatusDto>("terminal_status"),
    connect: (geometry, onEvent) => {
      const channel = new Channel<FrameEvent>();
      channel.onmessage = onEvent;
      return invoke<StatusDto>("terminal_connect", { geometry, onEvent: channel });
    },
    input: (events) => invoke<void>("terminal_input", { events }),
    resize: (geometry) => invoke<void>("terminal_resize", { geometry }),
    focus: (focused) => invoke<void>("terminal_focus", { focused }),
    detach: () => invoke<StatusDto>("terminal_detach"),
    startSession: () => invoke<StatusDto>("session_start"),
  };
}

export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}
