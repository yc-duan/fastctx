// The machines of the World: the hub they reach, each one's system, whether
// it is online and since when, where it stands (management rights, who
// brought it in, what it may use), where it hangs in the tree (the relay it
// connects through, and whether it relays itself), who is on it and what
// runs there, and the agents its own programs say it has: installed or not,
// which version, logged in or not, and whether the sessions the user opens
// there have the World's tools. On this machine those tools can be put in or
// taken out here; from a machine with management rights, what a member may
// use, where it connects and whether a machine relays are set here. A member
// that just came in with this machine's invitation is said first, with the
// one button that revokes it. A machine named in the address is brought into
// view and marked.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { redrawKeepingFocus } from "./focus";
import { osName } from "../home/frontispiece";
import type { AgentFacts, WorldAgents } from "../world/agents";
import type { WorldNow } from "../world/bridge";
import type { Node, Scope } from "../world/world";
import type { Devices } from "./devices";
import { GrantSheet } from "./grant";
import { standingLine } from "./scope";
import { placeLine, RelaySheet, RouteSheet, structure } from "./tree";
import { talkLink } from "./places";
import type { Page } from "./shell";
import { effect } from "./signals";
import { jobsOf, residentsOf, type Store } from "./store";
import type { DeviceWords, MachinesWords, ScopeWords, TreeWords } from "./words";
import { agentName, titleOf } from "../world/names";

export interface MachinesOptions {
  store: Store;
  words: MachinesWords;
  devices: Devices;
  deviceWords: DeviceWords;
  scopeWords: ScopeWords;
  treeWords: TreeWords;
  /** Whether this machine holds management rights. */
  manages(): boolean;
  /** Sets what a member may use, and whether it manages. */
  grant(member: string, scope: Scope, manage: boolean): Promise<void>;
  /** Sets the relays a member connects through, in order; none for
   *  straight to the hub. */
  route(member: string, via: string[]): Promise<void>;
  /** Makes a machine relay at `publicAt`, or stop when it is empty. */
  setRelay(machine: string, publicAt: string[], listen: string | null): Promise<void>;
  /** The user has been shown that a member came in. */
  seen(member: string): Promise<void>;
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
  /** Asks the user, and gives or withdraws Claude's consent to work
   *  without asking on a machine; true once it is done. */
  consent?: { allow(machine: string): Promise<boolean>; withdraw(machine: string): Promise<boolean> };
}

/** What is known of the agents on one machine. */
type Asked = { state: "known"; facts: AgentFacts[] } | { state: "unknown"; why: string };

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
  private readonly granting: GrantSheet;
  private readonly routing: RouteSheet;
  private readonly relaying: RelaySheet;

  constructor(private readonly options: MachinesOptions) {
    this.granting = new GrantSheet(options.words, options.scopeWords, options.deviceWords);
    this.routing = new RouteSheet(options.treeWords);
    this.relaying = new RelaySheet(options.treeWords);
    this.title = options.focus ?? options.words.title;
    this.element = document.createElement("div");
    this.element.className = "machines-page";
    this.element.innerHTML = `
      ${options.head(
        `<h1 class="page-title">${escape(options.words.title)}</h1>`,
        `<button class="text-button invite" type="button">${glyph("plus", 16)}<span>${escape(options.deviceWords.invite)}</span></button>`,
      )}
      <div class="machines-list scroller"></div>`;
    // Only a member that may invite is offered to.
    effect(() => {
      const world = options.store.world.get();
      const me = world?.nodes.machines.find((node) => node.name === world.here);
      const invites = !me?.device && (!me?.standing || me.standing.manage === true || me.standing.scope.invite);
      this.element.querySelector<HTMLElement>(".invite")!.hidden = !invites;
    });
    this.list = this.element.querySelector<HTMLElement>(".machines-list")!;
    this.element.querySelector(".invite")!.addEventListener("click", () => options.devices.invite());
    this.list.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>(
        "[data-revoke], [data-leave], [data-apply], [data-consent], [data-scope], [data-route], [data-relay], [data-seen], [data-not-me], [data-show]",
      );
      if (!button || button.disabled) return;
      if (button.dataset.show) {
        this.list.querySelector<HTMLElement>(`[data-machine="${CSS.escape(button.dataset.show)}"]`)?.scrollIntoView({ behavior: "smooth", block: "center" });
        return;
      }
      if (button.dataset.scope) this.scope(button.dataset.scope);
      else if (button.dataset.route) this.route(button.dataset.route);
      else if (button.dataset.relay) this.relay(button.dataset.relay);
      else if (button.dataset.seen) void this.seen(button, button.dataset.seen);
      else if (button.dataset.notMe) {
        const name = button.dataset.notMe;
        options.devices.revoke(name, options.words.notMeWhat(name), () => void options.seen(name).catch(() => {}));
      } else if (button.dataset.consent) void this.consent(button, button.dataset.consent === "allow");
      else if (button.dataset.apply) void this.apply(button, button.dataset.apply as "codex" | "claude", button.dataset.into === "1");
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

  /** The sheet that sets what `member` may use. */
  private scope(member: string): void {
    const world = this.options.store.world.peek();
    const node = world?.nodes.machines.find((n) => n.name === member);
    if (!world || !node?.standing) return;
    this.granting.open(
      member,
      node.standing,
      {
        machines: world.nodes.machines.filter((n) => !n.device && !n.revoked).map((n) => n.name),
        projects: world.towns.map((t) => t.town),
        invite: true,
      },
      (scope, manage) => this.options.grant(member, scope, manage),
      () => void this.options.store.refresh(),
    );
  }

  /** The sheet that sets where `member` connects. */
  private route(member: string): void {
    const world = this.options.store.world.peek();
    const node = world?.nodes.machines.find((n) => n.name === member);
    if (!world || !node) return;
    const relays = world.nodes.machines.filter((n) => !n.revoked && n.place?.relays_at?.length).map((n) => n.name);
    this.routing.open(
      node,
      relays,
      (via) => this.options.route(member, via),
      () => void this.options.store.refresh(),
    );
  }

  /** The sheet that sets whether `machine` relays. */
  private relay(machine: string): void {
    const world = this.options.store.world.peek();
    const node = world?.nodes.machines.find((n) => n.name === machine);
    if (!node) return;
    this.relaying.open(
      node,
      (publicAt, listen) => this.options.setRelay(machine, publicAt, listen),
      () => void this.options.store.refresh(),
    );
  }

  private async seen(button: HTMLButtonElement, member: string): Promise<void> {
    button.disabled = true;
    try {
      await this.options.seen(member);
    } finally {
      void this.options.store.refresh();
    }
  }

  private redraw(): void {
    this.drawn = "";
    this.draw(this.options.store.world.peek(), this.options.store.clock.peek());
  }

  /** The sheet says what it means and writes it; the machine is asked
   *  again so the line says what Claude Code now records. */
  private async consent(button: HTMLButtonElement, allow: boolean): Promise<void> {
    const machine = button.closest<HTMLElement>("[data-machine]")?.dataset.machine ?? "";
    const consent = this.options.consent;
    if (!consent || !machine) return;
    const done = await (allow ? consent.allow(machine) : consent.withdraw(machine));
    if (!done) return;
    this.asked.delete(machine);
    this.redraw();
    void this.ask();
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
    const html = `${this.top(world, now)}<ol class="machine-entries">${world.nodes.machines.map((node) => this.machine(node, world, now)).join("")}</ol>`;
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

  /** The hub every machine reaches, and the members that just came in with
   *  this machine's invitations. */
  private top(world: WorldNow, now: Date): string {
    const { words } = this.options;
    const link = world.nodes.link;
    const hub =
      link.kind === "online"
        ? `<p class="machines-hub">${escape(link.relay ? words.hubVia(link.relay, link.hub) : words.hub(link.hub))}</p>`
        : "";
    const joined = (world.nodes.joined ?? [])
      .map(
        (member) => `<li class="joined-line" data-joined="${escape(member.name)}">
          <span class="joined-text">${escape(words.joined(member.name, member.device === true, this.options.when(new Date(member.noticed_at), now)))}</span>
          <span class="joined-acts"><button class="text-button quiet-button" type="button" data-seen="${escape(member.name)}">${escape(words.gotIt)}</button><button class="text-button quiet-button danger" type="button" data-not-me="${escape(member.name)}">${escape(words.notMe)}</button></span>
        </li>`,
      )
      .join("");
    return `${hub}${joined ? `<ul class="joined-lines" aria-live="polite">${joined}</ul>` : ""}${structure(world.nodes.machines, world.here, this.options.treeWords)}`;
  }

  /** Where a member stands: its rights, who brought it in, what it may use. */
  private standing(node: Node): string {
    const { words, scopeWords } = this.options;
    const standing = node.standing;
    if (!standing || node.revoked) return "";
    const from = standing.invited_by
      ? standing.delegated
        ? words.branch(standing.invited_by)
        : words.invitedBy(standing.invited_by)
      : words.founder;
    const sealing = standing.sealing ? "" : `<p class="machine-standing caution">${escape(words.noSealing)}</p>`;
    const place = placeLine(node, this.options.treeWords);
    return `<p class="machine-standing">${standing.manage ? `<span class="tag tag-manage">${escape(words.manages)}</span>` : ""}<span>${escape(standingLine(standing, scopeWords))}</span><span class="sep">·</span><span class="standing-from">${escape(from)}</span></p>${place ? `<p class="machine-place">${escape(place)}</p>` : ""}${sealing}`;
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
    const way =
      link.kind === "online"
        ? link.route.kind === "card"
          ? link.route.name
          : link.route.kind === "proxy"
            ? link.route.proxy
            : link.route.kind
        : "";
    const route =
      node.name === world.here && link.kind === "online" ? (link.relay ? words.viaRelay(way, link.relay) : words.via(way)) : "";
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
      <div class="machine-line"><span class="dot" aria-hidden="true"></span><h2 class="machine-title">${escape(node.name)}</h2>${node.name === world.here ? `<span class="tag">${escape(words.here)}</span>` : ""}${node.device ? `<span class="tag">${escape(words.phone)}</span>` : ""}${node.place?.relays_at?.length && !node.revoked ? `<span class="tag tag-relay">${escape(words.relayTag)}</span>` : ""}<span class="machine-state">${escape(state)}</span></div>
      ${describe ? `<p class="machine-facts">${describe}</p>` : ""}
      ${this.standing(node)}
      ${facts ? `<p class="machine-home"><span>${escape(words.home)}</span><code>${escape(facts.home)}</code></p>` : ""}
      ${
        node.device
          ? ""
          : residents.length
          ? `<ul class="machine-agents">${residents
              .map(
                (r) =>
                  `<li><a href="${talkLink({ resident: r.about.id, machine: r.about.machine })}"><span class="mark ${r.now?.waiting_for_user ? "mark-ask" : r.now?.activity === "busy" ? "mark-busy" : "mark-idle"}" aria-hidden="true"></span>${escape([agentName(r.about.host.agent), titleOf(r)].filter(Boolean).join(" · "))}${r.town ? `<span class="tag">${escape(r.town)}</span>` : ""}</a></li>`,
              )
              .join("")}</ul>`
          : node.online === true
            ? `<p class="machine-quiet">${escape(words.nobody)}</p>`
            : ""
      }
      ${jobs.length ? `<p class="machine-jobs">${escape(words.jobs(jobs.length))}</p>` : ""}
      ${node.online === true && !node.revoked && !node.device ? this.hosts(node.name, node.name === world.here) : ""}
      ${node.revoked || !world.nodes.world ? "" : this.acts(node, world)}
    </li>`;
  }

  /** What this machine may do to a member: set its scope, where it
   *  connects and whether it relays, with management rights; revoke it,
   *  with them or when it is this machine's own branch; leave, when it is
   *  this machine. Nothing it may not do is offered. */
  private acts(node: Node, world: WorldNow): string {
    const words = this.options.deviceWords;
    const manages = this.options.manages();
    const button = (attribute: string, value: string, label: string) =>
      `<button class="text-button quiet-button" type="button" ${attribute}="${escape(value)}">${escape(label)}</button>`;
    const acts: string[] = [];
    if (manages && node.standing) {
      acts.push(button("data-scope", node.name, this.options.words.changeScope));
      acts.push(button("data-route", node.name, this.options.words.changeRoute));
      if (!node.device) acts.push(button("data-relay", node.name, this.options.words.relaySettings));
    }
    if (node.name === world.here) acts.push(button("data-leave", "1", words.leave));
    else if (manages || (node.standing?.delegated && node.standing.invited_by === world.here)) {
      acts.push(button("data-revoke", node.name, words.revoke));
    }
    return acts.length ? `<div class="machine-acts">${acts.join("")}</div>` : "";
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
      // Claude works for the World only once the user let it skip
      // approvals there; said, and changed, from any of the user's devices.
      const unattended = agent === "claude" && facts.unattended?.kind === "known" ? facts.unattended.value : null;
      const consent =
        unattended === null || !this.options.consent
          ? ""
          : `<span class="host-use"><span class="${unattended ? "" : "caution"}">${escape(unattended ? words.unattendedYes : words.unattendedNo)}</span><button class="text-button quiet-button" type="button" data-consent="${unattended ? "withdraw" : "allow"}">${escape(unattended ? words.withdraw : words.allow)}</button></span>`;
      return `<li class="host-line"><span class="host-name">${escape(name)}${version ? `<span class="host-version">${escape(version)}</span>` : ""}</span><span class="host-fact${loggedOut ? " caution" : ""}">${escape(login)}</span>${tools || act ? `<span class="host-use">${tools ? `<span>${escape(tools)}</span>` : ""}${act}</span>` : ""}${consent}</li>`;
    });
    return `<ul class="machine-hosts" aria-label="${escape(words.hosts)}">${lines.join("")}</ul>${note}`;
  }
}

