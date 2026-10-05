// The page's side of MCP Apps (protocol 2026-01-26, `modelcontextprotocol/
// ext-apps`): JSON-RPC over postMessage to the frame's parent, which is the
// host. The page says who it is with `ui/initialize`, hears the host's
// context back, says it is ready, and from then on calls the World's tools
// with `tools/call`. Messages from anything but the parent are not heard,
// as the official SDK does; a page written with that SDK speaks the same.

import type { Bridge, PageContext, Theme } from "./bridge";

const PROTOCOL = "2026-01-26";

interface HostContext {
  theme?: "light" | "dark";
  locale?: string;
  /** The World's own: which of its three themes. */
  world?: { theme?: Theme };
  [key: string]: unknown;
}

interface CallToolResult {
  content?: { type: string; text?: string }[];
  structuredContent?: unknown;
  isError?: boolean;
}

function contextOf(host: HostContext, before?: PageContext): PageContext {
  const theme = host.world?.theme ?? (host.theme === "light" ? "warm-white" : host.theme === "dark" ? "warm-black" : before?.theme ?? "warm-black");
  return { theme, locale: host.locale ?? before?.locale ?? "zh-CN" };
}

export async function connect(name: string, version: string): Promise<Bridge> {
  const host = window.parent;
  if (host === window) throw new Error("this page is not inside a host: open it in World");
  let serial = 0;
  const waiting = new Map<number, { resolve(value: unknown): void; reject(error: Error): void }>();
  const listeners = new Set<(context: PageContext) => void>();
  let context: PageContext = { theme: "warm-black", locale: "zh-CN" };
  const post = (message: object) => host.postMessage({ jsonrpc: "2.0", ...message }, "*");
  const request = <T>(method: string, params: object): Promise<T> =>
    new Promise((resolve, reject) => {
      serial += 1;
      waiting.set(serial, { resolve: resolve as (value: unknown) => void, reject });
      post({ id: serial, method, params });
    });

  window.addEventListener("message", (event) => {
    if (event.source !== host) return;
    const message = event.data as { jsonrpc?: string; id?: number; method?: string; params?: unknown; result?: unknown; error?: { message?: string } };
    if (!message || message.jsonrpc !== "2.0") return;
    if (message.id !== undefined && !message.method) {
      const pending = waiting.get(message.id);
      if (!pending) return;
      waiting.delete(message.id);
      if (message.error) pending.reject(new Error(message.error.message ?? "the host refused"));
      else pending.resolve(message.result);
      return;
    }
    switch (message.method) {
      case "ui/notifications/host-context-changed":
        context = contextOf((message.params ?? {}) as HostContext, context);
        for (const listener of listeners) listener(context);
        return;
      // The host asks whether the page is there, or is about to take it
      // down: either way the page answers, and has nothing to keep.
      case "ping":
      case "ui/resource-teardown":
        if (message.id !== undefined) post({ id: message.id, result: {} });
        return;
    }
  });

  const initialized = await request<{ hostContext?: HostContext }>("ui/initialize", {
    appInfo: { name, version },
    appCapabilities: {},
    protocolVersion: PROTOCOL,
  });
  context = contextOf(initialized.hostContext ?? {});
  post({ method: "ui/notifications/initialized", params: {} });

  return {
    get context() {
      return context;
    },
    onContext(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    async call<T>(tool: string, args: Record<string, unknown> = {}): Promise<T> {
      const result = await request<CallToolResult>("tools/call", { name: tool, arguments: args });
      if (result.isError) {
        throw new Error(result.content?.map((part) => part.text ?? "").join("\n") || `${tool} was refused`);
      }
      return result.structuredContent as T;
    },
  };
}
