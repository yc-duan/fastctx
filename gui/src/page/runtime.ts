// What a page is given to run on: the World's design tokens, the elements,
// and the bridge, put into the page's own document. The host puts it first
// in the page's head as a plain script, so it runs before anything the page
// wrote: the page's own script, inline or not, finds the World as
// `window.world` from its first line, and the elements are defined before
// the parser meets them. Calls made before the host has answered wait for
// it. A bridge handed in before the runtime (a specimen's stand-in) is used
// instead of the host.

import tokens from "../design/tokens.css?raw";
import type { Bridge, PageContext } from "./bridge";
import { drawAll, useBridge } from "./element";
import { Chart } from "./elements/chart";
import { Card, Grid, Stat } from "./elements/layout";
import { Run, Say } from "./elements/acts";
import { Table } from "./elements/table";
import { Code, Markdown } from "./elements/text";
import { Agent, FileView, Job, Machine } from "./elements/world";
import { connect, type Face } from "./mcp";
import { themed } from "./themed";

declare global {
  interface Window {
    /** The World, for the page's own script. */
    world?: Bridge;
    /** A bridge handed in before the runtime loads, instead of the host. */
    __worldBridge?: Bridge;
  }
}

const ELEMENTS: [string, CustomElementConstructor][] = [
  ["w-grid", Grid],
  ["w-card", Card],
  ["w-stat", Stat],
  ["w-table", Table],
  ["w-chart", Chart],
  ["w-markdown", Markdown],
  ["w-code", Code],
  ["w-machine", Machine],
  ["w-agent", Agent],
  ["w-file", FileView],
  ["w-job", Job],
  ["w-run", Run],
  ["w-say", Say],
];

const PAGE = `
  html { background: var(--bg); color: var(--text); color-scheme: light dark; }
  body {
    margin: 0;
    padding: var(--space-5) var(--gutter);
    font-family: var(--font-reading);
    font-size: var(--body-size);
    line-height: var(--leading-prose);
    -webkit-font-smoothing: antialiased;
  }
  h1, h2, h3 { font-family: var(--font-emphasis); font-weight: 400; color: var(--text-strong); line-height: var(--leading-heading); }
  h1 { font-size: var(--size-title); margin: 0 0 var(--space-4); }
  a { color: var(--accent); }
  code { font-family: var(--font-code); font-size: var(--size-code); }
  /* Blocks of the page part by one space; things in a card by a smaller
     one. Inside a grid, the grid spaces them. */
  body > * + * { margin-top: var(--space-4); }
  w-card > * + * { margin-top: var(--space-3); }
`;

/** What the page is until the host says otherwise: what the host wrote on
 *  the document when it put the page together, or the World's defaults. */
function first(): PageContext {
  const root = document.documentElement;
  const theme = root.dataset.worldTheme;
  return {
    theme: theme === "ink-black" || theme === "warm-white" ? theme : "warm-black",
    locale: root.dataset.locale || "zh-CN",
  };
}

function style(id: string, css: string): void {
  let element = document.getElementById(id);
  if (!element) {
    element = document.createElement("style");
    element.id = id;
    (document.head ?? document.documentElement).prepend(element);
  }
  element.textContent = css;
}

/** The World before the host has answered: calls wait for the answer, the
 *  context is the first one until the host's arrives. */
function waiting(connecting: Promise<Bridge>, start: PageContext): Bridge {
  let context = start;
  const listeners = new Set<(context: PageContext) => void>();
  const changed = (next: PageContext) => {
    context = next;
    for (const listener of listeners) listener(next);
  };
  void connecting.then(
    (bridge) => {
      changed(bridge.context);
      bridge.onContext(changed);
    },
    () => {},
  );
  return {
    get context() {
      return context;
    },
    onContext(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    async call<T>(tool: string, args?: Record<string, unknown>): Promise<T> {
      return (await connecting).call<T>(tool, args);
    },
  };
}

/** The World's faces, made faces of this document. */
function install(faces: Face[]): void {
  for (const face of faces) {
    const made = new FontFace(face.family, face.data, {
      style: face.style,
      weight: face.weight,
      display: "swap",
      ...(face.unicodeRange ? { unicodeRange: face.unicodeRange } : {}),
    });
    document.fonts.add(made);
    void made.load().catch(() => {});
  }
}

export function start(): Bridge {
  style("world-page", `${themed(tokens)}\n${PAGE}`);
  const begun = first();
  const connecting = window.__worldBridge
    ? Promise.resolve(window.__worldBridge)
    : connect("world-page", "1", begun, install);
  const bridge = waiting(connecting, begun);
  let locale = begun.locale;
  bridge.onContext((context) => {
    // The interface's language is the elements', not the page's: what the
    // resident wrote keeps the language it was written in.
    document.documentElement.dataset.worldTheme = context.theme;
    document.documentElement.dataset.locale = context.locale;
    if (context.locale !== locale) {
      locale = context.locale;
      drawAll();
    }
  });
  useBridge(bridge);
  window.world = bridge;
  for (const [name, element] of ELEMENTS) if (!customElements.get(name)) customElements.define(name, element);
  connecting.catch((error: unknown) => {
    const said = document.createElement("p");
    said.style.color = "#c96a6a";
    said.textContent = navigator.language.startsWith("zh") ? "这个页面没能连上 Fastctx" : "This page couldn't reach Fastctx";
    said.title = error instanceof Error ? error.message : String(error);
    const shown = () => document.body.prepend(said);
    if (document.body) shown();
    else document.addEventListener("DOMContentLoaded", shown, { once: true });
  });
  return bridge;
}

start();
