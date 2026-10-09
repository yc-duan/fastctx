// The machines a login may use (`world-gui.md`, 账号、机器与授权): this
// computer first, with how it reaches each hub and whose machine it is
// there; then, account by account, each machine its account may use: online
// or not and through which relay, whose it is, what it was lent for, the
// agents its own programs say it has (installed or not, which version,
// logged in or not, and whether sessions the user opens there have the
// World's tools), who is on it and what runs there. A machine of the
// account's own is lent to other accounts, given its relays and removed
// here; one lent by another account is trusted here. On this computer the
// World's tools are put into its agents or taken out. A machine named in
// the address is brought into view and marked.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { osName } from "../home/frontispiece";
import type { AccountChange, GrantSeen, HubAccounts, Login, Rights } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { AgentFacts, WorldAgents } from "../world/agents";
import type { WorldNow } from "../world/bridge";
import type { LinkState } from "../world/world";
import { agentName, titleOf } from "../world/names";
import type { AccessWords, MachinesWords } from "./account-words";
import type { Confirm } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { GrantSheet, RelaysSheet } from "./machine-sheets";
import { link, talkLink } from "./places";
import { groupsOf, isComputer, loginLabel, machinesOf, owns, residentIn, rightsOn, type Group, type Machine } from "./reach";
import type { Page } from "./shell";
import { effect } from "./signals";
import { jobsOf, residentsOf, type Store } from "./store";

export interface MachinesOptions {
  store: Store;
  words: MachinesWords;
  access: AccessWords;
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
  consent?: { allow(machine: string): Promise<boolean>; withdraw(machine: string): Promise<boolean> } | undefined;
  /** Makes a change as a login. */
  change(login: Login, change: AccountChange): Promise<void>;
  /** Opens the sheet that lets another device into `login`. */
  invite(login: Login): void;
  confirm: Confirm;
}

/** What is known of the agents on one machine. */
type Asked = { state: "known"; facts: AgentFacts[] } | { state: "unknown"; why: string };

export class MachinesPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly list: HTMLElement;
  private drawn = "";
  private focused = false;
  private readonly stop: () => void;
  private asked = new Map<string, Asked>();
  private asking = false;
  private shape = "";
  private note: { machine: string; text: string } | null = null;
  private readonly granting: GrantSheet;
  private readonly relaying: RelaysSheet;

  constructor(private readonly options: MachinesOptions) {
    const { words } = options;
    this.granting = new GrantSheet(options.access);
    this.relaying = new RelaysSheet(options.access);
    this.title = options.focus ?? words.title;
    this.element = document.createElement("div");
    this.element.className = "machines-page";
    this.element.innerHTML = `
      ${options.head(
        `<h1 class="page-title">${escape(words.title)}</h1>`,
        `<button class="text-button add-device" type="button" hidden>${glyph("plus", 16)}<span>${escape(words.add)}</span></button>`,
      )}
      <div class="machines-list scroller"></div>`;
    this.list = this.element.querySelector<HTMLElement>(".machines-list")!;
    const add = this.element.querySelector<HTMLButtonElement>(".add-device")!;
    // Another device comes into an account: offered where there is one.
    effect(() => {
      add.hidden = !loginsOf(options.store.world.get()?.accounts).length;
    });
    add.addEventListener("click", () => {
      const [first] = loginsOf(options.store.world.peek()?.accounts);
      if (first) options.invite(first);
    });
    this.list.addEventListener("click", (event) => this.click(event));
    this.stop = effect(() => this.draw(options.store.world.get(), options.store.clock.get()));
  }

  leave(): void {
    this.stop();
  }

  private click(event: Event): void {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]");
    if (!button || button.disabled) return;
    const world = this.options.store.world.peek();
    const at = button.closest<HTMLElement>("[data-hub]");
    const login = loginsOf(world?.accounts).find((l) => l.hub === at?.dataset.hub && l.account === at?.dataset.account);
    const machine = button.closest<HTMLElement>("[data-machine]")?.dataset.machine ?? "";
    switch (button.dataset.act) {
      case "apply":
        void this.apply(button, machine, button.dataset.agent as "codex" | "claude", button.dataset.into === "1");
        return;
      case "consent":
        void this.consent(machine, button.dataset.allow === "1");
        return;
      case "grant":
        if (login) this.grant(login, machine);
        return;
      case "relays":
        if (login) this.relays(login, machine);
        return;
      case "remove":
        if (login) this.remove(login, machine);
        return;
      case "trust":
        if (login) this.trust(login, machine, button.dataset.owner ?? "");
        return;
      case "untrust":
        if (login) void this.act(button, login, { change: "trust", machine, trusted: false });
        return;
    }
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
      for (const machine of machinesOf(this.options.store.world.peek())) this.asked.set(machine.name, { state: "unknown", why: String(error) });
    }
    this.asking = false;
    this.redraw();
  }

  private hubOf(login: Login): HubAccounts | null {
    return this.options.store.world.peek()?.accounts.hubs.find((hub) => hub.hub === login.hub) ?? null;
  }

  /** The sheet that lends `machine` to other accounts. */
  private grant(login: Login, machine: string): void {
    const lent = () => (this.hubOf(login)?.grants ?? []).filter((grant) => grant.machine === machine);
    this.granting.open(machine, lent(), async (to, rights) => {
      await this.options.change(login, { change: "grant", machine, to, rights });
      await this.options.store.refresh();
      return lent();
    });
  }

  /** The sheet that sets the relays `machine` connects through. */
  private relays(login: Login, machine: string): void {
    const member = this.hubOf(login)?.members.find((member) => member.name === machine);
    this.relaying.open(
      machine,
      member?.relays ?? [],
      (relays) => this.options.change(login, { change: "relays", machine, relays }),
      () => void this.options.store.refresh(),
    );
  }

  private remove(login: Login, machine: string): void {
    const { access } = this.options;
    this.options.confirm.ask({
      title: access.removeTitle(machine),
      what: access.removeWhat(machine),
      go: access.removeGo,
      act: () => this.options.change(login, { change: "remove", name: machine, reason: access.reasonDefault }),
      done: () => void this.options.store.refresh(),
    });
  }

  private trust(login: Login, machine: string, owner: string): void {
    const { access } = this.options;
    this.options.confirm.ask({
      title: access.trustTitle(machine),
      what: access.trustWhat(owner),
      go: access.trustGo,
      gentle: true,
      act: () => this.options.change(login, { change: "trust", machine, trusted: true }),
      done: () => void this.options.store.refresh(),
    });
  }

  /** A change made at once, with nothing to read first. */
  private async act(button: HTMLButtonElement, login: Login, change: AccountChange): Promise<void> {
    const machine = button.closest<HTMLElement>("[data-machine]")?.dataset.machine ?? "";
    button.disabled = true;
    try {
      await this.options.change(login, change);
      this.note = null;
    } catch (error) {
      this.note = { machine, text: this.options.access.failed(error instanceof Error ? error.message : String(error)) };
    }
    await this.options.store.refresh();
    this.redraw();
  }

  private redraw(): void {
    this.drawn = "";
    this.draw(this.options.store.world.peek(), this.options.store.clock.peek());
  }

  /** The sheet says what it means and writes it; the machine is asked
   *  again so the line says what Claude Code now records. */
  private async consent(machine: string, allow: boolean): Promise<void> {
    const consent = this.options.consent;
    if (!consent || !machine) return;
    const done = await (allow ? consent.allow(machine) : consent.withdraw(machine));
    if (!done) return;
    this.asked.delete(machine);
    this.redraw();
    void this.ask();
  }

  private async apply(button: HTMLButtonElement, machine: string, agent: "codex" | "claude", into: boolean): Promise<void> {
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
    const { words } = this.options;
    const machines = machinesOf(world);
    const shape = machines.map((machine) => `${machine.written}:${machine.online}`).join(",");
    if (shape !== this.shape) {
      this.shape = shape;
      void this.ask();
    }
    const groups = groupsOf(world);
    const here = machines.find((machine) => machine.here);
    const parts: string[] = [];
    if (here) parts.push(`<ol class="machine-entries machine-here">${this.hereEntry(here, world, now)}</ol>`);
    if (!groups.length && isComputer(world)) {
      parts.push(`<p class="machines-alone">${escape(words.alone)} <a class="text-link" href="${link({ kind: "login" })}">${escape(words.logIn)}</a></p>`);
    }
    for (const group of groups) {
      const others = group.machines.filter((machine) => !machine.here);
      const heading =
        groups.length > 1 || !here
          ? `<h2 class="machines-group" data-hub="${escape(group.login.hub)}" data-account="${escape(group.login.account)}">${glyph("person", 16)}<span>${escape(words.group(loginLabel(group.login)))}</span></h2>`
          : "";
      const body = others.length
        ? `<ol class="machine-entries" data-hub="${escape(group.login.hub)}" data-account="${escape(group.login.account)}">${others.map((machine) => this.entry(machine, group, world, now)).join("")}</ol>`
        : `<p class="machines-none">${escape(here ? words.none : words.noneDevice)}</p>`;
      parts.push(`<section class="machines-section">${heading}${body}</section>`);
    }
    const html = parts.join("");
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

  private stateOf(machine: Machine, now: Date): { kind: string; said: string } {
    const { words } = this.options;
    const since = machine.since ? this.options.when(new Date(machine.since), now) : null;
    if (machine.here || machine.online === true) return { kind: "online", said: since ? words.online(since) : words.onlineNow };
    if (machine.online === false) return { kind: "offline", said: words.offline(since) };
    return { kind: "unknown", said: words.unknown };
  }

  /** The head of an entry: its name, its tags and whether it is online. */
  private line(machine: Machine, tags: string[], now: Date): string {
    const state = this.stateOf(machine, now);
    return `<div class="machine-line"><span class="dot" aria-hidden="true"></span><h2 class="machine-title">${escape(machine.written)}</h2>${tags.join("")}<span class="machine-state">${escape(state.said)}</span></div>`;
  }

  private facts(machine: Machine, extra: string[] = []): string {
    const { words } = this.options;
    const facts = "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
    const said = facts
      ? [osName(facts.os), facts.arch, `fastctx ${facts.version}`, ...extra].filter(Boolean).map(escape).join(`<span class="sep">·</span>`)
      : "Err" in machine.facts && machine.facts.Err && machine.online !== false
        ? escape(words.factsUnknown(machine.facts.Err))
        : extra.map(escape).join(`<span class="sep">·</span>`);
    return `${said ? `<p class="machine-facts">${said}</p>` : ""}${facts ? `<p class="machine-home"><span>${escape(words.home)}</span><code>${escape(facts.home)}</code></p>` : ""}`;
  }

  /** Who is on a machine and what runs there, as the login sees it. */
  private present(machine: Machine, world: WorldNow, group: Group | null): string {
    const { words } = this.options;
    const residents = residentsOf(world).filter(
      (r) => r.about.machine === machine.name && (r.here || r.now?.waiting_for_user) && (!group || residentIn(world, group, r)),
    );
    const jobs = jobsOf(world).filter((job) => job.machine === machine.name);
    const agents = residents.length
      ? `<ul class="machine-agents">${residents
          .map(
            (r) =>
              `<li><a href="${talkLink({ resident: r.about.id, machine: r.about.machine })}"><span class="mark ${r.now?.waiting_for_user ? "mark-ask" : r.now?.activity === "busy" ? "mark-busy" : "mark-idle"}" aria-hidden="true"></span>${escape([agentName(r.about.host.agent), titleOf(r)].filter(Boolean).join(" · "))}${r.town ? `<span class="tag">${escape(r.town)}</span>` : ""}</a></li>`,
          )
          .join("")}</ul>`
      : machine.here || machine.online === true
        ? `<p class="machine-quiet">${escape(words.nobody)}</p>`
        : "";
    return `${agents}${jobs.length ? `<p class="machine-jobs">${escape(words.jobs(jobs.length))}</p>` : ""}`;
  }

  /** This computer: how it reaches each hub and whose machine it is
   *  there, with what its owner sets on it. */
  private hereEntry(machine: Machine, world: WorldNow, now: Date): string {
    const { words } = this.options;
    const hubs = world.nodes.hubs.map((hub) => {
      const state = linkSaid(hub.link, words);
      const stand = hub.home ? words.registeredUnder(hub.home) : hub.logins.length ? words.onlyLogins(hub.logins.join(words.separator)) : "";
      const login = hub.home ? loginsOf(world.accounts).find((l) => l.hub === hub.hub && l.account === hub.home) : undefined;
      const lent = login ? this.lentLine(login, machine.name) : "";
      const acts = login
        ? `<span class="machine-hub-acts">${this.button("grant", words.grant)}${this.button("relays", words.relays)}</span>`
        : "";
      return `<li class="machine-hub ${hub.link.kind}" data-hub="${escape(hub.hub)}" data-account="${escape(hub.home ?? "")}">
        <span class="machine-hub-text"><span class="machine-hub-name">${escape(words.link(hub.name, state))}</span>${stand ? `<span class="machine-hub-stand">${escape(stand)}</span>` : ""}${lent ? `<span class="machine-hub-stand">${escape(lent)}</span>` : ""}</span>
        ${acts}
      </li>`;
    });
    const focus = this.options.focus === machine.name ? " focused" : "";
    return `<li class="machine-entry online${focus}" data-machine="${escape(machine.name)}">
      ${this.line(machine, [`<span class="tag">${escape(words.here)}</span>`], now)}
      ${this.facts(machine)}
      ${hubs.length ? `<ul class="machine-hubs">${hubs.join("")}</ul>` : ""}
      ${this.present(machine, world, null)}
      ${this.hosts(machine.name, true)}
    </li>`;
  }

  /** Whom `login` lent its `machine` to, in a sentence; nothing when none. */
  private lentLine(login: Login, machine: string): string {
    const { words } = this.options;
    const lent = (this.hubOf(login)?.grants ?? []).filter((grant: GrantSeen) => grant.machine === machine);
    return lent.length ? words.grantsOut(lent.map((grant) => words.lent(grant.account, rightsSaid(grant.rights, words))).join(words.separator)) : "";
  }

  private button(act: string, label: string, extra = ""): string {
    return `<button class="text-button quiet-button" type="button" data-act="${act}"${extra}>${escape(label)}</button>`;
  }

  /** One machine `group`'s account may use. */
  private entry(machine: Machine, group: Group, world: WorldNow, now: Date): string {
    const { words } = this.options;
    const login = group.login;
    const on = machine.on.find((on) => on.hub === login.hub);
    const mine = owns(machine, login);
    const trusted = (group.hub?.trust ?? []).some((trust) => trust.account === login.account && trust.machine === machine.name);
    const tags = [
      mine ? `<span class="tag tag-own">${escape(words.yours)}</span>` : on?.owner ? `<span class="tag">${escape(words.ownedBy(on.owner))}</span>` : "",
      trusted ? `<span class="tag tag-trust">${glyph("shield", 12)}${escape(words.trusted)}</span>` : "",
    ];
    const route = machine.online === true ? (machine.through ? words.via(machine.through) : words.direct) : "";
    const standing = mine ? this.lentLine(login, machine.name) : on?.owner ? words.granted(on.owner, rightsSaid(rightsOn(machine, login), words)) : "";
    const acts = mine
      ? [this.button("grant", words.grant), this.button("relays", words.relays), this.button("remove", words.remove, ` data-danger`)]
      : trusted
        ? [this.button("untrust", words.untrust)]
        : [this.button("trust", words.trust, ` data-owner="${escape(on?.owner ?? "")}"`)];
    const state = this.stateOf(machine, now);
    const focus = this.options.focus === machine.name ? " focused" : "";
    const note = this.note?.machine === machine.name ? `<p class="machine-note">${escape(this.note.text)}</p>` : "";
    return `<li class="machine-entry ${state.kind}${focus}" data-machine="${escape(machine.name)}">
      ${this.line(machine, tags, now)}
      ${this.facts(machine, [route])}
      ${standing ? `<p class="machine-standing">${escape(standing)}</p>` : ""}
      ${this.present(machine, world, group)}
      ${machine.online === true ? this.hosts(machine.name, false) : ""}
      <div class="machine-acts">${acts.join("")}</div>
      ${note}
    </li>`;
  }

  /** The agents on a machine as its own programs say them. */
  private hosts(machine: string, here: boolean): string {
    const { words } = this.options;
    const asked = this.asked.get(machine);
    const note = here && this.note?.machine === machine ? `<p class="machine-note">${escape(this.note.text)}</p>` : "";
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
        facts.login.kind === "known" ? (facts.login.value.kind === "logged_in" ? words.loggedIn : words.loggedOut) : words.loginUnknown;
      const integration = facts.integration.kind === "known" ? facts.integration.value : null;
      const tools =
        integration === "in" ? words.toolsIn : integration === "out" ? words.toolsOut : integration === "old" ? words.toolsOld : null;
      const act =
        here && integration && integration !== "absent"
          ? `<button class="text-button quiet-button" type="button" data-act="apply" data-agent="${agent}" data-into="${integration === "in" ? "0" : "1"}">${escape(integration === "in" ? words.takeOut : integration === "old" ? words.renew : words.putIn)}</button>`
          : "";
      const loggedOut = facts.login.kind === "known" && facts.login.value.kind === "logged_out";
      // Claude works for the World only once the user let it skip
      // approvals there; said, and changed, from any of the user's devices.
      const unattended = agent === "claude" && facts.unattended?.kind === "known" ? facts.unattended.value : null;
      const consent =
        unattended === null || !this.options.consent
          ? ""
          : `<span class="host-use"><span class="${unattended ? "" : "caution"}">${escape(unattended ? words.unattendedYes : words.unattendedNo)}</span><button class="text-button quiet-button" type="button" data-act="consent" data-allow="${unattended ? "0" : "1"}">${escape(unattended ? words.withdraw : words.allow)}</button></span>`;
      return `<li class="host-line"><span class="host-name">${escape(name)}${version ? `<span class="host-version">${escape(version)}</span>` : ""}</span><span class="host-fact${loggedOut ? " caution" : ""}">${escape(login)}</span>${tools || act ? `<span class="host-use">${tools ? `<span>${escape(tools)}</span>` : ""}${act}</span>` : ""}${consent}</li>`;
    });
    return `<ul class="machine-hosts" aria-label="${escape(words.hosts)}">${lines.join("")}</ul>${note}`;
  }
}

/** What an account may do on a machine, in words: "work on it and use its
 *  agents". */
export function rightsSaid(rights: Rights, words: Pick<MachinesWords, "hands" | "agentsRight" | "and">): string {
  return [rights.hands ? words.hands : "", rights.agents ? words.agentsRight : ""].filter(Boolean).join(words.and);
}

/** How this computer reaches a hub, in words. */
export function linkSaid(link: LinkState, words: MachinesWords): string {
  switch (link.kind) {
    case "online":
      return link.relay ? words.linkVia(link.relay, link.hub) : words.linkOnline(link.hub);
    case "connecting":
      return words.linkConnecting(link.last_error);
    case "stopped":
      return words.linkStopped(link.reason);
    case "alone":
      return "";
  }
}
