// The machines a login may use (`world-gui.md`, 账号、机器、共享与借出),
// drawn: the account's World as a hand-drawn map (`network-map.ts`), each
// hub a knot and each machine a ring round it, and beside it what is known
// of the one pointed at. A machine: online or not and through which relay,
// its system, whose it is and why the account may use it, who is on it,
// the agents its own programs say it has, once for each of their logins
// (who is logged in and on what plan, how much of each usage window is
// spent, whom it is lent to, whether sessions the user opens there have the
// World's tools). A machine of the account's own is shared, given its
// relays and removed there, and each login on it lent; whom it is shared
// with and lent to are taken back with a press. An agent lent to the
// account sits on a machine of another's, drawn apart, its lent logins each
// a way into a conversation. A hub: how this computer reaches it, the
// account's level there and who its super administrator is. Long sayings
// stand behind a pointer's rest, not on the page.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { AccountChange, HubAccounts, Login } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { AgentFacts, HomeChange, WorldAgents } from "../world/agents";
import type { WorldNow } from "../world/bridge";
import type { LinkState } from "../world/world";
import { agentName, osName, titleOf } from "../world/names";
import type { AccessWords, MachinesWords } from "./account-words";
import { accountOf, dirName, homeChoice, homeLabel, homesOf, installed, isDefault, limitsHtml, loggedOut, usableHome, versionOf, type HomeWords } from "./agent-homes";
import type { ConsentHome } from "./claude-consent";
import type { Confirm } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { GiveSheet, RelaysSheet } from "./machine-sheets";
import { mapOf } from "./map-of";
import { NetworkMap } from "./network-map";
import { link, talkLink } from "./places";
import { borrowedOf, groupsOf, isComputer, isSuper, loginKey, machineActsFor, machinesOf, ownerOf, owns, residentIn, sameHome, sameLogin, superOf, type Borrowed, type Machine } from "./reach";
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
  homeWords: HomeWords;
  /** Puts the World's tools into one login of an agent on this machine, its
   *  home's directory or null for the default one, or takes them out; what
   *  it could not do, said. */
  applyHere(agent: "codex" | "claude", into: boolean, home: string | null): Promise<string[]>;
  /** Asks the user, and gives or withdraws Claude's consent to work
   *  without asking in one of its homes on a machine; true once it is
   *  done. */
  consent?: { allow(machine: string, home: ConsentHome): Promise<boolean>; withdraw(machine: string, home: ConsentHome): Promise<boolean> } | undefined;
  /** Adds a login the World did not find on a machine, or stops listing
   *  one; the user's own machines only. */
  homes?: ((machine: string, change: HomeChange) => Promise<void>) | undefined;
  /** Makes a change as a login. */
  change(login: Login, change: AccountChange): Promise<void>;
  /** Opens the sheet that lets another device into `login`. */
  invite(login: Login): void;
  /** Opens a new conversation with one agent's login chosen, by its key. */
  talkWith(key: string): void;
  confirm: Confirm;
}

/** What is known of the agents on one machine. */
type Asked = { state: "known"; facts: AgentFacts[] } | { state: "unknown"; why: string };

/** One thing on the map, as the page knows it. */
type Spot = { kind: "machine"; machine: Machine } | { kind: "lent"; lent: Borrowed[] } | { kind: "hub"; hub: HubAccounts };

export class MachinesPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly panel: HTMLElement;
  private readonly map: NetworkMap;
  private drawn = "";
  private readonly stop: () => void;
  private asked = new Map<string, Asked>();
  private asking = false;
  private shape = "";
  /** The spot pointed at, by its key on the map. */
  private selected: string | null;
  private note: { machine: string; text: string } | null = null;
  /** The agent whose login is being added on a machine, and the directory
   *  typed so far. */
  private adding: { machine: string; agent: string; dir: string } | null = null;
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
      this.adding = null;
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
    this.panel.addEventListener("input", (event) => {
      const input = event.target as HTMLInputElement;
      if (input.classList.contains("home-add-dir") && this.adding) this.adding.dir = input.value;
    });
    this.panel.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.addHome();
    });
    this.panel.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && (event.target as HTMLElement).classList.contains("home-add-dir")) {
        this.adding = null;
        this.redraw();
      }
    });
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
    const world = this.options.store.world.peek();
    const login = this.loginOf(button);
    const machine = button.closest<HTMLElement>("[data-machine]")?.dataset.machine ?? "";
    switch (button.dataset.act) {
      case "apply":
        void this.apply(button, machine, button.dataset.agent as "codex" | "claude", button.dataset.into === "1", button.dataset.home || null);
        return;
      case "consent":
        void this.consent(machine, button.dataset.allow === "1", { dir: button.dataset.home || null, label: button.dataset.label || null });
        return;
      case "add-home":
        this.adding = { machine, agent: button.dataset.agent ?? "", dir: "" };
        this.redraw();
        this.panel.querySelector<HTMLInputElement>(".home-add-dir")?.focus();
        return;
      case "add-cancel":
        this.adding = null;
        this.redraw();
        return;
      case "hide-home":
        void this.hideHome(button, machine, button.dataset.agent ?? "", button.dataset.dir ?? "");
        return;
      case "share":
        if (login) this.share(login, machine);
        return;
      case "unshare":
        if (login) void this.act(button, login, { change: "share", machine, to: button.dataset.to ?? "", shared: false });
        return;
      case "lend":
      case "unlend": {
        const lender = loginsOf(world?.accounts).find((l) => l.hub === button.dataset.lenderHub && l.account === button.dataset.lender);
        const agent = button.dataset.agent ?? "";
        const home = button.dataset.home || null;
        if (!lender) return;
        if (button.dataset.act === "lend") this.lend(lender, machine, agent, home, button.dataset.label ?? "");
        else void this.act(button, lender, { change: "lend", machine, to: button.dataset.to ?? "", agent, ...(home ? { home } : {}), lent: false });
        return;
      }
      case "talk":
        this.options.talkWith(button.dataset.key ?? "");
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

  /** The accounts `machine` is shared with, as `login`'s hub says. */
  private sharedWith(login: Login, machine: string): string[] {
    return (this.hubOf(login)?.shares ?? []).filter((share) => share.machine === machine).map((share) => share.account);
  }

  /** The accounts one login of `agent` on `login`'s `machine` is lent to. */
  private lentTo(login: Login, machine: string, agent: string, home: string | null): string[] {
    return (this.hubOf(login)?.lends ?? []).filter((lend) => lend.machine === machine && lend.agent === agent && sameHome(lend.home, home)).map((lend) => lend.account);
  }

  /** The sheet that shares `machine` with other accounts. */
  private share(login: Login, machine: string): void {
    const { access } = this.options;
    this.giving.open(
      { title: access.shareTitle(machine), what: access.shareWhat, sees: access.shareSees, given: access.sharedTo, go: access.shareGo, after: access.shareAfter },
      this.sharedWith(login, machine),
      async (to, shared) => {
        await this.options.change(login, { change: "share", machine, to, shared });
        await this.options.store.refresh();
        return this.sharedWith(login, machine);
      },
    );
  }

  /** The sheet that lends one login of `agent` on `machine` to other
   *  accounts; `home` is its directory, or null for the default one. */
  private lend(login: Login, machine: string, agent: string, home: string | null, label: string): void {
    const { access } = this.options;
    this.giving.open(
      { title: access.lendTitle(label, machine), what: access.lendWhat, sees: access.lendSees, given: access.lentTo, go: access.lendGo },
      this.lentTo(login, machine, agent, home),
      async (to, lent) => {
        await this.options.change(login, { change: "lend", machine, to, agent, ...(home ? { home } : {}), lent });
        await this.options.store.refresh();
        return this.lentTo(login, machine, agent, home);
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

  /** The sheet says what it means and writes it; the machine is asked
   *  again so the line says what Claude Code now records. */
  private async consent(machine: string, allow: boolean, home: ConsentHome): Promise<void> {
    const consent = this.options.consent;
    if (!consent || !machine) return;
    const done = await (allow ? consent.allow(machine, home) : consent.withdraw(machine, home));
    if (!done) return;
    this.asked.delete(machine);
    this.redraw();
    void this.ask();
  }

  private async apply(button: HTMLButtonElement, machine: string, agent: "codex" | "claude", into: boolean, home: string | null): Promise<void> {
    button.disabled = true;
    try {
      const left = await this.options.applyHere(agent, into, home);
      this.note = { machine, text: left.length ? left.join("；") : this.options.words.applied };
    } catch (error) {
      this.note = { machine, text: this.options.words.notApplied(String(error)) };
    }
    this.asked.delete(machine);
    this.redraw();
    void this.ask();
  }

  /** Adds the directory typed as a login of the agent; the machine says
   *  whether it is one, and is asked again so it is listed. */
  private async addHome(): Promise<void> {
    const adding = this.adding;
    const homes = this.options.homes;
    const dir = adding?.dir.trim();
    if (!adding || !homes || !dir) return;
    const go = this.panel.querySelector<HTMLButtonElement>(".home-add-go");
    if (go) go.disabled = true;
    try {
      await homes(adding.machine, { change: "add", agent: adding.agent, dir });
      this.adding = null;
      this.note = { machine: adding.machine, text: this.options.words.homeAdded(dir) };
    } catch (error) {
      this.note = { machine: adding.machine, text: this.options.words.homeNotChanged(error instanceof Error ? error.message : String(error)) };
    }
    this.asked.delete(adding.machine);
    this.redraw();
    void this.ask();
  }

  /** Stops listing a login the World found or the user added; written
   *  again under "add", it comes back. */
  private async hideHome(button: HTMLButtonElement, machine: string, agent: string, dir: string): Promise<void> {
    const homes = this.options.homes;
    if (!homes || !dir) return;
    button.disabled = true;
    try {
      await homes(machine, { change: "hide", agent, dir });
      this.note = { machine, text: this.options.words.homeHidden(dirName(dir)) };
    } catch (error) {
      this.note = { machine, text: this.options.words.homeNotChanged(error instanceof Error ? error.message : String(error)) };
    }
    this.asked.delete(machine);
    this.redraw();
    void this.ask();
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
    const shape = [...spots.entries()].map(([key, spot]) => `${key}:${spot.kind === "machine" ? spot.machine.online : spot.kind === "lent" ? spot.lent[0]!.online : ""}`).join(",");
    if (shape !== this.shape) {
      this.shape = shape;
      void this.ask();
    }
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
        ? escape(words.factsUnknown(machine.facts.Err))
        : escape(route);
    // This computer, hub by hub: how it reaches each, and whose it is there.
    const hubs = machine.here
      ? world.nodes.hubs
          .map((hub) => {
            const stand = hub.home ? words.registeredUnder(hub.home) : hub.logins.length ? words.onlyLogins(hub.logins.join(words.separator)) : "";
            return `<li class="panel-hub ${hub.link.kind}"><span class="panel-hub-name">${escape(hub.name)}</span><span>${escape([linkSaid(hub.link, words), stand].filter(Boolean).join(" · "))}</span></li>`;
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
      ${live ? this.hosts(machine.name, { here: machine.here, mine, lending: owners, lent: null }, now) : ""}
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
      ${first.online === true ? this.hosts(first.machine, { here: false, mine: false, lending: [], lent }, now) : ""}
    </article>`;
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
      (r) => r.about.machine === machine && (r.here || r.now?.waiting_for_user) && (!group || residentIn(world, group, r)),
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

  /** The agents on a machine as its own programs say them, a block for
   *  each and a card in it for each login. `here` is this computer, where
   *  the World's tools are put in; `mine` a machine of the user's own,
   *  where logins are added and hidden; `lending` who may lend its logins;
   *  `lent` the logins lent to the account, on a machine of another, the
   *  only ones shown there. */
  private hosts(machine: string, on: { here: boolean; mine: boolean; lending: Login[]; lent: Borrowed[] | null }, now: Date): string {
    const { words } = this.options;
    const asked = this.asked.get(machine);
    if (!asked) return `<p class="machine-hosts-asking">${escape(words.asking)}</p>`;
    if (asked.state === "unknown") return `<p class="machine-hosts-asking">${escape(words.notAsked(asked.why))}</p>`;
    const changeable = on.mine && !!this.options.homes;
    const agents = on.lent ? [...new Set(on.lent.map((one) => one.agent))] : ["codex", "claude"];
    const blocks = agents.map((agent) => this.agentBlock(machine, agent as "codex" | "claude", asked.facts, on, changeable, now));
    return `<ul class="machine-hosts" aria-label="${escape(words.hosts)}">${blocks.join("")}</ul>`;
  }

  /** One agent on a machine: its program and version, then its logins. */
  private agentBlock(
    machine: string,
    agent: "codex" | "claude",
    facts: AgentFacts[],
    on: { here: boolean; lending: Login[]; lent: Borrowed[] | null },
    changeable: boolean,
    now: Date,
  ): string {
    const { words } = this.options;
    const name = agent === "codex" ? "Codex" : "Claude Code";
    const homes = homesOf(facts, agent);
    const program = homes.find(installed);
    if (on.lent) {
      // A machine of another says only the logins lent to the account.
      const lent = on.lent.filter((one) => one.agent === agent);
      const rows = lent.map((one) => {
        const home = homes.find((facts) => sameHome(one.home, homeChoice(facts)));
        return home
          ? this.homeLine(machine, agent, home, lent.length > 1, { here: false, lending: [], talk: usableHome(home) ? loginKey(one.machine, agent, one.home) : null }, false, now)
          : `<li class="host-home logged-out"><div class="home-head"><span class="home-who caution">${escape(words.borrowedGone)}</span>${one.home ? `<span class="home-dir"><code>${escape(dirName(one.home))}</code></span>` : ""}</div></li>`;
      });
      const version = program ? versionOf(program) : null;
      return `<li class="host-agent"><div class="host-line"><span class="host-name">${escape(name)}${version ? `<span class="host-version">${escape(version)}</span>` : ""}</span></div><ul class="host-homes">${rows.join("")}</ul></li>`;
    }
    if (!program) {
      return `<li class="host-agent absent"><div class="host-line"><span class="host-name">${escape(name)}</span><span class="host-fact">${escape(words.notInstalled)}</span></div></li>`;
    }
    const version = versionOf(program);
    const many = homes.length > 1;
    const rows = homes.map((home) => this.homeLine(machine, agent, home, many, { here: on.here, lending: on.lending, talk: null }, changeable, now)).join("");
    const adding = this.adding?.machine === machine && this.adding.agent === agent;
    const add = !changeable
      ? ""
      : adding
        ? `<form class="home-add" data-agent="${agent}"><input class="home-add-dir" type="text" value="${escape(this.adding!.dir)}" placeholder="${escape(words.addHomeWhere(agent))}" aria-label="${escape(words.addHomeWhere(agent))}" spellcheck="false" autocomplete="off" autocapitalize="off"><button class="text-button home-add-go" type="submit">${escape(words.addHomeGo)}</button><button class="text-button quiet-button" type="button" data-act="add-cancel">${escape(words.cancel)}</button></form>`
        : `<button class="text-button quiet-button home-add-open" type="button" data-act="add-home" data-agent="${agent}">${glyph("plus", 14)}<span>${escape(words.addHome)}</span></button>`;
    return `<li class="host-agent"><div class="host-line"><span class="host-name">${escape(name)}${version ? `<span class="host-version">${escape(version)}</span>` : ""}</span></div><ul class="host-homes">${rows}</ul>${add}</li>`;
  }

  /** One login of an agent, a card: who and on what plan, where it keeps
   *  it when there are several, how much of each window is spent, whom it
   *  is lent to, and what this device can change of it. */
  private homeLine(
    machine: string,
    agent: "codex" | "claude",
    facts: AgentFacts,
    many: boolean,
    on: { here: boolean; lending: Login[]; talk: string | null },
    changeable: boolean,
    now: Date,
  ): string {
    const { words, homeWords } = this.options;
    const out = loggedOut(facts);
    const { email, plan } = accountOf(facts);
    const login = facts.login.kind === "known" ? words.loggedIn : words.loginUnknown;
    const who = out
      ? `<span class="home-who caution">${escape(words.loggedOut)}</span>`
      : `<span class="home-who">${escape(email ?? login)}</span>${plan ? `<span class="home-plan">${escape(plan)}</span>` : ""}`;
    const dir = facts.home?.dir ?? null;
    const how = facts.home?.found === "found" ? homeWords.found : facts.home?.found === "added" ? homeWords.added : homeWords.defaultHome;
    const where = dir && (many || !isDefault(facts)) ? `<span class="home-dir" title="${escape(dir)}"><code>${escape(dirName(dir))}</code><span>${escape(how)}</span></span>` : "";
    const choice = homeChoice(facts) ?? "";
    const hide =
      changeable && !isDefault(facts) && dir
        ? `<button class="text-button quiet-button home-hide" type="button" data-act="hide-home" data-agent="${agent}" data-dir="${escape(dir)}" title="${escape(words.hideHomeWhat)}">${escape(words.hideHome)}</button>`
        : "";
    const talk = on.talk ? `<button class="text-button home-talk" type="button" data-act="talk" data-key="${escape(on.talk)}">${glyph("plus", 14)}<span>${escape(words.talk)}</span></button>` : "";
    // Whom its owner lent it to, each taken back with its cross, and
    // lending it to another.
    const label = `${agent === "codex" ? "Codex" : "Claude"} · ${homeLabel(facts, homeWords)}`;
    const lending = out
      ? ""
      : on.lending
          .map((lender) => {
            const data = `data-agent="${agent}" data-home="${escape(choice)}" data-label="${escape(label)}" data-lender="${escape(lender.account)}" data-lender-hub="${escape(lender.hub)}"`;
            return this.givenRow(
              on.lending.length > 1 ? `${words.lentTo} · ${lender.hubName}` : words.lentTo,
              this.lentTo(lender, machine, agent, homeChoice(facts)),
              (to) => `data-act="unlend" data-to="${escape(to)}" ${data}`,
              `<button class="text-button quiet-button" type="button" data-act="lend" ${data}>${glyph("plus", 13)}<span>${escape(words.lend)}</span></button>`,
            );
          })
          .join("");
    const limits = limitsHtml(facts.limits, homeWords, now, this.options.when, { notRead: agent === "claude" && !out && facts.login.kind === "known" });
    const integration = facts.integration.kind === "known" ? facts.integration.value : null;
    const tools = integration === "in" ? words.toolsIn : integration === "out" ? words.toolsOut : integration === "old" ? words.toolsOld : null;
    const act =
      on.here && integration && integration !== "absent"
        ? `<button class="text-button quiet-button" type="button" data-act="apply" data-agent="${agent}" data-home="${escape(choice)}" data-into="${integration === "in" ? "0" : "1"}">${escape(integration === "in" ? words.takeOut : integration === "old" ? words.renew : words.putIn)}</button>`
        : "";
    // Claude works for the World only once the user let it skip approvals
    // in that login; said, and changed, from any of the user's devices.
    const unattended = agent === "claude" && facts.unattended?.kind === "known" ? facts.unattended.value : null;
    const consentLabel = many ? homeLabel(facts, homeWords) : "";
    const consent =
      unattended === null || !this.options.consent || out || on.talk
        ? ""
        : `<span class="host-use"><span class="${unattended ? "" : "caution"}">${escape(unattended ? words.unattendedYes : words.unattendedNo)}</span><button class="text-button quiet-button" type="button" data-act="consent" data-home="${escape(choice)}" data-label="${escape(consentLabel)}" data-allow="${unattended ? "0" : "1"}">${escape(unattended ? words.withdraw : words.allow)}</button></span>`;
    return `<li class="host-home${out ? " logged-out" : ""}"><div class="home-head">${who}${where}${hide}${talk}</div>${limits}${tools || act ? `<span class="host-use">${tools ? `<span>${escape(tools)}</span>` : ""}${act}</span>` : ""}${consent}${lending}</li>`;
  }
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
