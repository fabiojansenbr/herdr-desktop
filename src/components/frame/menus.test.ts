// Spec 010 (AC-010-02) — menus Arquivo/Editar/Ver/Agentes/Janela: each enabled item calls exactly
// one existing action; what does not exist, or depends on an unavailable host, is disabled with a reason.
import { describe, expect, it } from "vitest";
import { buildMenus, MENU_ACTIONS, type MenuContext } from "./menus";

function context(over: Partial<MenuContext> = {}): { ctx: MenuContext; calls: string[] } {
  const calls: string[] = [];
  const actions = Object.fromEntries(MENU_ACTIONS.map((name) => [name, () => calls.push(name)])) as unknown as MenuContext["actions"];
  return {
    calls,
    ctx: { hostSelected: true, hostOnline: true, canInput: true, canRetry: false, caps: { split: true, create_tab: true, start_agent: true }, platform: "linux", actions, ...over },
  };
}

describe("menus", () => {
  // Would catch: menus reordered, renamed or a sixth menu.
  it("are Arquivo, Editar, Ver, Agentes, Janela in order", () => {
    expect(buildMenus(context().ctx).map((m) => m.label)).toEqual(["Arquivo", "Editar", "Ver", "Agentes", "Janela"]);
  });

  // Would catch: an item that does nothing (enabled without action), an action firing several
  // handlers, a disabled item without reason, or an item calling an action not in the app.
  it("every enabled item triggers exactly its one action; disabled ones carry a reason", () => {
    const { ctx, calls } = context({ canRetry: true });
    let enabled = 0;
    for (const menu of buildMenus(ctx)) {
      for (const item of menu.items) {
        if (item.run) {
          expect([item.id, item.reason]).toEqual([item.id, null]);
          expect(MENU_ACTIONS).toContain(item.action);
          calls.length = 0;
          item.run();
          expect([item.id, calls]).toEqual([item.id, [item.action]]);
          enabled++;
        } else {
          expect([item.id, (item.reason ?? "").length > 0]).toEqual([item.id, true]);
        }
      }
    }
    expect(enabled).toBe(MENU_ACTIONS.length);
  });

  // Would catch: host-dependent items left enabled on an offline host (the action would fail or
  // fall back to another host) or disabled without saying why.
  it("host-dependent items are disabled with the host reason when the host is unavailable", () => {
    const { ctx } = context({ hostOnline: false, canInput: false, canRetry: true });
    const items = buildMenus(ctx).flatMap((m) => m.items);
    for (const id of ["new-agent", "split", "new-tab", "paste"]) {
      const item = items.find((i) => i.id === id)!;
      expect([id, item.run, item.reason]).toEqual([id, null, "Indisponível: host desconectado"]);
    }
    expect(items.find((i) => i.id === "reconnect")!.run).not.toBeNull();
    const none = buildMenus(context({ hostSelected: false, hostOnline: false, canInput: false }).ctx).flatMap((m) => m.items);
    expect(none.find((i) => i.id === "split")!.reason).toBe("Indisponível: nenhum host selecionado");
    // Capability missing on a live host is its own reason.
    const legacy = buildMenus(context({ caps: { split: false, create_tab: true, start_agent: true } }).ctx).flatMap((m) => m.items);
    expect(legacy.find((i) => i.id === "split")!.reason).toBe("Indisponível: o servidor não oferece esta ação");
    expect(legacy.find((i) => i.id === "new-tab")!.run).not.toBeNull();
  });

  it("shows the palette shortcut of the platform", () => {
    const find = (platform: MenuContext["platform"]) => buildMenus(context({ platform }).ctx).flatMap((m) => m.items).find((i) => i.id === "search")!;
    expect(find("linux").shortcut).toBe("Ctrl K");
    expect(find("macos").shortcut).toBe("⌘ K");
  });
});
