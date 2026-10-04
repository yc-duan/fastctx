// The machines of the World: each one's system, whether it is online and
// since when, who is on it and what runs there. A machine named in the
// address is brought into view and marked.

import { escape } from "../conversation/prose";
import { osName } from "../home/frontispiece";
import type { WorldNow } from "../world/bridge";
import type { Node } from "../world/world";
import { talkLink } from "./places";
import type { Page } from "./shell";
import { effect } from "./signals";
import { jobsOf, residentsOf, type Store } from "./store";
import type { MachinesWords } from "./words";

export interface MachinesOptions {
  store: Store;
  words: MachinesWords;
  head(title: string, actions?: string): string;
  /** The machine to bring into view, when one was named. */
  focus: string | null;
  /** When, as the front page says it: a clock time today, a day before. */
  when(at: Date, now: Date): string;
}

const agentName = (agent: string) => (agent === "claude" ? "Claude" : agent === "codex" ? "Codex" : agent);

export class MachinesPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private list: HTMLElement;
  private drawn = "";
  private focused = false;
  private stop: () => void;

  constructor(private readonly options: MachinesOptions) {
    this.title = options.focus ?? options.words.title;
    this.element = document.createElement("div");
    this.element.className = "machines-page";
    this.element.innerHTML = `
      ${options.head(`<h1 class="page-title">${escape(options.words.title)}</h1>`)}
      <div class="machines-list scroller"></div>`;
    this.list = this.element.querySelector<HTMLElement>(".machines-list")!;
    this.stop = effect(() => this.draw(options.store.world.get(), options.store.clock.get()));
  }

  leave(): void {
    this.stop();
  }

  private draw(world: WorldNow | null, now: Date): void {
    if (!world) return;
    const html = `<ol class="machine-entries">${world.nodes.machines.map((node) => this.machine(node, world, now)).join("")}</ol>`;
    if (html !== this.drawn) {
      this.drawn = html;
      this.list.innerHTML = html;
    }
    if (!this.focused && this.options.focus) {
      this.focused = true;
      this.list.querySelector<HTMLElement>(`[data-machine="${CSS.escape(this.options.focus)}"]`)?.scrollIntoView({ block: "center" });
    }
  }

  private machine(node: Node, world: WorldNow, now: Date): string {
    const { words } = this.options;
    const facts = "Ok" in node.facts && node.facts.Ok ? node.facts.Ok : null;
    const since = node.since ? this.options.when(new Date(node.since), now) : null;
    const state = node.revoked
      ? words.revoked(node.revoked.by, node.revoked.reason)
      : node.online === true
        ? words.online(since ?? "")
        : node.online === false
          ? words.offline(since)
          : words.unknown;
    const residents = residentsOf(world).filter((r) => r.about.machine === node.name && (r.here || r.now?.waiting_for_user));
    const jobs = jobsOf(world).filter((job) => job.machine === node.name);
    const link = world.nodes.link;
    const route =
      node.name === world.here && link.kind === "online"
        ? words.via(link.route.kind === "card" ? link.route.name : link.route.kind === "proxy" ? link.route.proxy : link.route.kind)
        : "";
    const describe = facts
      ? [osName(facts.os), facts.arch, `fastctx ${facts.version}`, route].filter(Boolean).map(escape).join(`<span class="sep">·</span>`)
      : "Err" in node.facts && node.facts.Err
        ? escape(words.factsUnknown(node.facts.Err))
        : "";
    const kind = node.revoked ? "revoked" : node.online === true ? "online" : node.online === false ? "offline" : "unknown";
    const focus = this.options.focus === node.name ? " focused" : "";
    return `<li class="machine-entry ${kind}${focus}" data-machine="${escape(node.name)}">
      <div class="machine-line"><span class="dot" aria-hidden="true"></span><h2 class="machine-title">${escape(node.name)}</h2>${node.name === world.here ? `<span class="tag">${escape(words.here)}</span>` : ""}<span class="machine-state">${escape(state)}</span></div>
      ${describe ? `<p class="machine-facts">${describe}</p>` : ""}
      ${facts ? `<p class="machine-home"><span>${escape(words.home)}</span><code>${escape(facts.home)}</code></p>` : ""}
      ${
        residents.length
          ? `<ul class="machine-agents">${residents
              .map(
                (r) =>
                  `<li><a href="${talkLink({ resident: r.about.id, machine: r.about.machine })}"><span class="mark ${r.now?.waiting_for_user ? "mark-ask" : r.now?.activity === "busy" ? "mark-busy" : "mark-idle"}" aria-hidden="true"></span>${escape(r.about.origin.kind === "colleague" ? `${agentName(r.about.host.agent)} · ${r.about.origin.label}` : agentName(r.about.host.agent))}${r.town ? `<span class="tag">${escape(r.town)}</span>` : ""}</a></li>`,
              )
              .join("")}</ul>`
          : node.online === true
            ? `<p class="machine-quiet">${escape(words.nobody)}</p>`
            : ""
      }
      ${jobs.length ? `<p class="machine-jobs">${escape(words.jobs(jobs.length))}</p>` : ""}
    </li>`;
  }
}

