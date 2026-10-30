// What a page does, each a button that says what it will do: run one
// command on a machine and show what it printed, or say some words to an
// agent. The page must have declared each when it was published, and the
// user agreed when they first opened it; otherwise the World refuses and
// the element says why.
//
//   <w-run machine="lab" command="nvidia-smi --query-gpu=utilization.gpu --format=csv"
//          label="看一眼显卡"></w-run>
//   <w-say resident="codex-7f3a" machine="lab" text="重跑第 7 组，学习率减半"
//          label="让它重跑第 7 组"></w-say>

import { PageElement, escape, sheet, world } from "../element";
import type { Tools } from "../tools";
import { words } from "../words";

const acts = sheet(`
  button.act {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    min-height: 32px;
    padding: 4px var(--space-4);
    border-radius: var(--radius-full);
    background: var(--accent-wash);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 45%, transparent);
    color: var(--text-strong);
    font-size: var(--body-size);
    transition: background-color var(--hover) ease;
  }
  button.act:hover:not(:disabled) { background: color-mix(in srgb, var(--accent) 30%, transparent); }
  button.act:disabled { opacity: 0.6; cursor: default; }
  .what { margin-top: var(--space-1); font-size: 12px; color: var(--text-3); font-family: var(--font-code); }
  .said { margin-top: var(--space-2); font-size: var(--body-size); color: var(--text-3); }
  pre {
    margin: var(--space-2) 0 0;
    max-height: 280px;
    overflow: auto;
    padding: var(--space-3);
    border-radius: var(--radius-m);
    background: var(--bg);
    box-shadow: inset 0 0 0 1px var(--hairline);
    font-family: var(--font-code);
    font-size: var(--size-code);
    line-height: var(--leading-code);
    white-space: pre-wrap;
    word-break: break-word;
  }
`);

abstract class Act extends PageElement {
  protected static override styles = acts;
  protected busy = false;
  protected said = "";
  /** What the World said, for the pointer's rest. */
  protected why = "";
  protected wrong = false;
  protected output: string | null = null;

  constructor() {
    super();
    this.shadow.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("button.act") && !this.busy) void this.act();
    });
  }

  protected abstract act(): Promise<void>;

  protected settle(said: string, wrong = false, why = ""): void {
    this.busy = false;
    this.said = said;
    this.why = why;
    this.wrong = wrong;
    this.draw();
  }
}

export class Run extends Act {
  static observedAttributes = ["machine", "command", "label"];

  protected async act(): Promise<void> {
    const machine = this.getAttribute("machine") ?? "";
    const command = this.getAttribute("command") ?? "";
    this.busy = true;
    this.output = null;
    this.said = words().running;
    this.draw();
    try {
      const ran = await world().call<Tools["run"]["answer"]>("run", { machine, command });
      this.output = ran.output;
      this.settle(words().exit(ran.exit), ran.exit !== 0);
    } catch (error) {
      this.settle(words().notDone, true, error instanceof Error ? error.message : String(error));
    }
  }

  protected draw(): void {
    const machine = this.getAttribute("machine") ?? "";
    const command = this.getAttribute("command") ?? "";
    this.shadow.innerHTML = `
      <button class="act" type="button"${this.busy ? " disabled" : ""}>${escape(this.getAttribute("label") ?? words().run)}</button>
      <div class="what">${escape(machine)} $ ${escape(command)}</div>
      ${this.said ? `<div class="said${this.wrong ? " wrong" : ""}" role="status"${this.why ? ` title="${escape(this.why)}"` : ""}>${escape(this.said)}</div>` : ""}
      ${this.output !== null ? `<pre>${escape(this.output)}</pre>` : ""}`;
  }
}

export class Say extends Act {
  static observedAttributes = ["resident", "machine", "text", "label"];

  protected async act(): Promise<void> {
    this.busy = true;
    this.said = words().saying;
    this.draw();
    try {
      await world().call<Tools["say"]["answer"]>("say", {
        resident: this.getAttribute("resident") ?? "",
        machine: this.getAttribute("machine") ?? "",
        text: this.getAttribute("text") ?? "",
      });
      this.settle(words().delivered);
    } catch (error) {
      this.settle(words().notDone, true, error instanceof Error ? error.message : String(error));
    }
  }

  protected draw(): void {
    this.shadow.innerHTML = `
      <button class="act" type="button"${this.busy ? " disabled" : ""}>${escape(this.getAttribute("label") ?? words().say)}</button>
      <div class="what">→ ${escape(this.getAttribute("resident"))} ${escape(this.getAttribute("text"))}</div>
      ${this.said ? `<div class="said${this.wrong ? " wrong" : ""}" role="status"${this.why ? ` title="${escape(this.why)}"` : ""}>${escape(this.said)}</div>` : ""}`;
  }
}
