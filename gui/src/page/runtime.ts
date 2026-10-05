// What a page is given to run on: the World's design tokens and faces, the
// elements, and the bridge, put into the page's own document. A page loads
// it as a module; it connects to its host by MCP Apps unless a bridge was
// handed to it first (a specimen's stand-in). The page's own script then
// finds the World as `window.world`.

import tokens from "../design/tokens.css?raw";
import type { Bridge, PageContext } from "./bridge";
import { useBridge } from "./element";
import { Chart } from "./elements/chart";
import { Card, Grid, Stat } from "./elements/layout";
import { Run, Say } from "./elements/acts";
import { Table } from "./elements/table";
import { Code, Markdown } from "./elements/text";
import { Agent, FileView, Job, Journal, Machine } from "./elements/world";
import { connect } from "./mcp";

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
  ["w-journal", Journal],
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

function apply(context: PageContext): void {
  document.documentElement.dataset.theme = context.theme;
  document.documentElement.lang = context.locale;
}

export async function start(): Promise<Bridge> {
  const style = document.createElement("style");
  style.textContent = `${tokens}\n${PAGE}`;
  document.head.prepend(style);
  const bridge = window.__worldBridge ?? (await connect("world-page", "1"));
  useBridge(bridge);
  apply(bridge.context);
  bridge.onContext(apply);
  window.world = bridge;
  for (const [name, element] of ELEMENTS) if (!customElements.get(name)) customElements.define(name, element);
  return bridge;
}

void start().catch((error: unknown) => {
  document.body.insertAdjacentHTML("afterbegin", `<p style="color:#c96a6a">${String(error instanceof Error ? error.message : error)}</p>`);
});
