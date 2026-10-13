// The user's own things in the network shown (`world-gui.md`, 账号、机器、
// 共享与协助: 「我」): who they are there, their password, and what of
// theirs others may use — the machines shared and the logins assisting, each
// taken back with its cross and for a moment given back; the machines of
// others whose agents may reach theirs; the phones logged in to the account;
// and leaving the network. A device that came into the account lately is
// said first, with "that wasn't me". Whatever has nothing in it is not
// drawn. Other people are the boxes on the machines page's drawing.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { circle, seeded } from "../pen/shapes";
import { loginsOf, type AccountChange, type Arrival, type AssistSeen, type HubAccounts, type Login, type MemberSeen } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import { wholeDollars } from "../world/money";
import { agentName } from "../world/names";
import type { MachinesWords } from "./account-words";
import { assistChange } from "./assist-sheet";
import { Confirm, Sheet } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { linkSaid } from "./machines";
import type { MeWords } from "./me-words";
import { newPasswordFields, passwordReady, sayPassword } from "./new-password";
import { link } from "./places";
import { isComputer, superOf } from "./reach";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";
import { factsByMachine } from "./talk-row";

export interface MeOptions {
  store: Store;
  words: MeWords;
  machines: MachinesWords;
  head(title: string, actions?: string): string;
  /** When, as the front page says it. */
  when(at: Date, now: Date): string;
  /** How long until a moment, short, as the assistance tags say it. */
  short(until: Date, now: Date): string;
  change(login: Login, change: AccountChange): Promise<void>;
  confirm: Confirm;
  /** Says a line for a moment, with the way back where there is one. */
  say(text: string, options?: { undo?: () => void; detail?: string }): void;
}

/** One thing taken back, until the server has done it. */
type Taking = { hub: string; key: string };

export class MePage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly body: HTMLElement;
  private readonly stop: () => void;
  private drawn = "";
  private readonly password = new Sheet("device-sheet password-sheet");
  /** Chips taken back here, gone from the page before the server answers. */
  private readonly taking: Taking[] = [];

  constructor(private readonly options: MeOptions) {
    const { words } = options;
    this.title = words.title;
    this.element = document.createElement("div");
    this.element.className = "settings-page me-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(words.title)}</h1>`)}<div class="me scroller"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".me")!;
    this.body.addEventListener("click", (event) => this.click(event));
    this.stop = effect(() => this.draw(options.store.world.get(), options.store.clock.get()));
  }

  leave(): void {
    this.stop();
  }

  private loginAt(element: HTMLElement): Login | undefined {
    const at = element.closest<HTMLElement>("[data-hub]");
    return loginsOf(this.options.store.world.peek()?.accounts).find((login) => login.hub === at?.dataset.hub && login.account === at?.dataset.account);
  }

  private hubOf(login: Login): HubAccounts | null {
    return this.options.store.world.peek()?.accounts.hubs.find((hub) => hub.hub === login.hub) ?? null;
  }

  private click(event: Event): void {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]");
    if (!button || button.disabled) return;
    const login = this.loginAt(button);
    if (!login) return;
    const { words, confirm, change, store } = this.options;
    const name = button.dataset.name ?? "";
    const refresh = () => void store.refresh();
    switch (button.dataset.act) {
      case "password":
        return this.changePassword(login);
      case "seen":
        return void this.quietly(button, login, { change: "seen", name });
      case "not-me":
        return confirm.ask({
          title: words.removeTitle(name),
          what: words.notMeWhat(name),
          go: words.removeGo,
          act: async () => {
            await change(login, { change: "remove", name, reason: "not me" });
            await change(login, { change: "seen", name });
          },
          done: refresh,
        });
      case "remove-device":
        return confirm.ask({ title: words.removeTitle(name), what: words.removeDeviceWhat(name), go: words.removeGo, act: () => change(login, { change: "remove", name, reason: "removed" }), done: refresh });
      case "stop-reaching":
        return confirm.ask({
          title: words.stopReachingTitle(name),
          what: words.stopReachingWhat(name),
          go: words.stopReaching,
          act: () => change(login, { change: "stop_acting", machine: name }),
          done: refresh,
        });
      case "unshare": {
        const machine = button.dataset.machine!;
        const give: AccountChange = { change: "share", machine, to: name, shared: true };
        return void this.takeBack(login, `share\n${machine}\n${name}`, words.sharedLabel(machine, name), { ...give, shared: false }, give);
      }
      case "unassist": {
        const assist = this.hubOf(login)?.assists.find((one) => assistKey(one) === button.dataset.assist);
        if (!assist) return;
        const until = assist.until ? new Date(assist.until) : null;
        const weekly = assist.weekly ? assist.weekly / 1_000_000 : null;
        const give = assistChange(assist.machine, assist.agent, assist.home, assist.account, until, weekly, true);
        const take = assistChange(assist.machine, assist.agent, assist.home, assist.account, null, null, false);
        return void this.takeBack(login, `assist\n${assistKey(assist)}`, words.assistLabel(`${agentName(assist.agent)} · ${assist.machine}`, assist.account), take, give);
      }
      case "leave":
        return this.leaveNetwork(login);
    }
  }

  /** Takes back a share or an assistance: gone from the page at once, and
   *  for a moment given back from the line that says so. */
  private async takeBack(login: Login, key: string, label: string, take: AccountChange, give: AccountChange): Promise<void> {
    const { words, change, store, say } = this.options;
    const taking = { hub: login.hub, key };
    this.taking.push(taking);
    this.redraw();
    try {
      await change(login, take);
    } catch (error) {
      const said = error instanceof Error ? error.message : String(error);
      say(words.failed(said), { detail: said });
      return;
    } finally {
      this.taking.splice(this.taking.indexOf(taking), 1);
    }
    say(words.tookBack(label), {
      undo: () => {
        void change(login, give).then(
          () => store.refresh(),
          (error: unknown) => say(words.failed(String(error instanceof Error ? error.message : error))),
        );
      },
    });
    await store.refresh();
    this.redraw();
  }

  private async quietly(button: HTMLButtonElement, login: Login, change: AccountChange): Promise<void> {
    button.disabled = true;
    try {
      await this.options.change(login, change);
    } catch (error) {
      const said = error instanceof Error ? error.message : String(error);
      this.options.say(this.options.words.failed(said), { detail: said });
    }
    await this.options.store.refresh();
    this.redraw();
  }

  private changePassword(login: Login): void {
    const { words, change, say } = this.options;
    const { password: sheet } = this;
    sheet.body.innerHTML = `
      ${sheet.head(words.passwordTitle, words.close)}
      <p class="confirm-what">${escape(words.passwordWhat)}</p>
      ${newPasswordFields(words, { pair: false })}
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.passwordGo)}</button></footer>`;
    sheet.body.oninput = () => sayPassword(sheet.body, words, login.account);
    sheet.body.onsubmit = (event) => {
      event.preventDefault();
      if (sheet.busy || !passwordReady(sheet.body, words, login.account)) return;
      const button = sheet.body.querySelector<HTMLButtonElement>(".sheet-go")!;
      const note = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
      const password = sheet.body.querySelector<HTMLInputElement>("[name=password]")!.value;
      sheet.busy = true;
      button.disabled = true;
      button.textContent = words.working;
      note.hidden = true;
      change(login, { change: "set_password", target: login.account, password }).then(
        () => {
          sheet.busy = false;
          sheet.close();
          say(words.passwordDone);
        },
        (error: unknown) => {
          sheet.busy = false;
          button.disabled = false;
          button.textContent = words.passwordGo;
          note.textContent = words.failed(error instanceof Error ? error.message : String(error));
          note.hidden = false;
        },
      );
    };
    sheet.show();
    sheet.body.querySelector<HTMLInputElement>("input")?.focus();
  }

  /** Leaves the network: every account this device has there logs out, the
   *  one this computer is registered under last, so the others are not left
   *  on a machine already gone. A phone logs out. */
  private leaveNetwork(login: Login): void {
    const { words, store, confirm, change } = this.options;
    const world = store.world.peek();
    const hub = this.hubOf(login);
    if (!world || !hub) return;
    const computer = isComputer(world);
    const logins = loginsOf(world.accounts).filter((one) => one.hub === hub.hub);
    const ordered = [...logins.filter((one) => one.account !== hub.home), ...logins.filter((one) => one.account === hub.home)];
    confirm.ask({
      title: computer ? words.leaveTitle(hub.name) : words.logoutTitle(hub.name),
      what: computer ? words.leaveWhat(hub.name, hub.home !== null) : words.logoutWhat(hub.name),
      go: computer ? words.leaveGo : words.logoutGo,
      act: async () => {
        for (const one of ordered) await change(one, { change: "logout" });
      },
      done: () => {
        void store.refresh();
        location.hash = link({ kind: "agents", machine: null });
      },
    });
  }

  private redraw(): void {
    this.drawn = "";
    this.draw(this.options.store.world.peek(), this.options.store.clock.peek());
  }

  private draw(world: WorldNow | null, now: Date): void {
    if (!world) return;
    const { words } = this.options;
    const logins = loginsOf(world.accounts);
    const html = logins.length
      ? logins.map((login) => this.section(login, world, now)).join("")
      : `<p class="me-none">${escape(words.none)} <a class="text-link" href="${link({ kind: "login" })}">${escape(words.logIn)}</a></p>`;
    if (html === this.drawn) return;
    this.drawn = html;
    redrawKeepingFocus(this.body, () => {
      this.body.innerHTML = html;
    });
  }

  private section(login: Login, world: WorldNow, now: Date): string {
    const hub = world.accounts.hubs.find((one) => one.hub === login.hub);
    if (!hub) return "";
    const computer = isComputer(world);
    const { words } = this.options;
    return `<section class="me-login" data-hub="${escape(login.hub)}" data-account="${escape(login.account)}">
      ${this.card(login, hub, world)}
      ${this.arrivals(hub.arrivals.filter((arrival) => arrival.account === login.account), world, now)}
      ${this.given(login, hub, world, now)}
      ${this.reaching(login, hub, now)}
      ${this.devices(login, hub, world, now)}
      <div class="me-leave"><button class="text-button danger-text" type="button" data-act="leave">${glyph("logout", 16)}<span>${escape(computer ? words.leave : words.logout)}</span></button></div>
    </section>`;
  }

  /** Who the user is: a seal with their initial, their name, whether they
   *  are the administrator, the network and how it is reached. */
  private card(login: Login, hub: HubAccounts, world: WorldNow): string {
    const { words, machines } = this.options;
    const me = hub.accounts.find((account) => account.name === login.account);
    const admin = superOf(world, login);
    // How the server is reached, in a word; where and through what, for a
    // pointer resting on it.
    const state = hub.link.kind === "online" ? machines.hubOnline : hub.link.kind === "connecting" ? machines.hubConnecting : machines.hubOff;
    const facts = [
      me ? `<span>${escape(words.level[me.level])}</span>` : "",
      `<span>${escape(hub.name)}</span>`,
      admin && admin !== login.account ? `<span class="me-admin" title="${escape(words.superNote(admin))}">${glyph("shield", 13)}${escape(words.adminIs(admin))}</span>` : "",
      hub.link.kind === "alone" ? "" : `<span class="me-link ${hub.link.kind}" title="${escape(linkSaid(hub.link, machines))}"><i aria-hidden="true"></i>${escape(state)}</span>`,
    ].filter(Boolean);
    return `<header class="me-card">
      <span class="me-seal" aria-hidden="true">${seal(login.account)}</span>
      <div class="me-who"><h2 class="me-name">${escape(login.account)}</h2><p class="me-facts">${facts.join("")}</p></div>
      <button class="text-button quiet-button me-password" type="button" data-act="password">${glyph("key", 16)}<span>${escape(words.changePassword)}</span></button>
    </header>`;
  }

  private arrivals(arrivals: Arrival[], world: WorldNow, now: Date): string {
    if (!arrivals.length) return "";
    const { words, when } = this.options;
    const facts = factsByMachine(world);
    return `<ul class="me-arrivals" aria-live="polite">${arrivals
      .map((arrival) => {
        const form = arrival.device ? undefined : facts.get(arrival.name)?.form;
        const kind = form ? words.forms[form] : words.kinds[arrival.device ? "phone" : "computer"];
        return `<li class="me-arrival">${glyph(arrival.device ? "phone" : "machine", 16)}<span class="me-arrival-text">${escape(words.arrived(kind, arrival.name, when(new Date(arrival.noticed_at), now)))}</span><span class="me-arrival-acts"><button class="text-button quiet-button" type="button" data-act="seen" data-name="${escape(arrival.name)}">${escape(words.me)}</button><button class="text-button quiet-button danger" type="button" data-act="not-me" data-name="${escape(arrival.name)}">${escape(words.notMe)}</button></span></li>`;
      })
      .join("")}</ul>`;
  }

  /** The user's machines shared with others and their logins assisting
   *  others, a row each, whom as chips to take back. */
  private given(login: Login, hub: HubAccounts, world: WorldNow, now: Date): string {
    const { words, short } = this.options;
    const mine = new Set(hub.members.filter((member) => member.home === login.account).map((member) => member.name));
    const taken = (key: string) => this.taking.some((one) => one.hub === hub.hub && one.key === key);
    const shares = hub.shares.filter((share) => mine.has(share.machine) && !taken(`share\n${share.machine}\n${share.account}`));
    const assists = hub.assists.filter((assist) => mine.has(assist.machine) && !taken(`assist\n${assistKey(assist)}`));
    if (!shares.length && !assists.length) return "";
    const forms = factsByMachine(world);
    const chip = (label: string, take: string, title: string, more = "") =>
      `<span class="chip">${escape(label)}${more}<button class="chip-x" type="button" ${take} aria-label="${escape(title)}" title="${escape(title)}">${glyph("close", 12)}</button></span>`;
    const machines = [...new Set(shares.map((share) => share.machine))].map((machine) => {
      const form = forms.get(machine)?.form;
      const chips = shares
        .filter((share) => share.machine === machine)
        .map((share) => chip(share.account, `data-act="unshare" data-machine="${escape(machine)}" data-name="${escape(share.account)}"`, words.takeBack(words.sharedLabel(machine, share.account))))
        .join("");
      return row(glyph(form === "laptop" ? "laptop" : form === "server" ? "server" : "desktop", 16), escape(machine), "", words.sharedTo, chips);
    });
    const logins = [...new Map(assists.map((assist) => [`${assist.machine}\n${assist.agent}\n${assist.home ?? ""}`, assist])).values()].map((first) => {
      const same = assists.filter((assist) => assist.machine === first.machine && assist.agent === first.agent && assist.home === first.home);
      const home = first.home ? (first.home.split(/[\\/]/).filter(Boolean).pop() ?? first.home) : "";
      const chips = same
        .map((assist) => {
          const until = assist.until ? `<span class="chip-more">${glyph("history", 12)}${escape(short(new Date(assist.until), now))}</span>` : "";
          const weekly = assist.weekly ? `<span class="chip-more">${escape(words.weeklyShort(wholeDollars(assist.weekly / 1_000_000)))}</span>` : "";
          return chip(assist.account, `data-act="unassist" data-assist="${escape(assistKey(assist))}"`, words.takeBack(words.assistLabel(`${agentName(assist.agent)} · ${assist.machine}`, assist.account)), until + weekly);
        })
        .join("");
      return row(glyph(first.agent === "claude" ? "claude" : "codex", 16), escape(agentName(first.agent)), escape([first.machine, home].filter(Boolean).join(" · ")), words.assisting, chips);
    });
    return part(words.given, "", `<ul class="me-rows">${[...machines, ...logins].join("")}</ul>`);
  }

  private reaching(login: Login, hub: HubAccounts, now: Date): string {
    const { words, when } = this.options;
    const reaching = hub.acting.filter((acting) => acting.account === login.account);
    if (!reaching.length) return "";
    const rows = reaching.map((acting) => {
      const owner = hub.members.find((member) => member.name === acting.machine)?.home ?? null;
      return row(
        glyph("shield", 16),
        escape(words.reachingLine(acting.machine, owner)),
        escape(words.reachingSince(when(new Date(acting.since), now))),
        "",
        `<button class="text-button quiet-button" type="button" data-act="stop-reaching" data-name="${escape(acting.machine)}">${escape(words.stopReaching)}</button>`,
      );
    });
    return part(words.reaching, words.reachingWhat, `<ul class="me-rows">${rows.join("")}</ul>`);
  }

  /** The phones, and any web AI, logged in to the account; a computer
   *  logged in is a machine, on the drawing. */
  private devices(login: Login, hub: HubAccounts, world: WorldNow, now: Date): string {
    const { words, when } = this.options;
    const devices = hub.members.filter((member) => member.kind !== "computer" && member.logins.includes(login.account));
    if (!devices.length) return "";
    const rows = devices.map((device: MemberSeen) => {
      const here = device.name === world.accounts.machine;
      const state = device.online ? words.onlineNow : device.since ? words.lastSeen(when(new Date(device.since), now)) : words.neverSeen;
      return row(
        glyph(device.kind === "phone" ? "phone" : "web", 16),
        `${escape(device.name)}${here ? `<span class="tag">${escape(words.thisOne)}</span>` : ""}`,
        escape([words.since(when(new Date(device.added), now)), state].join(" · ")),
        "",
        here ? "" : `<button class="text-button quiet-button" type="button" data-act="remove-device" data-name="${escape(device.name)}">${escape(words.removeDevice)}</button>`,
        device.online ? " online" : "",
      );
    });
    return part(words.devices(devices.some((device) => device.kind === "web")), "", `<ul class="me-rows">${rows.join("")}</ul>`);
  }
}

/** A part of the page: a quiet head ruled across, and what it holds. */
function part(title: string, what: string, body: string): string {
  return `<section class="me-part"><h3 class="picker-title"${what ? ` title="${escape(what)}"` : ""}>${escape(title)}</h3>${body}</section>`;
}

/** A row: its mark, its name with a quieter line beside it, what is said
 *  of the things at its end, and those things. */
function row(mark: string, name: string, sub: string, verb: string, end: string, state = ""): string {
  // A row that ends in one button keeps it on its line, even on a phone.
  const act = !verb && end.startsWith("<button") ? " act" : "";
  return `<li class="me-row${state}${act}"><span class="me-row-mark">${mark}</span><span class="me-row-name">${name}${sub ? `<span class="me-row-sub">${sub}</span>` : ""}</span><span class="me-row-end">${verb ? `<span class="me-row-verb">${escape(verb)}</span>` : ""}${end}</span></li>`;
}

function assistKey(assist: AssistSeen): string {
  return [assist.machine, assist.agent, assist.home ?? "", assist.account].join("\n");
}

/** A seal for the user: a ring drawn by hand round the first letter of
 *  their name. */
function seal(account: string): string {
  const ring = circle([24, 24], 20, 1.6, seeded(`seal:${account}`));
  return `<svg viewBox="0 0 48 48" width="48" height="48"><path class="me-seal-ring" d="${ring}"/><text x="24" y="30.5" text-anchor="middle">${escape(account.slice(0, 1).toUpperCase())}</text></svg>`;
}
