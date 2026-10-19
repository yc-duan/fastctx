// What every element of a page shares: it draws in its own shadow root, so a
// page's styles cannot unmake it and its styles cannot leak into the page;
// the design tokens reach it through inheritance, as custom properties do;
// it draws again when an attribute it watches changes.
//
// Data comes from a property the page's script sets, a JSON script inside
// the element, or the World: an element that names a tool asks for it
// through the bridge, again every so often where it says so, and reads the
// answer as it is told.
//
//   <w-chart tool="file.read" args='{"machine":"lab","path":"/runs/7/metrics.jsonl"}'
//            pick="text" parse="jsonl" every="30s" x="step" y="loss"></w-chart>

import type { Bridge } from "./bridge";

let bridge: Bridge | null = null;
/** The elements on the page, to draw again when the language changes. */
const living = new Set<PageElement>();

/** The World as the page holds it; set once when the runtime starts. */
export function world(): Bridge {
  if (!bridge) throw new Error("the page's runtime has not started");
  return bridge;
}

export function useBridge(given: Bridge): void {
  bridge = given;
}

/** Draws every element again, as when the page's language changed. */
export function drawAll(): void {
  for (const element of living) element.redraw();
}

const shared = sheet(`
  :host {
    display: block;
    box-sizing: border-box;
    min-width: 0;
    color: var(--text);
    font-family: var(--font-reading);
    font-size: var(--body-size);
    line-height: var(--leading-ui);
  }
  :host([hidden]) { display: none; }
  *, *::before, *::after { box-sizing: border-box; }
  .faint { color: var(--text-3); }
  .wrong { color: var(--wrong); }
  .label {
    font-size: var(--body-size);
    color: var(--text-3);
  }
  button {
    font: inherit;
    color: inherit;
    border: 0;
    background: none;
    cursor: pointer;
  }
  button:focus-visible {
    outline: 1px solid var(--focus);
    outline-offset: 2px;
  }
`);

export function sheet(css: string): CSSStyleSheet {
  const made = new CSSStyleSheet();
  made.replaceSync(css);
  return made;
}

/** The attributes that bind an element to a tool of the World. */
export const BOUND = ["tool", "args", "every", "pick", "parse"];

export abstract class PageElement extends HTMLElement {
  protected readonly shadow: ShadowRoot;
  /** This element's own styles, after the shared ones. */
  protected static styles: CSSStyleSheet | null = null;
  private dataGiven: unknown = undefined;
  /** What the World answered last for a bound element, or why it did not. */
  private fetched: { data: unknown } | { error: string } | null = null;
  private asking = 0;
  private timer = 0;
  /** The binding asked last, so an attribute that does not change it only
   *  draws again. */
  private bound = "";

  constructor() {
    super();
    this.shadow = this.attachShadow({ mode: "open" });
    const own = (this.constructor as typeof PageElement).styles;
    this.shadow.adoptedStyleSheets = own ? [shared, own] : [shared];
  }

  connectedCallback(): void {
    living.add(this);
    this.draw();
    this.ask();
  }

  disconnectedCallback(): void {
    living.delete(this);
    clearTimeout(this.timer);
    this.asking += 1;
  }

  redraw(): void {
    this.draw();
  }

  attributeChangedCallback(): void {
    if (!this.isConnected) return;
    if (JSON.stringify(this.binding()) !== this.bound) this.ask();
    else this.draw();
  }

  /** The tool the element reads and with what: those its attributes name,
   *  or, for an element that knows what it shows, its own. */
  protected binding(): { tool: string; args: Record<string, unknown> } | { error: string } | null {
    const tool = this.getAttribute("tool");
    if (!tool) return null;
    try {
      return { tool, args: this.hasAttribute("args") ? (JSON.parse(this.getAttribute("args")!) as Record<string, unknown>) : {} };
    } catch (error) {
      return { error: `args: ${(error as Error).message}` };
    }
  }

  /** Asks the World for the element's data, where it reads a tool, and
   *  again after `every` while it is on the page. */
  protected ask(): void {
    clearTimeout(this.timer);
    const binding = this.binding();
    this.bound = JSON.stringify(binding);
    const asked = ++this.asking;
    if (!binding) return;
    if ("error" in binding) {
      this.fetched = { error: binding.error };
      this.draw();
      return;
    }
    const { tool, args } = binding;
    world()
      .call(tool, args)
      .then(
        (answer) => {
          if (asked !== this.asking) return;
          try {
            this.fetched = { data: parsed(picked(answer, this.getAttribute("pick")), this.getAttribute("parse")) };
          } catch (error) {
            this.fetched = { error: (error as Error).message };
          }
        },
        (error: unknown) => {
          if (asked !== this.asking) return;
          this.fetched = { error: error instanceof Error ? error.message : String(error) };
        },
      )
      .finally(() => {
        if (asked !== this.asking) return;
        this.draw();
        const every = duration(this.getAttribute("every"));
        if (every !== null && this.isConnected) this.timer = window.setTimeout(() => this.ask(), every);
      });
  }

  /** Whether the element is waiting for the World's first answer. */
  protected get loading(): boolean {
    return this.binding() !== null && this.fetched === null;
  }

  /** Data given by the page's script. */
  set data(value: unknown) {
    this.dataGiven = value;
    if (this.isConnected) this.draw();
  }

  get data(): unknown {
    if (this.dataGiven !== undefined) return this.dataGiven;
    if (this.fetched) return "data" in this.fetched ? this.fetched.data : { error: this.fetched.error };
    // Or written into the page: a JSON script inside the element.
    const script = this.querySelector<HTMLScriptElement>('script[type="application/json"]');
    if (!script?.textContent) return undefined;
    try {
      return JSON.parse(script.textContent);
    } catch (error) {
      return { error: `JSON: ${(error as Error).message}` };
    }
  }

  protected abstract draw(): void;

  /** A number attribute, or `fallback` where it is not one. */
  protected number(name: string, fallback: number): number {
    const value = Number(this.getAttribute(name));
    return this.hasAttribute(name) && Number.isFinite(value) ? value : fallback;
  }
}

export function escape(text: unknown): string {
  return String(text ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** A part of an answer, by a dotted path: `result.rows`, `items.0.text`. */
function picked(answer: unknown, path: string | null): unknown {
  if (!path) return answer;
  let at: unknown = answer;
  for (const part of path.split(".")) {
    if (at === null || typeof at !== "object") throw new Error(`pick: nothing at ${path}`);
    at = (at as Record<string, unknown>)[part];
  }
  return at;
}

/** Text read as data: JSON, JSON lines, CSV with a header, or lines. */
function parsed(value: unknown, as: string | null): unknown {
  if (!as || typeof value !== "string") return value;
  switch (as) {
    case "json":
      return JSON.parse(value);
    case "jsonl":
      return value
        .split("\n")
        .filter((line) => line.trim())
        .map((line) => JSON.parse(line));
    case "csv": {
      const [head, ...rows] = value.split(/\r?\n/).filter((line) => line.trim());
      const keys = (head ?? "").split(",").map((key) => key.trim());
      return rows.map((row) =>
        Object.fromEntries(
          row.split(",").map((cell, index) => {
            const number = Number(cell);
            return [keys[index] ?? String(index), cell.trim() !== "" && Number.isFinite(number) ? number : cell.trim()];
          }),
        ),
      );
    }
    case "lines":
      return value.split(/\r?\n/);
    default:
      throw new Error(`parse: ${as} is not json, jsonl, csv or lines`);
  }
}

/** "10s", "2m", "1h" in milliseconds; null for none. */
function duration(text: string | null): number | null {
  const match = text?.trim().match(/^(\d+(?:\.\d+)?)\s*(ms|s|m|h)?$/);
  if (!match) return null;
  const unit = { ms: 1, s: 1000, m: 60_000, h: 3_600_000 }[(match[2] ?? "s") as "ms" | "s" | "m" | "h"];
  return Number(match[1]) * unit;
}
