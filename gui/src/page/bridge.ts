// What a page holds of the World: one way to ask it things and ask it to do
// things, and the host's context (theme, language) as it is and as it
// changes. The page's elements and its own script use only this; how it
// travels (MCP Apps over postMessage, or a specimen's stand-in) is the
// runtime's business.

export type Theme = "warm-black" | "ink-black" | "warm-white";

export interface PageContext {
  theme: Theme;
  /** The interface's language: "zh-CN" or "en". */
  locale: string;
}

export interface Bridge {
  /** Calls one of the World's tools for the page; rejects with what the
   *  World or the host said when it was not done. */
  call<T = unknown>(tool: string, args?: Record<string, unknown>): Promise<T>;
  readonly context: PageContext;
  /** Hears the context change; returns how to stop hearing. */
  onContext(listener: (context: PageContext) => void): () => void;
}
