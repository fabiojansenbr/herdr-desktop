// @vitest-environment happy-dom
// Spec 029 (AC-029-02/03) — CONEXÕES footer: failure states with the reason and Tentar novamente,
// the per-host menu (Desconectar/Reconectar/Editar…/Remover) and the inline remove confirmation.
// Local never offers Desconectar/Remover, and the chosen herdr binary travels in the tooltip.
import { mount, unmount, flushSync } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ConnectionItem } from "./tree-model";
import ConnectionsFooter from "./ConnectionsFooter.svelte";

const mounted: { el: HTMLElement; app: ReturnType<typeof mount> }[] = [];

function item(over: Partial<ConnectionItem> & { endpoint: string }): ConnectionItem {
  return {
    name: over.endpoint,
    typeBadge: "SSH",
    latencyText: "offline",
    statusText: "Offline",
    tone: "idle",
    active: false,
    online: false,
    retryable: false,
    tooltip: "Offline",
    ...over,
  };
}

const LOCAL = item({ endpoint: "local", name: "Este computador", typeBadge: "Local", online: true, tone: "ok", statusText: "Online", latencyText: "Local" });
const SSH_ONLINE = item({
  endpoint: "ssh-mac",
  name: "mac-mini",
  online: true,
  tone: "ok",
  statusText: "Online",
  latencyText: "42 ms",
  tooltip: "herdr 0.9.1 · /Users/ec2-user/.local/bin/herdr",
});
const SSH_FAILED = item({
  endpoint: "ssh-broken",
  name: "dev-box",
  tone: "attention",
  statusText: "Herdr não encontrado",
  latencyText: "atenção",
  retryable: true,
  tooltip: "Herdr não encontrado",
});

function render(items: ConnectionItem[], handlers: Record<string, (endpoint: string) => void> = {}) {
  const props = {
    items,
    onSelectHost: vi.fn(),
    onOpenDialog: vi.fn(),
    onConnect: vi.fn(),
    onDisconnect: vi.fn(),
    onReconnect: vi.fn(),
    onEdit: vi.fn(),
    onRemove: vi.fn(),
    ...handlers,
  };
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(ConnectionsFooter, { target: el, props });
  flushSync();
  mounted.push({ el, app });
  return { el, props };
}

function click(el: Element) {
  (el as HTMLElement).click();
  flushSync();
}

function openMenu(el: HTMLElement, endpoint: string) {
  click(el.querySelector(`[data-menu="${endpoint}"]`)!);
  return el.querySelector(`[data-menu-for="${endpoint}"]`);
}

afterEach(() => {
  for (const host of mounted.splice(0)) {
    unmount(host.app);
    host.el.remove();
  }
});

describe("ConnectionsFooter (AC-029-03)", () => {
  it("offers Desconectar/Editar…/Remover for an online SSH host", () => {
    const { el, props } = render([LOCAL, SSH_ONLINE]);
    const menu = openMenu(el, "ssh-mac")!;
    expect(menu).toBeTruthy();
    expect(menu.querySelector('[data-action="disconnect"]')?.textContent).toContain("Desconectar");
    expect(menu.querySelector('[data-action="edit"]')?.textContent).toContain("Editar");
    expect(menu.querySelector('[data-action="remove"]')?.textContent).toContain("Remover");
    expect(menu.querySelector('[data-action="reconnect"]')).toBeNull();

    click(menu.querySelector('[data-action="disconnect"]')!);
    expect(props.onDisconnect).toHaveBeenCalledExactlyOnceWith("ssh-mac");
    expect(props.onSelectHost).not.toHaveBeenCalled();
    expect(el.querySelector('[data-menu-for="ssh-mac"]')).toBeNull();
  });

  it("offers Reconectar (not Desconectar) for a failed host and Tentar novamente", () => {
    const { el, props } = render([LOCAL, SSH_FAILED]);
    const menu = openMenu(el, "ssh-broken")!;
    expect(menu.querySelector('[data-action="reconnect"]')?.textContent).toContain("Reconectar");
    expect(menu.querySelector('[data-action="disconnect"]')).toBeNull();
    click(menu.querySelector('[data-action="reconnect"]')!);
    expect(props.onReconnect).toHaveBeenCalledExactlyOnceWith("ssh-broken");

    click(el.querySelector('[data-retry="ssh-broken"]')!);
    expect(props.onConnect).toHaveBeenCalledExactlyOnceWith("ssh-broken");
    expect(el.textContent).toContain("Herdr não encontrado");
    // The failed host is never drawn as connected: no latency, no Online.
    expect(el.textContent).not.toContain("42 ms");
  });

  it("never offers Desconectar/Remover on Local and opens no menu for it", () => {
    const { el } = render([LOCAL, SSH_ONLINE]);
    expect(el.querySelector('[data-menu="local"]')).toBeNull();
    expect(el.textContent).not.toContain("Desconectar");
    const row = el.querySelector('[data-endpoint="local"]')!;
    row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    flushSync();
    expect(el.querySelector('[data-menu-for="local"]')).toBeNull();
    expect(el.textContent).not.toContain("Remover");
  });

  it("asks for inline confirmation before removing a saved profile", () => {
    const { el, props } = render([LOCAL, SSH_ONLINE]);
    const menu = openMenu(el, "ssh-mac")!;
    click(menu.querySelector('[data-action="remove"]')!);
    const confirm = el.querySelector('[data-confirm="ssh-mac"]');
    expect(confirm).toBeTruthy();
    expect(confirm!.textContent).toContain("Remover mac-mini?");
    expect(props.onRemove).not.toHaveBeenCalled();

    click(el.querySelector('[data-cancel-remove="ssh-mac"]')!);
    expect(props.onRemove).not.toHaveBeenCalled();
    expect(el.querySelector('[data-confirm="ssh-mac"]')).toBeNull();

    click(openMenu(el, "ssh-mac")!.querySelector('[data-action="remove"]')!);
    click(el.querySelector('[data-confirm-remove="ssh-mac"]')!);
    expect(props.onRemove).toHaveBeenCalledExactlyOnceWith("ssh-mac");
    expect(el.querySelector('[data-confirm="ssh-mac"]')).toBeNull();
  });

  it("opens the edit dialog from the menu and puts the chosen binary in the tooltip", () => {
    const { el, props } = render([LOCAL, SSH_ONLINE]);
    click(openMenu(el, "ssh-mac")!.querySelector('[data-action="edit"]')!);
    expect(props.onEdit).toHaveBeenCalledExactlyOnceWith("ssh-mac");
    const row = el.querySelector<HTMLElement>('[data-endpoint="ssh-mac"]')!;
    expect(row.getAttribute("title")).toBe("herdr 0.9.1 · /Users/ec2-user/.local/bin/herdr");
  });

  // Spec 032 (AC-032-01): the fixed overlay still closes on Escape and on a click outside it,
  // and a pointer down inside it (e.g. choosing an item) never closes it by itself.
  it("closes the host menu with Escape and with a click outside, but not inside", () => {
    const { el } = render([LOCAL, SSH_ONLINE]);
    openMenu(el, "ssh-mac");
    expect(el.querySelector('[data-menu-for="ssh-mac"]')).toBeTruthy();
    el.querySelector<HTMLElement>('[data-menu-for="ssh-mac"]')!.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    flushSync();
    expect(el.querySelector('[data-menu-for="ssh-mac"]')).toBeTruthy();

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    flushSync();
    expect(el.querySelector('[data-menu-for="ssh-mac"]')).toBeNull();

    openMenu(el, "ssh-mac");
    expect(el.querySelector('[data-menu-for="ssh-mac"]')).toBeTruthy();
    document.body.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    flushSync();
    expect(el.querySelector('[data-menu-for="ssh-mac"]')).toBeNull();
  });

  it("keeps a failed host selectable and shows its reason in the row", () => {
    const { el, props } = render([LOCAL, SSH_FAILED]);
    click(el.querySelector('[data-endpoint="ssh-broken"]')!);
    expect(props.onSelectHost).toHaveBeenCalledExactlyOnceWith("ssh-broken");
    expect(el.querySelector('[data-endpoint="ssh-broken"]')!.getAttribute("aria-label")).toContain("Herdr não encontrado");
  });
});
