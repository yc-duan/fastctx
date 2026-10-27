// Things of the World, live in a page: a machine and who is on it, an
// agent and what it is doing (pressing it opens its conversation), a file
// or its end, a job and its output. Each
// reads its tool through the bridge, again every so often where `every`
// says so.
//
//   <w-machine name="lab" every="30s"></w-machine>
//   <w-agent resident="codex-7f3a" machine="lab" every="10s"></w-agent>
//   <w-file machine="lab" path="/runs/7/train.log" tail="4000" every="10s"></w-file>
//   <w-job machine="lab" job="j-k3x9a2" every="10s"></w-job>

import { BOUND, PageElement, escape, sheet, world } from "../element";
import type { Tools } from "../tools";
import { words } from "../words";

type Now = Tools["world.now"]["answer"];

const live = sheet(`
  .row { display: flex; align-items: baseline; gap: var(--space-2); min-width: 0; }
  .name { color: var(--text-strong); }
  .mark { flex: none; width: 7px; height: 7px; border-radius: 50%; align-self: center; box-shadow: inset 0 0 0 1.5px var(--text-3); }
  .mark.on, .mark.working { background: var(--accent); box-shadow: none; }
  .mark.waiting { box-shadow: inset 0 0 0 2px var(--accent); }
  .mark.idle { box-shadow: inset 0 0 0 1.5px var(--text-2); }
  .mark.working { animation: pulse 1.6s ease-in-out infinite; }
  @keyframes pulse { 50% { opacity: 0.35; } }
  @media (prefers-reduced-motion: reduce) { .mark.working { animation: none; } }
  .sub { font-size: var(--body-size); color: var(--text-3); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .state.waiting { color: var(--accent); }
  ul { list-style: none; margin: var(--space-2) 0 0; padding: 0; }
  li + li { margin-top: var(--space-1); }
  pre {
    margin: 0;
    max-height: var(--height, 320px);
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
  .entry { padding: var(--space-2) 0; border-top: 1px solid var(--divider); }
  .entry:first-child { border-top: 0; }
  .entry-text { margin-top: 2px; font-size: var(--body-size); color: var(--text-2); display: -webkit-box; -webkit-line-clamp: 3; -webkit-box-orient: vertical; overflow: hidden; }
  button.agent { display: block; width: 100%; text-align: left; padding: 0; }
  button.agent:hover .name { text-decoration: underline; text-underline-offset: 3px; }
`);

/** What a failed or pending answer shows; null when there is data. */
function standing(loading: boolean, data: unknown): string | null {
  if (loading) return `<span class="faint">…</span>`;
  if (data && typeof data === "object" && "error" in data && typeof (data as { error: unknown }).error === "string") {
    return `<span class="wrong">${escape((data as { error: string }).error)}</span>`;
  }
  return null;
}

abstract class LiveElement extends PageElement {
  protected static override styles = live;
}

export class Machine extends LiveElement {
  static observedAttributes = ["name", ...BOUND];

  protected override binding() {
    return { tool: "world.now", args: {} };
  }

  protected draw(): void {
    const name = this.getAttribute("name") ?? "";
    const shown = standing(this.loading, this.data);
    if (shown) {
      this.shadow.innerHTML = `<div class="row"><span class="name">${escape(name)}</span>${shown}</div>`;
      return;
    }
    const now = this.data as Now;
    const machine = now.machines.find((m) => m.name === name);
    const w = words();
    if (!machine) {
      this.shadow.innerHTML = `<span class="wrong">${escape(w.notFound(name))}</span>`;
      return;
    }
    const agents = now.agents.filter((a) => a.machine === name);
    const busy = agents.filter((a) => a.state === "working").length;
    this.shadow.innerHTML = `
      <div class="row"><span class="mark ${machine.online ? "on" : ""}"></span><span class="name">${escape(name)}</span><span class="sub">${escape([machine.online ? w.online : w.offline, machine.device ? w.device : machine.system].filter(Boolean).join(" "))}</span></div>
      ${machine.device ? "" : `<div class="sub">${escape(w.agents(agents.length))}${busy ? ` ${escape(w.state.working)} ${busy}` : ""}</div>`}`;
  }
}

export class Agent extends LiveElement {
  static observedAttributes = ["resident", "machine", ...BOUND];

  constructor() {
    super();
    // Pressing it opens its conversation in the interface.
    this.shadow.addEventListener("click", (event) => {
      if (!(event.target as HTMLElement).closest("button.agent")) return;
      const resident = this.getAttribute("resident");
      const machine = this.getAttribute("machine");
      if (resident && machine) void world().call("open", { resident, machine }).catch(() => {});
    });
  }

  protected override binding() {
    return { tool: "world.now", args: {} };
  }

  protected draw(): void {
    const resident = this.getAttribute("resident") ?? "";
    const machine = this.getAttribute("machine");
    const shown = standing(this.loading, this.data);
    if (shown) {
      this.shadow.innerHTML = `<div class="row"><span class="name">${escape(resident)}</span>${shown}</div>`;
      return;
    }
    const now = this.data as Now;
    const agent = now.agents.find((a) => a.resident === resident && (!machine || a.machine === machine));
    const w = words();
    if (!agent) {
      this.shadow.innerHTML = `<span class="wrong">${escape(w.notFound(resident))}</span>`;
      return;
    }
    this.shadow.innerHTML = `<button class="agent" type="button">
      <div class="row"><span class="mark ${agent.state}"></span><span class="name">${escape(agent.title ?? `${agent.agent} ${agent.machine}`)}</span><span class="sub state ${agent.state}">${escape(w.state[agent.state])}</span></div>
      ${agent.last ? `<div class="sub">${escape(agent.last)}</div>` : ""}
    </button>`;
  }
}

export class FileView extends LiveElement {
  static observedAttributes = ["machine", "path", "tail", "height", ...BOUND];

  protected override binding() {
    const machine = this.getAttribute("machine");
    const path = this.getAttribute("path");
    if (!machine || !path) return null;
    return { tool: "file.read", args: this.hasAttribute("tail") ? { machine, path, tail: this.number("tail", 4096) } : { machine, path } };
  }

  protected draw(): void {
    const shown = standing(this.loading, this.data);
    const height = this.getAttribute("height");
    if (height) this.style.setProperty("--height", /^\d+$/.test(height) ? `${height}px` : height);
    if (shown) {
      this.shadow.innerHTML = shown;
      return;
    }
    const read = this.data as Tools["file.read"]["answer"] | undefined;
    if (!read) {
      this.shadow.innerHTML = "";
      return;
    }
    let text = read.text;
    if (text === null) {
      this.shadow.innerHTML = `<span class="faint">${escape(words().notText)}</span>`;
      return;
    }
    // The end of a file begins inside a line; that broken line is left out.
    if (read.offset > 0) text = text.slice(text.indexOf("\n") + 1);
    this.shadow.innerHTML = `<pre>${escape(text)}</pre>`;
    const pre = this.shadow.querySelector("pre")!;
    if (read.offset > 0) pre.scrollTop = pre.scrollHeight;
  }
}

export class Job extends LiveElement {
  static observedAttributes = ["machine", "job", "height", ...BOUND];

  protected override binding() {
    const machine = this.getAttribute("machine");
    const job = this.getAttribute("job");
    return machine && job ? { tool: "job.read", args: { machine, job } } : null;
  }

  protected draw(): void {
    const shown = standing(this.loading, this.data);
    if (shown) {
      this.shadow.innerHTML = shown;
      return;
    }
    const job = this.data as Tools["job.read"]["answer"] | undefined;
    if (!job) {
      this.shadow.innerHTML = "";
      return;
    }
    const w = words();
    this.shadow.innerHTML = `
      <div class="row"><span class="mark ${job.state === "running" ? "working" : ""}"></span><span class="name">${escape(job.job)}</span><span class="sub">${escape(w.jobState[job.state] ?? job.state)}${job.exit !== null ? ` ${escape(w.exit(job.exit))}` : ""}${job.signal != null ? ` ${escape(w.signal(job.signal))}` : ""}</span></div>
      <div class="sub" style="margin:2px 0 var(--space-2)"><code>${escape(job.command)}</code></div>
      ${job.output ? `<pre>${escape(job.output)}</pre>` : ""}`;
    const pre = this.shadow.querySelector("pre");
    if (pre) pre.scrollTop = pre.scrollHeight;
  }
}
