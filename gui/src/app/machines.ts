// The machines of the World: each one's system, whether it is online and
// since when, who is on it and what runs there, and the agents its own
// programs say it has: installed or not, which version, logged in or not,
// and whether the sessions the user opens there have the World's tools.
// On this machine those tools can be put in or taken out here. A machine
// named in the address is brought into view and marked.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { redrawKeepingFocus } from "./focus";
import { osName } from "../home/frontispiece";
import type { AgentFacts, WorldAgents } from "../world/agents";
import type { WorldNow } from "../world/bridge";
import type { Node } from "../world/world";
import type { Devices } from "./devices";
import { talkLink } from "./places";
import type { Page } from "./shell";
import { effect } from "./signals";
import { jobsOf, residentsOf, type Store } from "./store";
import type { DeviceWords, MachinesWords } from "./words";

export interface MachinesOptions {
  store: Store;
  words: MachinesWords;
  devices: Devices;
  deviceWords: DeviceWords;
  head(title: string, actions?: string): string;
  /** The machine to bring into view, when one was named. */
  focus: string | null;
  /** When, as the front page says it: a clock time today, a day before. */
  when(at: Date, now: Date): string;
  /** What every online machine's programs say of its agents. */
  agents(): Promise<WorldAgents>;
  /** Puts the World's tools into an agent's program on this machine, or
   *  takes them out; what it could not do, said. */
  applyHere(agent: "codex" | "claude", into: boolean): Promise<string[]>;
}

/** What is known of the agents on one machine. */
type Asked = { state: "known"; facts: AgentFacts[] } | { state: "unknown"; why: string };

const agentName = (agent: string) => (agent === "claude" ? "Claude" : agent === "codex" ? "Codex" : agent);

export class MachinesPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private list: HTMLElement;
  private drawn = "";
  private focused = false;
  private stop: () => void;
  private asked = new Map<string, Asked>();
  private asking = false;
  private shape = "";
  private note: { machine: string; text: string } | null = null;

  constructor(private readonly options: MachinesOptions) {
    this.title = options.focus ?? options.words.title;
    this.element = document.createElement("div");
    this.element.className = "machines-page";
    this.element.innerHTML = `
      ${options.head(
        `<h1 class="page-title">${escape(options.words.title)}</h1>`,
        `<button class="text-button invite" type="button">${glyph("plus", 16)}<span>${escape(options.deviceWords.invite)}</span></button>`,
      )}
      <div class="machines-list scroller"></div>`;
    this.list = this.element.querySelector<HTMLElement>(".machines-list")!;
    this.element.querySelector(".invite")!.addEventListener("click", () => options.devices.invite());
    this.list.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-revoke], [data-leave], [data-apply]");
      if (!button || button.disabled) return;
      if (button.dataset.apply) void this.apply(button, button.dataset.apply as "codex" | "claude", button.dataset.into === "1");
      else if (button.dataset.revoke) options.devices.revoke(button.dataset.revoke);
      else options.devices.leave();
    });
    this.stop = effect(() => this.draw(options.store.world.get(), options.store.clock.get()));
  }

  leave(): void {
    this.stop();
  }

  /** Asks every online machine about its agents, again whenever which
   *  machines are online changes. */
  private async ask(): Promise<void> {
    if (this.asking) return;
    this.asking = true;
    try {
      const answer = await this.options.agents();
      for (const machine of answer.machines) {
        if (machine.kind === "answered") this.asked.set(machine.reply.machine, { state: "known", facts: machine.reply.agents });
        else if (machine.kind === "no_answer") this.asked.set(machine.machine, { state: "unknown", why: machine.reason });
        else this.asked.delete(machine.machine);
      }
    } catch (error) {
      const world = this.options.store.world.peek();
      for (const node of world?.nodes.machines ?? []) this.asked.set(node.name, { state: "unknown", why: String(error) });
    }
    this.asking = false;
    this.redraw();
  }

  private redraw(): void {
    this.drawn = "";
    this.draw(this.options.store.world.peek(), this.options.store.clock.peek());
  }

  private async apply(button: HTMLButtonElement, agent: "codex" | "claude", into: boolean): Promise<void> {
    const machine = button.closest<HTMLElement>("[data-machine]")?.dataset.machine ?? "";
    button.disabled = true;
    try {
      const left = await this.options.applyHere(agent, into);
      this.note = { machine, text: left.length ? left.join("；") : this.options.words.applied };
    } catch (error) {
      this.note = { machine, text: this.options.words.notApplied(String(error)) };
    }
    this.asked.delete(machine);
    this.redraw();
    void this.ask();
  }

  private draw(world: WorldNow | null, now: Date): void {
    if (!world) return;
    const shape = world.nodes.machines.map((node) => `${node.name}:${node.online}`).join(",");
    if (shape !== this.shape) {
      this.shape = shape;
      void this.ask();
    }
    const html = `<ol class="machine-entries">${world.nodes.machines.map((node) => this.machine(node, world, now)).join("")}</ol>`;
    if (html !== this.drawn) {
      this.drawn = html;
      redrawKeepingFocus(this.list, () => {
        this.list.innerHTML = html;
      });
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
    // A phone is a member nothing runs on: it is named as one, not asked.
    const describe = node.device
      ? escape(words.device)
      : facts
        ? [osName(facts.os), facts.arch, `fastctx ${facts.version}`, route].filter(Boolean).map(escape).join(`<span class="sep">·</span>`)
        : "Err" in node.facts && node.facts.Err
          ? escape(words.factsUnknown(node.facts.Err))
          : "";
    const kind = node.revoked ? "revoked" : node.online === true ? "online" : node.online === false ? "offline" : "unknown";
    const focus = this.options.focus === node.name ? " focused" : "";
    return `<li class="machine-entry ${kind}${focus}" data-machine="${escape(node.name)}">
      <div class="machine-line"><span class="dot" aria-hidden="true"></span><h2 class="machine-title">${escape(node.name)}</h2>${node.name === world.here ? `<span class="tag">${escape(words.here)}</span>` : ""}${node.device ? `<span class="tag">${escape(words.phone)}</span>` : ""}<span class="machine-state">${escape(state)}</span></div>
      ${describe ? `<p class="machine-facts">${describe}</p>` : ""}
      ${facts ? `<p class="machine-home"><span>${escape(words.home)}</span><code>${escape(facts.home)}</code></p>` : ""}
      ${
        node.device
          ? ""
          : residents.length
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
      ${node.online === true && !node.revoked && !node.device ? this.hosts(node.name, node.name === world.here) : ""}
      ${
        node.revoked || !world.nodes.world
          ? ""
          : `<div class="machine-acts">${
              node.name === world.here
                ? `<button class="text-button quiet-button" type="button" data-leave="1">${escape(this.options.deviceWords.leave)}</button>`
                : `<button class="text-button quiet-button" type="button" data-revoke="${escape(node.name)}">${escape(this.options.deviceWords.revoke)}</button>`
            }</div>`
      }
    </li>`;
  }

  /** The agents on a machine as its own programs say them. */
  private hosts(machine: string, here: boolean): string {
    const { words } = this.options;
    const asked = this.asked.get(machine);
    const note = this.note?.machine === machine ? `<p class="machine-note">${escape(this.note.text)}</p>` : "";
    if (!asked) return `<p class="machine-hosts-asking">${escape(words.asking)}</p>`;
    if (asked.state === "unknown") return `<p class="machine-hosts-asking">${escape(words.notAsked(asked.why))}</p>`;
    const lines = (["codex", "claude"] as const).map((agent) => {
      const all = asked.facts.filter((f) => f.agent === agent);
      const facts = all.find((f) => f.program.kind === "known" && f.program.value?.exists) ?? all[0];
      const name = agent === "codex" ? "Codex" : "Claude Code";
      if (!facts || (facts.program.kind === "known" && (!facts.program.value || !facts.program.value.exists))) {
        return `<li class="host-line absent"><span class="host-name">${escape(name)}</span><span class="host-fact">${escape(words.notInstalled)}</span></li>`;
      }
      const version = facts.version.kind === "known" ? /\d+(?:\.\d+)+/.exec(facts.version.value)?.[0] ?? facts.version.value : null;
      const login =
        facts.login.kind === "known"
          ? facts.login.value.kind === "logged_in"
            ? words.loggedIn
            : words.loggedOut
          : words.loginUnknown;
      const integration = facts.integration.kind === "known" ? facts.integration.value : null;
      const tools =
        integration === "in" ? words.toolsIn : integration === "out" ? words.toolsOut : integration === "old" ? words.toolsOld : null;
      const act =
        here && integration && integration !== "absent"
          ? `<button class="text-button quiet-button" type="button" data-apply="${agent}" data-into="${integration === "in" ? "0" : "1"}">${escape(integration === "in" ? words.takeOut : integration === "old" ? words.renew : words.putIn)}</button>`
          : "";
      const loggedOut = facts.login.kind === "known" && facts.login.value.kind === "logged_out";
      return `<li class="host-line"><span class="host-name">${escape(name)}${version ? `<span class="host-version">${escape(version)}</span>` : ""}</span><span class="host-fact${loggedOut ? " caution" : ""}">${escape(login)}</span>${tools || act ? `<span class="host-use">${tools ? `<span>${escape(tools)}</span>` : ""}${act}</span>` : ""}</li>`;
    });
    return `<ul class="machine-hosts" aria-label="${escape(words.hosts)}">${lines.join("")}</ul>${note}`;
  }
}

