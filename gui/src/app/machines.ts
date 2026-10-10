// The machines a login may use (`world-gui.md`, 账号、机器、共享与借出),
// drawn: the account's World as a hand-drawn map (`network-map.ts`), each
// hub a knot and each machine a ring round it, and beside it what is known
// of the one pointed at. A machine: online or not and through which relay,
// its system, whose it is and why the account may use it, who is on it, and
// the way to its agents, which have a page of their own (`agents.ts`). A
// machine of the account's own is shared, given its relays and removed
// there; whom it is shared with is taken back with a press. A machine of
// another's an agent is lent on is drawn apart. A hub: how this computer
// reaches it, the account's level there and who its super administrator
// is. Long sayings stand behind a pointer's rest, not on the page.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { AccountChange, HubAccounts, Login } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { LinkState } from "../world/world";
import { agentName, osName, titleOf } from "../world/names";
import type { AccessWords, MachinesWords } from "./account-words";
import type { Confirm } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { GiveSheet, RelaysSheet } from "./machine-sheets";
import { mapOf } from "./map-of";
import { NetworkMap } from "./network-map";
import { link, talkLink } from "./places";
import { borrowedOf, groupsOf, isComputer, isSuper, machineActsFor, machinesOf, ownerOf, owns, residentIn, sameLogin, superOf, type Borrowed, type Machine } from "./reach";
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
  /** Makes a change as a login. */
  change(login: Login, change: AccountChange): Promise<void>;
  /** Opens the sheet that lets another device into `login`. */
  invite(login: Login): void;
  confirm: Confirm;
}

/** One thing on the map, as the page knows it. */
type Spot = { kind: "machine"; machine: Machine } | { kind: "lent"; lent: Borrowed[] } | { kind: "hub"; hub: HubAccounts };

export class MachinesPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly panel: HTMLElement;
  private readonly map: NetworkMap;
  private drawn = "";
  private readonly stop: () => void;
  /** The spot pointed at, by its key on the map. */
  private selected: string | null;
  private note: { machine: string; text: string } | null = null;
  private readonly giving: GiveSheet;
  private readonly relaying: RelaysSheet;

  constructor(private readonly options: MachinesOptions) {
    const { words } = options;
    this.giving = new GiveSheet(options.access);
    this.relaying = new RelaysSheet(options.access);
    this.title = options.focus ?? words.title;
    this.selected = options.focus ? `m:${options.focus}` : null;
    this.element = document.createElement("div");
    this.element.className = "machines-page";
    this.element.innerHTML = `
      ${options.head(
        `<h1 class="page-title">${escape(words.title)}</h1>`,
        `<button class="text-button add-device" type="button" hidden>${glyph("plus", 16)}<span>${escape(words.add)}</span></button>`,
      )}
      <div class="machines-layout"><aside class="machine-panel scroller" aria-live="polite"></aside></div>`;
    this.panel = this.element.querySelector<HTMLElement>(".machine-panel")!;
    this.map = new NetworkMap((key) => {
      this.selected = key;
      this.note = null;
      this.redraw();
      this.panel.scrollTop = 0;
      if (matchMedia("(max-width: 760px)").matches) this.panel.scrollIntoView({ block: "start", behavior: "smooth" });
    });
    this.panel.before(this.map.element);
    const add = this.element.querySelector<HTMLButtonElement>(".add-device")!;
    // Another device comes into an account: offered where there is one.
    effect(() => {
      add.hidden = !loginsOf(options.store.world.get()?.accounts).length;
    });
    add.addEventListener("click", () => {
      const [first] = loginsOf(options.store.world.peek()?.accounts);
      if (first) options.invite(first);
    });
    this.panel.addEventListener("click", (event) => this.click(event));
    this.stop = effect(() => this.draw(options.store.world.get(), options.store.clock.get()));
  }

  leave(): void {
    this.stop();
  }

  private loginOf(element: HTMLElement): Login | undefined {
    const at = element.closest<HTMLElement>("[data-hub]");
    return loginsOf(this.options.store.world.peek()?.accounts).find((l) => l.hub === at?.dataset.hub && l.account === at?.dataset.account);
  }

  private click(event: Event): void {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]");
    if (!button || button.disabled) return;
    const login = this.loginOf(button);
    const machine = button.closest<HTMLElement>("[data-machine]")?.dataset.machine ?? "";
    switch (button.dataset.act) {
      case "share":
        if (login) this.share(login, machine);
        return;
      case "unshare":
        if (login) void this.act(button, login, { change: "share", machine, to: button.dataset.to ?? "", shared: false });
        return;
      case "invite":
        if (login) this.options.invite(login);
        return;
      case "relays":
        if (login) this.relays(login, machine);
        return;
      case "remove":
        if (login) this.remove(login, machine);
        return;
    }
  }

  private hubOf(login: Login): HubAccounts | null {
    return this.options.store.world.peek()?.accounts.hubs.find((hub) => hub.hub === login.hub) ?? null;
  }

  /** The accounts `machine` is shared with, as `login`'s hub says. */
  private sharedWith(login: Login, machine: string): string[] {
    return (this.hubOf(login)?.shares ?? []).filter((share) => share.machine === machine).map((share) => share.account);
  }

  /** The sheet that shares `machine` with other accounts. */
  private share(login: Login, machine: string): void {
    const { access } = this.options;
    this.giving.open(
      { title: access.shareTitle(machine), what: access.shareWhat, sees: access.shareSees, given: access.sharedTo, go: access.shareGo },
      this.sharedWith(login, machine),
      async (to, shared) => {
        await this.options.change(login, { change: "share", machine, to, shared });
        await this.options.store.refresh();
        return this.sharedWith(login, machine);
      },
    );
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

  /** A change made at once, with nothing to read first: taking back a
   *  share or a loan, which can be given again as easily. */
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

  /** Everything on the map, by its key. */
  private spots(world: WorldNow): Map<string, Spot> {
    const spots = new Map<string, Spot>();
    for (const hub of world.accounts.hubs) if (hub.logins.length) spots.set(`hub:${hub.hub}`, { kind: "hub", hub });
    for (const machine of machinesOf(world)) spots.set(`m:${machine.written}`, { kind: "machine", machine });
    for (const lent of borrowedOf(world)) {
      const key = `lent:${lent.written}`;
      const known = spots.get(key);
      if (known?.kind === "lent") known.lent.push(lent);
      else spots.set(key, { kind: "lent", lent: [lent] });
    }
    return spots;
  }

  private draw(world: WorldNow | null, now: Date): void {
    if (!world) return;
    const { words } = this.options;
    const spots = this.spots(world);
    // A machine named in the address may be one an agent is lent on.
    if (this.selected?.startsWith("m:") && !spots.has(this.selected) && spots.has(`lent:${this.selected.slice(2)}`)) this.selected = `lent:${this.selected.slice(2)}`;
    if (!this.selected || !spots.has(this.selected)) {
      const here = machinesOf(world).find((machine) => machine.here);
      this.selected = here ? `m:${here.written}` : ([...spots.keys()].find((key) => key.startsWith("m:")) ?? [...spots.keys()][0] ?? null);
    }
    this.drawMap(world);
    const spot = this.selected ? spots.get(this.selected) : undefined;
    const alone =
      !loginsOf(world.accounts).length && isComputer(world)
        ? `<p class="machines-alone">${escape(words.alone)} <a class="text-link" href="${link({ kind: "login" })}">${escape(words.logIn)}</a></p>`
        : "";
    const html =
      alone +
      (spot ? (spot.kind === "hub" ? this.hubPanel(spot.hub, world) : spot.kind === "lent" ? this.lentPanel(spot.lent, world, now) : this.machinePanel(spot.machine, world, now)) : "");
    if (html !== this.drawn) {
      this.drawn = html;
      redrawKeepingFocus(this.panel, () => {
        this.panel.innerHTML = html;
      });
    }
  }

  /** The drawing: each hub the account is on, each machine it may work
   *  on, and each machine of another an agent is lent on. */
  private drawMap(world: WorldNow): void {
    const { hubs, nodes, hubLabels } = mapOf(world, this.options.words);
    this.map.update(hubs, nodes, this.selected, hubLabels);
  }

  private stateOf(online: boolean | null, since: string | null, here: boolean, now: Date): { kind: string; said: string } {
    const { words } = this.options;
    const at = since ? this.options.when(new Date(since), now) : null;
    if (here || online === true) return { kind: "online", said: at ? words.online(at) : words.onlineNow };
    if (online === false) return { kind: "offline", said: words.offline(at) };
    return { kind: "unknown", said: words.unknown };
  }

  /** The head of the panel: a name, its tags, and whether it is online. */
  private panelHead(name: string, tags: string[], state: { kind: string; said: string }, lent = false): string {
    return `<header class="panel-head ${state.kind}${lent ? " lent" : ""}"><span class="dot" aria-hidden="true"></span><h2 class="panel-title">${escape(name)}</h2>${tags.join("")}<span class="panel-state">${escape(state.said)}</span></header>`;
  }

  private tag(text: string, kind = "", title = ""): string {
    return text ? `<span class="tag${kind ? ` ${kind}` : ""}"${title ? ` title="${escape(title)}"` : ""}>${kind === "tag-reach" ? glyph("shield", 12) : ""}${escape(text)}</span>` : "";
  }

  /** A row of accounts something is given to, each taken back with its
   *  cross, and the button that gives it to another. */
  private givenRow(label: string, given: string[], take: (to: string) => string, add: string): string {
    const { words } = this.options;
    const chips = given
      .map((to) => `<span class="chip">${escape(to)}<button class="chip-x" type="button" ${take(to)} aria-label="${escape(words.takeBack(to))}" title="${escape(words.takeBack(to))}">${glyph("close", 12)}</button></span>`)
      .join("");
    return `<div class="given-row"><span class="given-label">${escape(label)}</span>${chips}${add}</div>`;
  }

  private button(act: string, label: string, extra = "", icon = ""): string {
    return `<button class="text-button quiet-button" type="button" data-act="${act}"${extra}>${icon}<span>${escape(label)}</span></button>`;
  }

  /** What is known of a machine the account may work on. */
  private machinePanel(machine: Machine, world: WorldNow, now: Date): string {
    const { words } = this.options;
    const logins = loginsOf(world.accounts);
    const owner = logins.find((login) => owns(machine, login)) ?? null;
    const mine = machine.here || !!owner;
    const ownerName = mine ? null : (logins.map((login) => ownerOf(machine, login)).find(Boolean) ?? null);
    const viewer = logins.find((login) => machine.on.some((on) => on.hub === login.hub)) ?? null;
    const shared = !!viewer && !mine && this.sharedWith(viewer, machine.name).includes(viewer.account);
    const bySuper = !!viewer && !shared && isSuper(world, viewer);
    const reaching = !mine && logins.some((login) => machineActsFor(world, machine.name, login));
    const tags = [
      machine.here ? this.tag(words.here) : "",
      owner && !machine.here ? this.tag(words.yours, "tag-own") : "",
      ownerName ? this.tag(bySuper ? words.superInShort(ownerName) : words.sharedInShort(ownerName), "", bySuper ? words.superIn(ownerName) : words.sharedIn(ownerName)) : "",
      reaching ? this.tag(words.reaching, "tag-reach", words.reachingWhat) : "",
    ];
    const facts = "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
    const route = machine.here ? "" : machine.online === true ? (machine.through ? words.via(machine.through) : words.direct) : "";
    const factLine = facts
      ? [osName(facts.os), facts.arch, `fastctx ${facts.version}`, route].filter(Boolean).map(escape).join(`<span class="sep">·</span>`)
      : "Err" in machine.facts && machine.facts.Err && machine.online !== false
        ? `<span title="${escape(machine.facts.Err)}">${escape(words.factsUnknown(machine.facts.Err))}</span>`
        : escape(route);
    // This computer, hub by hub: how it reaches each, and whose it is there.
    const hubs = machine.here
      ? world.nodes.hubs
          .map((hub) => {
            const stand = hub.home ? words.registeredUnder(hub.home) : hub.logins.length ? words.onlyLogins(hub.logins.join(words.separator)) : "";
            return `<li class="panel-hub ${hub.link.kind}"><span class="panel-hub-name">${escape(hub.name)}</span><span${linkWhy(hub.link) ? ` title="${escape(linkWhy(hub.link))}"` : ""}>${escape([linkSaid(hub.link, words), stand].filter(Boolean).join(" · "))}</span></li>`;
          })
          .join("")
      : "";
    // Its owner's own: whom it is shared with, and the rest it sets.
    const owners = machine.here ? logins.filter((login) => world.nodes.hubs.some((hub) => hub.hub === login.hub && hub.home === login.account)) : owner ? [owner] : [];
    const sharing = owners
      .map(
        (login) =>
          `<div class="panel-give" data-hub="${escape(login.hub)}" data-account="${escape(login.account)}">${this.givenRow(
            owners.length > 1 ? `${words.sharedTo} · ${login.hubName}` : words.sharedTo,
            this.sharedWith(login, machine.name),
            (to) => `data-act="unshare" data-to="${escape(to)}"`,
            this.button("share", words.share, "", glyph("plus", 13)),
          )}</div>`,
      )
      .join("");
    const acts = owners.length
      ? `<div class="panel-acts" data-hub="${escape(owners[0]!.hub)}" data-account="${escape(owners[0]!.account)}">${this.button("relays", words.relays, "", glyph("relay", 14))}${machine.here ? "" : this.button("remove", words.remove, " data-danger", glyph("close", 14))}</div>`
      : "";
    const note = this.note?.machine === machine.name ? `<p class="machine-note">${escape(this.note.text)}</p>` : "";
    const live = machine.here || machine.online === true;
    const state = this.stateOf(machine.online, machine.since, machine.here, now);
    return `<article class="panel-machine" data-machine="${escape(machine.name)}">
      ${this.panelHead(machine.written, tags, state)}
      ${factLine ? `<p class="panel-facts">${factLine}</p>` : ""}
      ${facts ? `<p class="panel-home"><span>${escape(words.home)}</span><code>${escape(facts.home)}</code></p>` : ""}
      ${hubs ? `<ul class="panel-hubs">${hubs}</ul>` : ""}
      ${sharing}
      ${this.present(machine.name, live, world, viewer)}
      ${this.agentsLink(machine.written)}
      ${note}
      ${acts}
    </article>`;
  }

  /** What is known of a machine of another's an agent is lent on: whose
   *  it is, that it is no machine of the account's to work on, and the
   *  lent logins, each a way into a conversation. */
  private lentPanel(lent: Borrowed[], world: WorldNow, now: Date): string {
    const { words } = this.options;
    const first = lent[0]!;
    const reaching = machineActsFor(world, first.machine, first.login);
    const tags = [this.tag(words.lentShort(first.owner ?? ""), "tag-lent", words.borrowedSaid(first.owner ?? "")), reaching ? this.tag(words.reaching, "tag-reach", words.reachingWhat) : ""];
    const state = this.stateOf(first.online, first.since, false, now);
    return `<article class="panel-machine" data-machine="${escape(first.machine)}">
      ${this.panelHead(first.written, tags, state, true)}
      <p class="panel-said" title="${escape(words.borrowedSaid(first.owner ?? ""))}">${glyph("colleague", 14)}<span>${escape(words.borrowedNote)}</span></p>
      ${this.present(first.machine, first.online === true, world, first.login)}
      ${this.agentsLink(first.written)}
    </article>`;
  }

  /** The way to the agents on a machine, which have a page of their own. */
  private agentsLink(machine: string): string {
    const { words } = this.options;
    return `<a class="text-button quiet-button panel-agents" href="${link({ kind: "agents", machine })}">${glyph("agents", 14)}<span>${escape(words.hosts)}</span></a>`;
  }

  /** What is known of a hub: how this computer reaches it, the account's
   *  level there, who its super administrator is, and its machines. */
  private hubPanel(hub: HubAccounts, world: WorldNow): string {
    const { words } = this.options;
    const logins = loginsOf(world.accounts).filter((login) => login.hub === hub.hub);
    const state = { kind: hub.link.kind === "online" ? "online" : "offline", said: linkSaid(hub.link, words) || words.hubOff };
    const rows = logins
      .map((login) => {
        const me = hub.accounts.find((account) => account.name === login.account);
        const admin = superOf(world, login);
        const top = me?.level === "super";
        return `<li class="panel-login" data-hub="${escape(login.hub)}" data-account="${escape(login.account)}">
          <span class="panel-login-name">${glyph("person", 14)}<span>${escape(login.account)}</span>${this.tag(top ? words.levelSuper : words.levelMember, top ? "tag-own" : "")}</span>
          ${!top && admin ? `<span class="panel-said" title="${escape(words.superNote(admin))}">${glyph("shield", 13)}<span>${escape(words.superShort(admin))}</span></span>` : ""}
          <span class="panel-login-acts">${this.button("invite", words.inviteDevice, "", glyph("plus", 13))}${top && !me?.locked ? `<a class="text-button quiet-button" href="${link({ kind: "admin" })}">${glyph("settings", 13)}<span>${escape(words.manageHub)}</span></a>` : ""}</span>
        </li>`;
      })
      .join("");
    const machines = hub.members.filter((member) => member.kind === "computer" && member.home).length;
    return `<article class="panel-hub-card">
      <header class="panel-head ${state.kind}"><span class="knot" aria-hidden="true"></span><h2 class="panel-title">${escape(words.hubTitle(hub.name))}</h2><span class="panel-state">${escape(state.said)}</span></header>
      <p class="panel-facts">${escape(words.hubMachines(machines))}</p>
      <ul class="panel-logins">${rows}</ul>
    </article>`;
  }

  /** Who is on a machine and what runs there, as `login` sees it. */
  private present(machine: string, live: boolean, world: WorldNow, login: Login | null): string {
    const { words } = this.options;
    const group = login ? (groupsOf(world).find((one) => sameLogin(one.login, login)) ?? null) : null;
    const residents = residentsOf(world).filter(
      (r) => r.about.machine === machine && (r.now?.activity === "busy" || r.now?.waiting_for_user) && (!group || residentIn(world, group, r)),
    );
    const jobs = jobsOf(world).filter((job) => job.machine === machine);
    const agents = residents.length
      ? `<ul class="machine-agents">${residents
          .map(
            (r) =>
              `<li><a href="${talkLink({ resident: r.about.id, machine: r.about.machine })}"><span class="mark ${r.now?.waiting_for_user ? "mark-ask" : r.now?.activity === "busy" ? "mark-busy" : "mark-idle"}" aria-hidden="true"></span>${escape([agentName(r.about.host.agent), titleOf(r)].filter(Boolean).join(" · "))}${r.town ? `<span class="tag">${escape(r.town)}</span>` : ""}</a></li>`,
          )
          .join("")}</ul>`
      : live
        ? `<p class="machine-quiet">${escape(words.nobody)}</p>`
        : "";
    return `${agents}${jobs.length ? `<p class="machine-jobs">${escape(words.jobs(jobs.length))}</p>` : ""}`;
  }

}

/** Why a hub is not reached, in the kernel's words, for the hover. */
export function linkWhy(link: LinkState): string {
  return link.kind === "connecting" ? (link.last_error ?? "") : link.kind === "stopped" ? link.reason : "";
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
