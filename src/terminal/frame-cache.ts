// Spec 037 — Cache de último quadro por host no WebView (AC-037-01).
// Guarda o último quadro conhecido de cada host (grade de células, cursor e metadados de panes).
// Limitado a um quadro por host (chave: endpoint + geração + boot).
// Descartado quando a conexão cai ou a geometria muda.

import type { CellDto, CursorDto, PaneMeta } from "./types";

export interface CachedFrame {
  endpoint: string;
  boot_id: string;
  connection_generation: number;
  width: number;
  height: number;
  cells: CellDto[];
  cursor: CursorDto | null;
  panes: PaneMeta[];
  links: string[];
  revision: number;
}

export class HostFrameCache {
  private cache = new Map<string, CachedFrame>();

  get(endpoint: string, identity?: { boot_id?: string; connection_generation?: number }): CachedFrame | null {
    const entry = this.cache.get(endpoint);
    if (!entry) return null;
    if (identity) {
      if (identity.boot_id !== undefined && entry.boot_id !== identity.boot_id) {
        this.cache.delete(endpoint);
        return null;
      }
      if (identity.connection_generation !== undefined && entry.connection_generation !== identity.connection_generation) {
        this.cache.delete(endpoint);
        return null;
      }
    }
    return entry;
  }

  set(frame: CachedFrame): void {
    this.cache.set(frame.endpoint, frame);
  }

  drop(endpoint: string): void {
    this.cache.delete(endpoint);
  }

  clear(): void {
    this.cache.clear();
  }

  invalidateGeometry(width: number, height: number): void {
    for (const [ep, frame] of this.cache) {
      if (frame.width !== width || frame.height !== height) {
        this.cache.delete(ep);
      }
    }
  }

  has(endpoint: string): boolean {
    return this.cache.has(endpoint);
  }
}

export const defaultFrameCache = new HostFrameCache();
