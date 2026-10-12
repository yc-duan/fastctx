// The agents (`world-gui.md`, 账号、机器、共享与借出): on every machine the
// account may use, the agents its own programs say it has, a card for each
// of their logins. A card: who is logged in and on what plan, how much of
// each usage window is spent, whom it is lent to, and what can be done with
// it from here: a new conversation in it, its tools put into the sessions
// the user opens on this computer, Claude's consent to work without asking,
// lending it, no longer listing it. A machine of another's an agent is lent
// on shows only the logins lent. The machines page shows the machines; this
// one the agents on them, and each machine there leads here.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { AccountChange, HubAccounts, Login } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { AgentFacts, HomeChange, WorldAgents } from "../world/agents";
import type { LoginChange, MachineLogins } from "../world/logins";
import type { WorldNow } from "../world/bridge";
import type { AccessWords } from "./account-words";
import { accountOf, dirName, homeChoice, homeLabel, homesOf, installed, isDefault, limitsHtml, loggedOut, usableHome, versionOf, type HomeWords } from "./agent-homes";
import type { AgentsWords } from "./agents-words";
import type { ConsentHome } from "./claude-consent";
import type { Confirm } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { GiveSheet } from "./machine-sheets";
import { link } from "./places";
import { detailOf } from "./problem";
import { borrowedOf, loginKey, machinesOf, ownerOf, owns, sameHome, type Borrowed, type Machine } from "./reach";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";

export interface AgentsOptions {
  store: Store;
  words: AgentsWords;
  access: AccessWords;
  homeWords: HomeWords;
  head(title: string, actions?: string): string;
  /** The machine to show alone, when one was named. */
  machine: string | null;
  when(at: Date, now: Date): string;
  /** What every online machine's node keeps of its agents. */
  agents(): Promise<WorldAgents>;
  /** What one machine's node keeps of its agents' logins: its scan and
   *  what the walk of its disks found. */
  logins?: ((machine: string) => Promise<MachineLogins | null>) | undefined;
  /** Scans one of the user's machines, or stops the walk of its disks. */
  scan?: ((machine: string, stop: boolean) => Promise<MachineLogins>) | undefined;
  /** Puts the World's tools into one login of an agent on a machine of the
   *  user's own, or takes them out, by that machine's node. */
  changeLogin(machine: string, agent: "codex" | "claude", dir: string, into: boolean, raise?: boolean): Promise<LoginChange>;
  /** Asks the user, and gives or withdraws Claude's consent to work
   *  without asking in one of its homes on a machine; true once done. */
  consent?: { allow(machine: string, home: ConsentHome): Promise<boolean>; withdraw(machine: string, home: ConsentHome): Promise<boolean> } | undefined;
  /** Adds a login the World did not find on a machine, or stops listing
   *  one; the user's own machines only. */
  homes?: ((machine: string, change: HomeChange) => Promise<void>) | undefined;
  change(login: Login, change: AccountChange): Promise<void>;
  /** Opens a new conversation with one agent's login chosen, by its key. */
  talkWith(key: string): void;
  confirm: Confirm;
}

/** What is known of the agents on one machine. */
type Asked = { state: "known"; facts: AgentFacts[] } | { state: "unknown"; why: string };

/** One machine as this page shows it. */
interface Shown {
  name: string;
  written: string;
  online: boolean;
  here: boolean;
  form: "desktop" | "laptop" | "server";
  /** The tag beside its name, and the saying behind it. */
  tag: { text: string; said: string; kind: string } | null;
  /** Its owner's logins here, who may lend what is on it. */
  lending: Login[];
  /** On a machine of another's, the logins lent to the account. */
  lent: Borrowed[] | null;
  changeable: boolean;
}

export class AgentsPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly body: HTMLElement;
  private readonly stop: () => void;
  private drawn = "";
  private shape = "";
  private asked = new Map<string, Asked>();
  private asking = false;
  /** What the last thing done on a card left to say, by the card's key. */
  private notes = new Map<string, { text: string; detail: string; caution: boolean; raise?: boolean }>();
  private adding: { machine: string; agent: string; dir: string } | null = null;
  private readonly giving: GiveSheet;
  /** What each machine's node keeps of its logins: its scan, and what the
   *  walk of its disks found. */
  private records = new Map<string, MachineLogins>();
  private following = false;
  private left = false;

  constructor(private readonly options: AgentsOptions) {
    const { words } = options;
    this.title = options.machine ?? words.title;
    this.giving = new GiveSheet(options.access);
    this.element = document.createElement("div");
    this.element.className = "agents-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(words.title)}</h1>`)}<div class="agents-body scroller"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".agents-body")!;
    this.body.addEventListener("click", (event) => this.click(event));
    this.body.addEventListener("input", (event) => {
      const input = event.target as HTMLInputElement;
      if (input.classList.contains("home-add-dir") && this.adding) this.adding.dir = input.value;
    });
    this.body.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.addHome();
    });
    this.body.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && (event.target as HTMLElement).classList.contains("home-add-dir")) {
        this.adding = null;
        this.redraw();
      }
    });
    this.stop = effect(() => this.draw(options.store.world.get(), options.store.clock.get()));
    void this.readRecords();
  }

  leave(): void {
    this.left = true;
    this.stop();
  }

  private click(event: Event): void {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button[data-act]");
    if (!button || button.disabled) return;
    const card = button.closest<HTMLElement>("[data-machine]");
    const machine = card?.dataset.machine ?? "";
    const agent = (button.dataset.agent ?? card?.dataset.agent ?? "") as "codex" | "claude";
    const home = button.dataset.home || null;
    const key = card?.dataset.key ?? "";
    switch (button.dataset.act) {
      case "talk":
        this.options.talkWith(button.dataset.key ?? "");
        return;
      case "apply":
        this.apply(button, key, machine, agent, button.dataset.into === "1", button.dataset.dir ?? "");
        return;
      case "raise":
        this.apply(button, key, machine, agent, true, button.dataset.dir ?? "", true);
        return;
      case "scan":
        void this.scan(machine, button.dataset.stop === "1");
        return;
      case "take-up":
        void this.takeUp(button, machine, agent, button.dataset.dir ?? "");
        return;
      case "consent":
        void this.consent(machine, button.dataset.allow === "1", { dir: home, label: button.dataset.label || null });
        return;
      case "lend": {
        const lender = this.lender(button);
        if (lender) this.lend(lender, machine, agent, home, button.dataset.label ?? "");
        return;
      }
      case "unlend": {
        const lender = this.lender(button);
        if (lender) void this.act(button, key, lender, { change: "lend", machine, to: button.dataset.to ?? "", agent, ...(home ? { home } : {}), lent: false });
        return;
      }
      case "add-home":
        this.adding = { machine, agent, dir: "" };
        this.redraw();
        this.body.querySelector<HTMLInputElement>(".home-add-dir")?.focus();
        return;
      case "add-cancel":
        this.adding = null;
        this.redraw();
        return;
    }
  }

  private lender(button: HTMLElement): Login | undefined {
    return loginsOf(this.options.store.world.peek()?.accounts).find((l) => l.hub === button.dataset.lenderHub && l.account === button.dataset.lender);
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
      for (const machine of machinesOf(this.options.store.world.peek())) this.asked.set(machine.name, { state: "unknown", why: detailOf(error) });
    }
    this.asking = false;
    this.redraw();
  }

  private again(machine: string): void {
    this.asked.delete(machine);
    this.redraw();
    void this.ask();
  }

  private hubOf(login: Login): HubAccounts | null {
    return this.options.store.world.peek()?.accounts.hubs.find((hub) => hub.hub === login.hub) ?? null;
  }

  private lentTo(login: Login, machine: string, agent: string, home: string | null): string[] {
    return (this.hubOf(login)?.lends ?? []).filter((lend) => lend.machine === machine && lend.agent === agent && sameHome(lend.home, home)).map((lend) => lend.account);
  }

  private lend(login: Login, machine: string, agent: string, home: string | null, label: string): void {
    const { access, words } = this.options;
    this.giving.open(
      { title: words.lendTitle(label, machine), what: access.lendWhat, sees: access.lendSees, given: access.lentTo, go: access.lendGo },
      this.lentTo(login, machine, agent, home),
      async (to, lent) => {
        await this.options.change(login, { change: "lend", machine, to, agent, ...(home ? { home } : {}), lent });
        await this.options.store.refresh();
        return this.lentTo(login, machine, agent, home);
      },
    );
  }

  private async act(button: HTMLButtonElement, key: string, login: Login, change: AccountChange): Promise<void> {
    button.disabled = true;
    try {
      await this.options.change(login, change);
      this.notes.delete(key);
    } catch (error) {
      this.notes.set(key, { text: this.options.words.changeFailed(detailOf(error)), detail: detailOf(error), caution: true });
    }
    await this.options.store.refresh();
    this.redraw();
  }

  /** Claude Code's tools go in only after the user has read what happens to
   *  sessions already open (the list's 21); Codex's, and taking either
   *  out, at once. What apply left as it was is behind a pointer's rest;
   *  its account of the files it wrote never reaches the page. Codex's
   *  output limit is raised only when the user asks, here as with
   *  `--raise-output-limit` (`world-run.md`). */
  private apply(button: HTMLButtonElement, key: string, machine: string, agent: "codex" | "claude", into: boolean, dir: string, raise = false): void {
    const { words } = this.options;
    const run = async () => {
      const change = await this.options.changeLogin(machine, agent, dir, into, raise);
      const end = change.end;
      if (end?.end === "failed") {
        this.notes.set(key, { text: (into ? words.applyFailed : words.takeOutFailed)(end.reason), detail: end.reason, caution: true, raise: !!end.output_limit });
        return;
      }
      const kept = (end?.end === "done" ? end.kept : undefined) ?? [];
      this.notes.set(key, kept.length ? { text: words.kept, detail: kept.map((one) => `${one.location}: ${one.reason}`).join("\n"), caution: false } : { text: "", detail: "", caution: false });
    };
    const failed = (error: unknown) => {
      const detail = detailOf(error);
      this.notes.set(key, { text: (into ? words.applyFailed : words.takeOutFailed)(detail), detail, caution: true });
    };
    const done = () => this.again(machine);
    if (agent === "claude" && into) {
      this.options.confirm.ask({ title: words.claudeTitle, what: words.claudeWhat, go: words.claudeGo, gentle: true, act: run, done });
      return;
    }
    button.disabled = true;
    run()
      .catch(failed)
      .finally(done);
  }

  private async consent(machine: string, allow: boolean, home: ConsentHome): Promise<void> {
    const consent = this.options.consent;
    if (!consent || !machine) return;
    const done = await (allow ? consent.allow(machine, home) : consent.withdraw(machine, home));
    if (done) this.again(machine);
  }

  private async addHome(): Promise<void> {
    const adding = this.adding;
    const homes = this.options.homes;
    const dir = adding?.dir.trim();
    if (!adding || !homes || !dir) return;
    const go = this.body.querySelector<HTMLButtonElement>(".home-add-go");
    if (go) go.disabled = true;
    const key = `add:${adding.machine}:${adding.agent}`;
    try {
      await homes(adding.machine, { change: "add", agent: adding.agent, dir });
      this.adding = null;
      this.notes.delete(key);
    } catch (error) {
      this.notes.set(key, { text: this.options.words.changeFailed(detailOf(error)), detail: detailOf(error), caution: true });
    }
    this.again(adding.machine);
  }

  /** Takes up a directory the walk of the disks found as a candidate. */
  private async takeUp(button: HTMLButtonElement, machine: string, agent: string, dir: string): Promise<void> {
    const homes = this.options.homes;
    if (!homes || !dir) return;
    button.disabled = true;
    const key = `add:${machine}:${agent}`;
    try {
      await homes(machine, { change: "add", agent, dir });
      this.notes.delete(key);
    } catch (error) {
      this.notes.set(key, { text: this.options.words.changeFailed(detailOf(error)), detail: detailOf(error), caution: true });
    }
    this.again(machine);
  }

  /** Scans a machine, or stops the walk of its disks; while a walk goes on
   *  its progress is read again each second the page is open. */
  private async scan(machine: string, stop: boolean): Promise<void> {
    const scan = this.options.scan;
    if (!scan) return;
    try {
      this.records.set(machine, await scan(machine, stop));
    } catch (error) {
      this.notes.set(`scan:${machine}`, { text: this.options.words.changeFailed(detailOf(error)), detail: detailOf(error), caution: true });
    }
    this.again(machine);
    this.follow();
  }

  /** Reads the records of the machines whose disks are being walked, until
   *  none is or the page is left. */
  private follow(): void {
    if (this.following || this.left) return;
    const walking = [...this.records.values()].filter((record) => record.scan?.disks?.stage.state === "running").map((record) => record.machine);
    if (!walking.length || !this.options.logins) return;
    this.following = true;
    window.setTimeout(async () => {
      for (const machine of walking) {
        const record = await this.options.logins!(machine).catch(() => null);
        if (record) this.records.set(machine, record);
      }
      this.following = false;
      if (this.left) return;
      const done = walking.filter((machine) => this.records.get(machine)?.scan?.disks?.stage.state !== "running");
      for (const machine of done) this.again(machine);
      this.redraw();
      this.follow();
    }, 1000);
  }

  private async readRecords(): Promise<void> {
    const logins = this.options.logins;
    if (!logins) return;
    for (const machine of machinesOf(this.options.store.world.peek()).filter((m) => m.here || m.online)) {
      const record = await logins(machine.name).catch(() => null);
      if (record) this.records.set(machine.name, record);
    }
    this.redraw();
    this.follow();
  }

  private redraw(): void {
    this.drawn = "";
    this.draw(this.options.store.world.peek(), this.options.store.clock.peek());
  }

  /** The machines shown: this computer, the account's own, those shared
   *  with it, then those an agent is lent from. */
  private machines(world: WorldNow): Shown[] {
    const { words } = this.options;
    const logins = loginsOf(world.accounts);
    const formOf = (machine: Machine) => ("Ok" in machine.facts ? machine.facts.Ok.form : undefined) ?? "desktop";
    const shown: Shown[] = machinesOf(world).map((machine) => {
      const owner = logins.find((login) => owns(machine, login)) ?? null;
      const mine = machine.here || !!owner;
      const ownerName = mine ? null : (logins.map((login) => ownerOf(machine, login)).find(Boolean) ?? null);
      const lending = machine.here ? logins.filter((login) => world.nodes.hubs.some((hub) => hub.hub === login.hub && hub.home === login.account)) : owner ? [owner] : [];
      return {
        name: machine.name,
        written: machine.written,
        online: machine.here || machine.online === true,
        here: machine.here,
        form: formOf(machine),
        tag: machine.here ? { text: words.here, said: "", kind: "" } : owner ? { text: words.yours, said: "", kind: "tag-own" } : ownerName ? { text: words.sharedBy(ownerName), said: "", kind: "" } : null,
        lending,
        lent: null,
        changeable: mine && !!this.options.homes,
      };
    });
    const lent = new Map<string, Borrowed[]>();
    for (const one of borrowedOf(world)) lent.set(one.written, [...(lent.get(one.written) ?? []), one]);
    for (const [written, ones] of lent) {
      const first = ones[0]!;
      shown.push({
        name: first.machine,
        written,
        online: first.online === true,
        here: false,
        form: "desktop",
        tag: { text: words.lentBy(first.owner ?? ""), said: words.lentSaid(first.owner ?? ""), kind: "tag-lent" },
        lending: [],
        lent: ones,
        changeable: false,
      });
    }
    const rank = (m: Shown) => (m.here ? 0 : m.tag?.kind === "tag-own" ? 1 : m.lent ? 3 : 2);
    return shown.sort((a, b) => rank(a) - rank(b));
  }

  private draw(world: WorldNow | null, now: Date): void {
    if (!world) return;
    const { words } = this.options;
    const machines = this.machines(world);
    const shape = machines.map((m) => `${m.written}:${m.online}`).join(",");
    if (shape !== this.shape) {
      this.shape = shape;
      void this.ask();
    }
    const only = this.options.machine;
    const chosen = only ? machines.filter((m) => m.name === only || m.written === only) : machines;
    const filter =
      machines.length > 1
        ? `<nav class="agents-filter" aria-label="${escape(words.title)}">
            <a class="agents-filter-one${only ? "" : " current"}" href="${link({ kind: "agents", machine: null })}">${escape(words.all)}</a>
            ${machines.map((m) => `<a class="agents-filter-one${only && (m.name === only || m.written === only) ? " current" : ""}" href="${link({ kind: "agents", machine: m.written })}">${glyph(m.form, 14)}<span>${escape(m.written)}</span></a>`).join("")}
          </nav>`
        : "";
    const sections = chosen.map((m) => this.section(m, now)).join("");
    const html = filter + (sections || `<p class="agents-nothing">${escape(words.nothing)}</p>`);
    if (html === this.drawn) return;
    this.drawn = html;
    redrawKeepingFocus(this.body, () => {
      this.body.innerHTML = html;
    });
  }

  /** One machine: its name and state, then a card for each login of each
   *  agent on it. */
  private section(m: Shown, now: Date): string {
    const { words } = this.options;
    const asked = this.asked.get(m.name);
    let cards: string;
    if (!m.online) cards = "";
    else if (!asked) cards = `<p class="agents-quiet">${escape(words.asking)}</p>`;
    else if (asked.state === "unknown") cards = `<p class="agents-quiet" title="${escape(asked.why)}">${escape(words.notAsked)}</p>`;
    else {
      const agents = m.lent ? [...new Set(m.lent.map((one) => one.agent))] : ["codex", "claude"];
      cards = `<ul class="agent-cards">${agents.map((agent) => this.cards(m, agent as "codex" | "claude", asked.facts, now)).join("")}</ul>`;
    }
    const tag = m.tag ? `<span class="tag ${m.tag.kind}"${m.tag.said ? ` title="${escape(m.tag.said)}"` : ""}>${escape(m.tag.text)}</span>` : "";
    const record = this.records.get(m.name);
    const walk = record?.scan?.disks;
    const walking = walk?.stage.state === "running";
    const scanned = record?.scan ? (walk?.disks ?? []).reduce((sum, disk) => sum + disk.directories, 0) : null;
    const scanNote = this.notes.get(`scan:${m.name}`);
    const scanControl =
      m.changeable && m.online && this.options.scan
        ? `<span class="agents-scan">${walking && scanned !== null ? `<span class="agents-quiet">${escape(words.walking(scanned))}</span>` : ""}<button class="text-button quiet-button" type="button" data-act="scan" data-stop="${walking ? "1" : "0"}">${escape(walking ? words.stopWalk : record?.scan ? words.scanAgain : words.scan)}</button>${scanNote ? `<span class="caution" title="${escape(scanNote.detail)}">${escape(scanNote.text)}</span>` : ""}</span>`
        : "";
    return `<section class="agents-machine${m.online ? "" : " offline"}">
      <header class="agents-machine-head">
        <span class="agents-machine-form" aria-hidden="true">${glyph(m.form, 18)}</span>
        <h2 class="agents-machine-name">${escape(m.written)}</h2>
        ${tag}
        <span class="agents-machine-state"><span class="dot" aria-hidden="true"></span>${escape(m.online ? words.online : words.offline)}</span>
        ${scanControl}
        <a class="icon-button agents-machine-map" href="${link({ kind: "machine", name: m.written })}" title="${escape(words.onMap(m.written))}" aria-label="${escape(words.onMap(m.written))}">${glyph("machine", 16)}</a>
      </header>
      ${cards}
      ${this.elsewhere(m, record)}
    </section>`;
  }

  /** What the walk of a machine's disks found that looks like a login, a
   *  row each, each to be taken up as a candidate. */
  private elsewhere(m: Shown, record: MachineLogins | undefined): string {
    if (!record || !m.changeable) return "";
    const { words } = this.options;
    const rows = (["codex", "claude"] as const).flatMap((agent) =>
      record[agent].elsewhere.map(
        (one) =>
          `<li class="agents-elsewhere-one"><code title="${escape(one.traces.join(", "))}">${escape(one.dir)}</code><button class="text-button quiet-button" type="button" data-act="take-up" data-agent="${agent}" data-dir="${escape(one.dir)}">${escape(words.takeUp(agent === "codex" ? "Codex" : "Claude Code"))}</button></li>`,
      ),
    );
    return rows.length ? `<details class="agents-elsewhere" data-machine="${escape(m.name)}"><summary>${escape(words.elsewhere(rows.length))}</summary><ul>${rows.join("")}</ul></details>` : "";
  }

  /** The cards of one agent on a machine: one per login, or one saying it
   *  is not there; and, on the user's own machines, the way to add one. */
  private cards(m: Shown, agent: "codex" | "claude", facts: AgentFacts[], now: Date): string {
    const { words } = this.options;
    const name = agent === "codex" ? "Codex" : "Claude Code";
    const homes = homesOf(facts, agent);
    const program = homes.find(installed);
    if (m.lent) {
      return m.lent
        .filter((one) => one.agent === agent)
        .map((one) => {
          const home = homes.find((facts) => sameHome(one.home, homeChoice(facts)));
          return home
            ? this.card(m, agent, home, homes.length > 1, usableHome(home) ? loginKey(one.machine, agent, one.home) : null, now)
            : `<li class="agent-card gone" data-machine="${escape(m.name)}"><div class="agent-card-head">${glyph(agent, 18)}<span class="agent-card-name">${escape(name)}</span></div><p class="agent-card-who caution">${escape(words.borrowedGone)}</p></li>`;
        })
        .join("");
    }
    if (!program) return `<li class="agent-card absent"><div class="agent-card-head">${glyph(agent, 18)}<span class="agent-card-name">${escape(words.notInstalled(name))}</span></div></li>`;
    const many = homes.length > 1;
    const rows = homes.map((home) => this.card(m, agent, home, many, usableHome(home) ? loginKey(m.name, agent, homeChoice(home)) : null, now)).join("");
    if (!m.changeable) return rows;
    const adding = this.adding?.machine === m.name && this.adding.agent === agent;
    const note = this.notes.get(`add:${m.name}:${agent}`);
    const add = adding
      ? `<li class="agent-card adding" data-machine="${escape(m.name)}" data-agent="${agent}"><form class="home-add"><input class="home-add-dir" type="text" value="${escape(this.adding!.dir)}" placeholder="${escape(words.addHomeWhere(agent))}" aria-label="${escape(words.addHomeWhere(agent))}" spellcheck="false" autocomplete="off" autocapitalize="off"><button class="text-button home-add-go" type="submit">${escape(words.addHomeGo)}</button><button class="text-button quiet-button" type="button" data-act="add-cancel">${escape(words.cancel)}</button></form>${note ? `<p class="agent-card-note caution" title="${escape(note.detail)}">${escape(note.text)}</p>` : ""}</li>`
      : `<li class="agent-card add" data-machine="${escape(m.name)}" data-agent="${agent}"><button class="agent-card-add" type="button" data-act="add-home" data-agent="${agent}">${glyph("plus", 16)}<span>${escape(words.addHome)}</span><span class="agent-card-add-what">${escape(name)}</span></button></li>`;
    return rows + add;
  }

  /** One login of an agent: who and on what plan, where it is kept when
   *  there are several, its usage windows, whom it is lent to, and what can
   *  be done with it from here. */
  private card(m: Shown, agent: "codex" | "claude", facts: AgentFacts, many: boolean, talk: string | null, now: Date): string {
    const { words, homeWords } = this.options;
    const name = agent === "codex" ? "Codex" : "Claude Code";
    const out = loggedOut(facts);
    const { email, plan } = accountOf(facts);
    const choice = homeChoice(facts) ?? "";
    const dir = facts.home?.dir ?? null;
    const key = `${m.name}|${agent}|${choice}`;
    const version = versionOf(facts);
    const who = out
      ? `<span class="agent-card-who caution">${escape(words.loggedOut)}</span>`
      : `<span class="agent-card-who">${escape(email ?? words.loginUnknown)}</span>${plan ? `<span class="home-plan">${escape(plan)}</span>` : ""}`;
    const where = dir && (many || !isDefault(facts)) ? `<span class="agent-card-dir" title="${escape(dir)}"><code>${escape(dirName(dir))}</code></span>` : "";
    const label = `${agent === "codex" ? "Codex" : "Claude"} · ${homeLabel(facts, homeWords)}`;

    // The World's tools in the sessions the user opens on this computer.
    const integration = facts.integration.kind === "known" ? facts.integration.value : null;
    const tools =
      m.changeable && integration && integration !== "absent"
        ? `${integration === "in" ? `<span class="tag tag-own" title="${escape(words.toolsInSaid(agent))}">${escape(words.toolsIn)}</span>` : integration === "old" ? `<span class="tag caution">${escape(words.toolsOld)}</span>` : ""}
           <button class="text-button quiet-button" type="button" data-act="apply" data-agent="${agent}" data-dir="${escape(dir ?? "")}" data-into="${integration === "in" ? "0" : "1"}">${escape(integration === "in" ? words.takeOut : integration === "old" ? words.renew : words.putIn)}</button>`
        : "";
    // Claude works for the World only once the user let it skip approvals
    // in that login.
    const unattended = agent === "claude" && facts.unattended?.kind === "known" ? facts.unattended.value : null;
    const consent =
      unattended === null || !this.options.consent || out || m.lent
        ? ""
        : `<span class="tag${unattended ? "" : " caution"}" title="${escape(words.unattendedSaid)}">${escape(unattended ? words.unattendedYes : words.unattendedNo)}</span>
           <button class="text-button quiet-button" type="button" data-act="consent" data-home="${escape(choice)}" data-label="${escape(many ? homeLabel(facts, homeWords) : "")}" data-allow="${unattended ? "0" : "1"}">${escape(unattended ? words.withdraw : words.allow)}</button>`;
    // Whom its owner lent it to, each taken back with its cross.
    const lending = out
      ? ""
      : m.lending
          .map((lender) => {
            const data = `data-agent="${agent}" data-home="${escape(choice)}" data-label="${escape(label)}" data-lender="${escape(lender.account)}" data-lender-hub="${escape(lender.hub)}"`;
            const given = this.lentTo(lender, m.name, agent, homeChoice(facts));
            const chips = given
              .map((to) => `<span class="chip">${escape(to)}<button class="chip-x" type="button" data-act="unlend" data-to="${escape(to)}" ${data} aria-label="${escape(words.takeBack(to))}" title="${escape(words.takeBack(to))}">${glyph("close", 12)}</button></span>`)
              .join("");
            return given.length ? `<div class="given-row"><span class="given-label">${escape(words.lentTo)}</span>${chips}</div>` : "";
          })
          .join("");
    const lendButtons = out
      ? ""
      : m.lending
          .map(
            (lender) =>
              `<button class="icon-button" type="button" data-act="lend" data-agent="${agent}" data-home="${escape(choice)}" data-label="${escape(label)}" data-lender="${escape(lender.account)}" data-lender-hub="${escape(lender.hub)}" title="${escape(words.lend)}" aria-label="${escape(words.lend)}">${glyph("colleague", 16)}</button>`,
          )
          .join("");
    const talkButton = talk
      ? `<button class="text-button agent-card-talk" type="button" data-act="talk" data-key="${escape(talk)}">${glyph("plus", 14)}<span>${escape(words.newTalk)}</span></button>`
      : "";
    const note = this.notes.get(key);
    const raise = note?.raise
      ? `<button class="text-button quiet-button" type="button" data-act="raise" data-agent="${agent}" data-dir="${escape(dir ?? "")}" title="${escape(words.raiseSaid)}">${escape(words.raise)}</button>`
      : "";
    const said = note?.text ? `<p class="agent-card-note${note.caution ? " caution" : ""}"><span title="${escape(note.detail)}">${escape(note.text)}</span>${raise}</p>` : "";
    const limits = limitsHtml(facts.limits, homeWords, now, this.options.when, { notRead: agent === "claude" && !out && facts.login.kind === "known" });
    return `<li class="agent-card${out ? " logged-out" : ""}" data-machine="${escape(m.name)}" data-agent="${agent}" data-key="${escape(key)}">
      <div class="agent-card-head">${glyph(agent, 18)}<span class="agent-card-name">${escape(name)}</span>${version ? `<span class="host-version">${escape(version)}</span>` : ""}</div>
      <div class="agent-card-line">${who}${where}</div>
      ${limits}
      ${tools || consent ? `<div class="agent-card-line agent-card-uses">${tools}${consent}</div>` : ""}
      ${lending}
      ${said}
      ${talkButton || lendButtons ? `<div class="agent-card-acts">${talkButton}<span class="agent-card-more">${lendButtons}</span></div>` : ""}
    </li>`;
  }
}
