// The page's side of MCP Apps (protocol 2026-01-26, `modelcontextprotocol/
// ext-apps`): JSON-RPC over postMessage to the frame's parent, which is the
// host. The page says who it is with `ui/initialize`, hears the host's
// context back, says it is ready, and from then on calls the World's tools
// with `tools/call`. Messages from anything but the parent are not heard,
// as the official SDK does; a page written with that SDK speaks the same.
//
// A page may carry the SDK's own client beside this one. Both talk to the
// same host over the same window, so this one's request ids are strings of
// its own (`world-1`, …) that never meet the SDK's numbers.

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

type Message = {
  jsonrpc?: string;
  id?: string | number;
  method?: string;
  params?: unknown;
  result?: unknown;
  error?: { message?: string };
};

function contextOf(host: HostContext, before: PageContext): PageContext {
  const theme = host.world?.theme ?? (host.theme === "light" ? "warm-white" : host.theme === "dark" ? "warm-black" : before.theme);
  return { theme, locale: host.locale ?? before.locale };
}

/** A face the World hands its pages, as bytes. */
export interface Face {
  family: string;
  style: string;
  weight: string;
  unicodeRange?: string;
  data: ArrayBuffer;
}

/** Connects to the host, and asks it for the World's faces. */
export async function connect(name: string, version: string, start: PageContext, fonts: (faces: Face[]) => void): Promise<Bridge> {
  const host = window.parent;
  if (host === window) throw new Error("this page is not inside a host: open it in Fastctx");
  let serial = 0;
  const waiting = new Map<string, { resolve(value: unknown): void; reject(error: Error): void }>();
  const listeners = new Set<(context: PageContext) => void>();
  let context = start;
  const post = (message: object) => host.postMessage({ jsonrpc: "2.0", ...message }, "*");
  const request = <T>(method: string, params: object): Promise<T> =>
    new Promise((resolve, reject) => {
      serial += 1;
      const id = `world-${serial}`;
      waiting.set(id, { resolve: resolve as (value: unknown) => void, reject });
      post({ id, method, params });
    });
  const heard = (given: HostContext) => {
    context = contextOf(given, context);
  };

  // The host stops answering this frame once the page goes: whatever is
  // loaded in its place is not the page the user opened.
  window.addEventListener("pagehide", () => post({ method: "world/leaving", params: {} }));

  window.addEventListener("message", (event) => {
    if (event.source !== host) return;
    const message = event.data as Message;
    if (!message || message.jsonrpc !== "2.0") return;
    if (message.id !== undefined && !message.method) {
      const pending = typeof message.id === "string" ? waiting.get(message.id) : undefined;
      if (!pending) return;
      waiting.delete(message.id as string);
      if (message.error) pending.reject(new Error(message.error.message ?? "the host refused"));
      else pending.resolve(message.result);
      return;
    }
    switch (message.method) {
      case "ui/notifications/host-context-changed":
        heard((message.params ?? {}) as HostContext);
        for (const listener of listeners) listener(context);
        return;
      // The host asks whether the page is there, or is about to take it
      // down: either way the page answers, and has nothing to keep. A page
      // with the SDK's client answers too; the host takes the first.
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
  heard(initialized.hostContext ?? {});
  post({ method: "ui/notifications/initialized", params: {} });
  // A host that is not the World has no faces to give; the page reads in
  // the faces its system has.
  request<{ faces?: Face[] }>("world/fonts", {}).then((given) => fonts(given.faces ?? []), () => {});

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
