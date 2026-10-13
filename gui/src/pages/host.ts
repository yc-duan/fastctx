// The interface as the host of one page, speaking MCP Apps (protocol
// 2026-01-26, `modelcontextprotocol/ext-apps`) with the frame it sits in:
// it answers the page's `ui/initialize` with its context, hears it is
// ready, carries out the tools it calls, and tells it when the theme or
// language changes. It hears only that frame's window. What a tool does is
// the caller's (`tools.ts`); this is only the conversation.
//
// A page may carry the SDK's client beside the World's runtime, and both
// initialize: each gets its answer, matched by its own id. Beyond MCP Apps
// the World's runtime asks one thing of its own, `world/fonts`: the faces,
// as bytes (`faces.ts`).

import type { Theme } from "../page/bridge";
import { faces } from "./faces";
import type { PageFrame } from "./frame";

const PROTOCOL = "2026-01-26";

export interface HostContext {
  theme: Theme;
  locale: string;
  platform: "desktop" | "mobile";
}

export interface HostOptions {
  frame: PageFrame;
  version: string;
  context(): HostContext;
  /** Does what the page asked; what it resolves with is the answer. */
  call(tool: string, args: Record<string, unknown>): Promise<unknown>;
  /** The page has said it is ready. */
  ready?(): void;
  /** The frame no longer holds the page it was given: it navigated itself
   *  somewhere else, or loaded again. The host has stopped answering. */
  left(error?: string): void;
}

type Id = string | number;

interface Message {
  jsonrpc?: string;
  id?: Id;
  method?: string;
  params?: Record<string, unknown>;
}

/** MCP's standard variables, in the World's own terms: a page written for
 *  any host looks like the World here. They name the World's tokens, which
 *  the runtime puts in every page, so they follow the theme by themselves. */
const VARIABLES: Record<string, string> = {
  "--color-background-primary": "var(--bg)",
  "--color-background-secondary": "var(--bg-elevated)",
  "--color-background-tertiary": "var(--bg-sunken)",
  "--color-background-inverse": "var(--text-strong)",
  "--color-background-ghost": "transparent",
  "--color-background-info": "var(--accent-wash)",
  "--color-background-danger": "color-mix(in srgb, var(--wrong) 14%, transparent)",
  "--color-background-success": "color-mix(in srgb, var(--alive) 14%, transparent)",
  "--color-background-warning": "color-mix(in srgb, var(--caution) 14%, transparent)",
  "--color-background-disabled": "var(--bg-sunken)",
  "--color-text-primary": "var(--text)",
  "--color-text-secondary": "var(--text-2)",
  "--color-text-tertiary": "var(--text-3)",
  "--color-text-inverse": "var(--bg)",
  "--color-text-ghost": "var(--text-3)",
  "--color-text-info": "var(--accent)",
  "--color-text-danger": "var(--wrong)",
  "--color-text-success": "var(--alive)",
  "--color-text-warning": "var(--caution)",
  "--color-text-disabled": "var(--text-3)",
  "--color-border-primary": "var(--rule)",
  "--color-border-secondary": "var(--divider)",
  "--color-border-tertiary": "var(--hairline)",
  "--color-border-inverse": "var(--text)",
  "--color-border-ghost": "transparent",
  "--color-border-info": "var(--accent)",
  "--color-border-danger": "var(--wrong)",
  "--color-border-success": "var(--alive)",
  "--color-border-warning": "var(--caution)",
  "--color-border-disabled": "var(--hairline)",
  "--color-ring-primary": "var(--focus)",
  "--color-ring-secondary": "var(--rule)",
  "--color-ring-inverse": "var(--bg)",
  "--color-ring-info": "var(--accent)",
  "--color-ring-danger": "var(--wrong)",
  "--color-ring-success": "var(--alive)",
  "--color-ring-warning": "var(--caution)",
  "--font-sans": "var(--font-reading)",
  "--font-mono": "var(--font-code)",
  // The faces have no bold of their own: weights are families
  // (`fonts.css`), and asking for a number above 400 would fake one.
  "--font-weight-normal": "400",
  "--font-weight-medium": "400",
  "--font-weight-semibold": "400",
  "--font-weight-bold": "400",
  "--font-text-xs-size": "12px",
  "--font-text-sm-size": "var(--size-small)",
  "--font-text-md-size": "var(--body-size)",
  "--font-text-lg-size": "var(--size-h3)",
  "--font-heading-xs-size": "var(--body-size)",
  "--font-heading-sm-size": "var(--size-h3)",
  "--font-heading-md-size": "var(--size-h2)",
  "--font-heading-lg-size": "var(--size-h1)",
  "--font-heading-xl-size": "var(--size-title)",
  "--font-heading-2xl-size": "var(--size-title)",
  "--font-heading-3xl-size": "var(--size-title)",
  "--font-text-xs-line-height": "var(--leading-ui)",
  "--font-text-sm-line-height": "var(--leading-ui)",
  "--font-text-md-line-height": "var(--leading-prose)",
  "--font-text-lg-line-height": "var(--leading-prose)",
  "--font-heading-xs-line-height": "var(--leading-heading)",
  "--font-heading-sm-line-height": "var(--leading-heading)",
  "--font-heading-md-line-height": "var(--leading-heading)",
  "--font-heading-lg-line-height": "var(--leading-heading)",
  "--font-heading-xl-line-height": "var(--leading-heading)",
  "--font-heading-2xl-line-height": "var(--leading-heading)",
  "--font-heading-3xl-line-height": "var(--leading-heading)",
  "--border-radius-xs": "var(--radius-s)",
  "--border-radius-sm": "var(--radius-s)",
  "--border-radius-md": "var(--radius-m)",
  "--border-radius-lg": "var(--radius-l)",
  "--border-radius-xl": "var(--radius-l)",
  "--border-radius-full": "var(--radius-full)",
  "--border-width-regular": "1px",
  "--shadow-hairline": "0 0 0 1px var(--hairline)",
  "--shadow-sm": "0 1px 2px rgb(0 0 0 / 0.18)",
  "--shadow-md": "var(--shadow-pop)",
  "--shadow-lg": "var(--shadow-pop)",
};

function hostContext(context: HostContext): Record<string, unknown> {
  return {
    theme: context.theme === "warm-white" ? "light" : "dark",
    locale: context.locale,
    styles: { variables: VARIABLES },
    displayMode: "inline",
    availableDisplayModes: ["inline"],
    platform: context.platform,
    userAgent: "Fastctx",
    world: { theme: context.theme },
  };
}

export class PageHost {
  private initialized = false;
  private closed = false;

  constructor(private readonly options: HostOptions) {
    options.frame.listen(value => this.heard(value), error => this.leave(error));
  }

  /** Tells the page the theme or language changed. */
  contextChanged(): void {
    if (this.initialized) this.post({ method: "ui/notifications/host-context-changed", params: hostContext(this.options.context()) });
  }

  close(): void {
    this.closed = true;
    this.options.frame.close();
  }

  private leave(error?: string): void {
    if (this.closed) return;
    this.close();
    this.options.left(error);
  }

  private post(message: object): void {
    if (!this.closed) this.options.frame.post({ jsonrpc: "2.0", ...message });
  }

  private heard(value: unknown): void {
    if (this.closed) return;
    const message = value as Message;
    if (!message || message.jsonrpc !== "2.0" || typeof message.method !== "string") return;
    const { id, method } = message;
    const params = message.params ?? {};
    if (id === undefined) {
      if (method === "world/isolation-failed") {
        this.leave(String(params.message));
        return;
      }
      // The World's runtime says so as the page's document goes away,
      // before anything that replaces it can speak in the same frame.
      if (method === "world/leaving") {
        this.leave();
        return;
      }
      if (method === "ui/notifications/initialized") {
        this.initialized = true;
        this.options.ready?.();
      }
      // Size changes, logging and other notices need nothing: the page
      // fills the frame and scrolls in it.
      return;
    }
    void this.answer(method, params).then(
      (result) => this.post({ id, result }),
      (error: unknown) =>
        this.post({
          id,
          error: { code: error instanceof Unknown ? -32601 : -32000, message: error instanceof Error ? error.message : String(error) },
        }),
    );
  }

  private async answer(method: string, params: Record<string, unknown>): Promise<unknown> {
    switch (method) {
      case "ui/initialize":
        return {
          protocolVersion: PROTOCOL,
          hostInfo: { name: "Fastctx", version: this.options.version },
          hostCapabilities: { serverTools: {}, logging: {} },
          hostContext: hostContext(this.options.context()),
        };
      case "ping":
        return {};
      case "world/fonts":
        return { faces: await faces() };
      case "tools/call": {
        const name = String(params.name ?? "");
        const args = (params.arguments ?? {}) as Record<string, unknown>;
        try {
          const answer = await this.options.call(name, args);
          return { content: [{ type: "text", text: JSON.stringify(answer) }], structuredContent: answer };
        } catch (error) {
          // A tool that was refused or failed is the tool's answer, as MCP
          // has it, not a broken conversation.
          return { content: [{ type: "text", text: error instanceof Error ? error.message : String(error) }], isError: true };
        }
      }
      case "ui/open-link":
        throw new Error("Fastctx does not open links for a page: a page acts only through its tools");
      default:
        throw new Unknown(`Fastctx does not answer ${method}`);
    }
  }
}

class Unknown extends Error {}
