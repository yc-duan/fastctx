// The user's own things in the network shown (`world-gui.md`, 账号、机器、
// 共享与协助: 「我」): who they are there and their password, at the top with
// what waits for their word — a visitor asked to become a member, a device
// that came into the account lately, with "that wasn't me", managed
// machines that came in by one code said together. Under it three tabs that
// stay at the top as what is under them scrolls: what of theirs
// others may use (the machines shared and the logins assisting, each taken
// back with its cross and for a moment given back, and the machines of
// others whose agents may reach theirs); the devices logged in to the
// account and leaving the network; the codes that let devices in. A tab with
// nothing in it is not drawn. Other people are the boxes on the machines
// page's drawing.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { circle, seeded } from "../pen/shapes";
import { loginsOf, type AccountChange, type Arrival, type AssistSeen, type HubAccounts, type Login, type MemberSeen } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import { wholeDollars } from "../world/money";
import { agentName } from "../world/names";
import type { MachinesWords } from "./account-words";
import { assistChange, termsOf } from "./assist-sheet";
import { codeName, codeRow, codesOf } from "./code-rows";
import { Confirm, Sheet } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { row } from "./rows";
import { linkSaid } from "./machine-spot";
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

/** The page's tabs, in their order, and where this device keeps the one
 *  last open. */
const TABS = ["given", "devices", "codes"] as const;
type Tab = (typeof TABS)[number];
const TAB_KEY = "me.tab";

function keptTab(): Tab {
  try {
    const kept = localStorage.getItem(TAB_KEY);
    return TABS.find((tab) => tab === kept) ?? "given";
  } catch {
    return "given";
  }
}

export class MePage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly top: HTMLElement;
  private readonly tabs: HTMLElement;
  private readonly body: HTMLElement;
  private readonly sheet: HTMLElement;
  private readonly stop: () => void;
  private drawn = "";
  private tab: Tab = keptTab();
  private readonly password = new Sheet("device-sheet password-sheet");
  /** Chips taken back here, gone from the page before the server answers. */
  private readonly taking: Taking[] = [];

  constructor(private readonly options: MeOptions) {
    const { words } = options;
    this.title = words.title;
    this.element = document.createElement("div");
    this.element.className = "settings-page me-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(words.title)}</h1>`)}<div class="me scroller"><div class="me-top"></div><div class="st-tabs me-tabs" role="tablist" aria-label="${escape(words.title)}"></div><div class="st-sheet"></div></div>`;
    this.top = this.element.querySelector<HTMLElement>(".me-top")!;
    this.tabs = this.element.querySelector<HTMLElement>(".me-tabs")!;
    this.body = this.element.querySelector<HTMLElement>(".me")!;
    this.sheet = this.element.querySelector<HTMLElement>(".me .st-sheet")!;
    for (const part of [this.top, this.sheet]) part.addEventListener("click", (event) => this.click(event));
    this.tabs.addEventListener("click", (event) => {
      const tab = (event.target as HTMLElement).closest<HTMLElement>("[data-tab]")?.dataset.tab as Tab | undefined;
      if (tab) this.show(tab);
    });
    // The arrow keys go from tab to tab, as a row of tabs does.
    this.tabs.addEventListener("keydown", (event) => {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      const shown = [...this.tabs.querySelectorAll<HTMLElement>("[data-tab]")].map((button) => button.dataset.tab as Tab);
      const at = shown.indexOf(this.tab);
      const next = shown[(at + (event.key === "ArrowRight" ? 1 : shown.length - 1)) % shown.length];
      if (!next) return;
      event.preventDefault();
      this.show(next);
      this.tabs.querySelector<HTMLElement>(`[data-tab="${next}"]`)?.focus();
    });
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
        return void this.takeBack(login, [`share\n${machine}\n${name}`], words.sharedLabel(machine, name), [{ ...give, shared: false }], [give]);
      }
      case "unshare-group": {
        const group = button.dataset.group!;
        const hub = this.hubOf(login);
        const ours = new Set(hub?.members.filter((member) => member.home === login.account && member.group === group).map((member) => member.name));
        const machines = (hub?.shares ?? []).filter((share) => share.account === name && ours.has(share.machine)).map((share) => share.machine);
        const gives: AccountChange[] = machines.map((machine) => ({ change: "share", machine, to: name, shared: true }));
        return void this.takeBack(
          login,
          machines.map((machine) => `share\n${machine}\n${name}`),
          words.sharedLabel(words.groupOf(group, machines.length), name),
          gives.map((give) => ({ ...give, shared: false })),
          gives,
        );
      }
      case "unassist": {
        const assist = this.hubOf(login)?.assists.find((one) => assistKey(one) === button.dataset.assist);
        if (!assist) return;
        const give = assistChange(assist.machine, assist.agent, assist.home, assist.account, termsOf(assist));
        const take = assistChange(assist.machine, assist.agent, assist.home, assist.account, null);
        return void this.takeBack(login, [`assist\n${assistKey(assist)}`], words.assistLabel(`${agentName(assist.agent)} ${assist.machine}`, assist.account), [take], [give]);
      }
      case "leave":
        return this.leaveNetwork(login);
      case "accept":
      case "decline": {
        const accept = button.dataset.act === "accept";
        return void this.quietly(button, login, { change: "accept_role", accept }).then(() => this.options.say(accept ? words.accepted : words.declined));
      }
      case "revoke": {
        const token = this.hubOf(login)?.tokens?.find((one) => one.key === button.dataset.key);
        if (!token) return;
        const codes = words.codeRows;
        return confirm.ask({
          title: codes.revokeTitle(codeName(token, codes)),
          what: codes.revokeWhat,
          go: codes.revokeGo,
          act: () => change(login, { change: "revoke_token", token: token.key }),
          done: refresh,
        });
      }
      case "seen-code":
        return void this.byCode(login, button.dataset.code ?? "").reduce(
          (done, arrival) => done.then(() => change(login, { change: "seen", name: arrival.name })),
          Promise.resolve(),
        ).then(refresh, (error: unknown) => this.options.say(words.failed(error instanceof Error ? error.message : String(error))));
      case "not-me-code": {
        const code = button.dataset.code ?? "";
        const came = this.byCode(login, code);
        const token = this.hubOf(login)?.tokens?.find((one) => one.key === code);
        const named = token ? codeName(token, words.codeRows) : words.someCode;
        return confirm.ask({
          title: words.notMeCodeTitle(named, came.length),
          what: words.notMeCodeWhat(came.length),
          go: words.notMeCodeGo,
          act: async () => {
            // A code already closed has nothing left to revoke.
            if (token) await change(login, { change: "revoke_token", token: token.key });
            for (const arrival of came) {
              await change(login, { change: "remove", name: arrival.name, reason: "not me" });
              await change(login, { change: "seen", name: arrival.name });
            }
          },
          done: refresh,
        });
      }
    }
  }

  /** The managed machines that came into `login`'s account by `code`, not
   *  yet shown. */
  private byCode(login: Login, code: string): Arrival[] {
    return (this.hubOf(login)?.arrivals ?? []).filter((arrival) => arrival.account === login.account && arrival.code === code);
  }

  /** Takes back a share or an assistance: gone from the page at once, and
   *  for a moment given back from the line that says so. */
  /** Takes back what `takes` say, each shown gone by its key while it
   *  goes; what went can be given again from the word that says so. */
  private async takeBack(login: Login, keys: string[], label: string, takes: AccountChange[], gives: AccountChange[]): Promise<void> {
    const { words, change, store, say } = this.options;
    const taking = keys.map((key) => ({ hub: login.hub, key }));
    this.taking.push(...taking);
    this.redraw();
    const went: AccountChange[] = [];
    try {
      for (const [index, take] of takes.entries()) {
        await change(login, take);
        went.push(gives[index]!);
      }
    } catch (error) {
      const said = error instanceof Error ? error.message : String(error);
      say(words.failed(said), { detail: said });
      await store.refresh();
      return;
    } finally {
      for (const one of taking) this.taking.splice(this.taking.indexOf(one), 1);
    }
    say(words.tookBack(label), {
      undo: () => {
        void (async () => {
          for (const give of went) await change(login, give);
        })().then(
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
        // A phone in no network any more shows only the way in again.
        void store.refresh().then(() => {
          const left = !computer && !loginsOf(store.world.peek()?.accounts).length;
          location.hash = link(left ? { kind: "login" } : { kind: "agents", machine: null });
        });
      },
    });
  }

  private redraw(): void {
    this.drawn = "";
    this.draw(this.options.store.world.peek(), this.options.store.clock.peek());
  }

  /** Shows `tab`'s sheet, kept on this device for the next time. */
  private show(tab: Tab): void {
    this.tab = tab;
    try {
      localStorage.setItem(TAB_KEY, tab);
    } catch {
      // Kept for this visit only.
    }
    this.redraw();
    // Back to the top of the sheet, the tabs where they stand.
    const tabs = this.tabs.offsetTop;
    if (this.body.scrollTop > tabs) this.body.scrollTo({ top: tabs });
  }

  private draw(world: WorldNow | null, now: Date): void {
    if (!world) return;
    const { words } = this.options;
    const logins = loginsOf(world.accounts);
    const hubOf = (login: Login) => world.accounts.hubs.find((one) => one.hub === login.hub);
    const signed = logins.filter((login) => hubOf(login));
    // Each tab's sheet, a part for each login of this device where it holds
    // something, headed by the login's name where there are several.
    const sheets = new Map<Tab, string>(
      TABS.map((tab) => [
        tab,
        signed
          .map((login) => {
            const inside = this.pane(tab, login, hubOf(login)!, world, now);
            if (!inside) return "";
            const named = signed.length > 1 ? `<h3 class="me-login-name">${escape(login.account)}</h3>` : "";
            return `<section class="me-login" data-hub="${escape(login.hub)}" data-account="${escape(login.account)}">${named}${inside}</section>`;
          })
          .join(""),
      ]),
    );
    const shown = TABS.filter((tab) => sheets.get(tab));
    const tab = shown.includes(this.tab) ? this.tab : (shown[0] ?? "devices");
    const top = signed.length
      ? signed.map((login) => this.head(login, hubOf(login)!, world, now)).join("")
      : `<p class="me-none">${escape(words.none)} <a class="text-link" href="${link({ kind: "login" })}">${escape(words.logIn)}</a></p>`;
    // A row of tabs only where there is more than one.
    const tabs =
      shown.length > 1
        ? shown.map((one) => `<button class="st-tab" type="button" role="tab" data-tab="${one}" aria-selected="${one === tab}" tabindex="${one === tab ? 0 : -1}">${escape(words.tabs[one])}</button>`).join("")
        : "";
    const sheet = sheets.get(tab) ?? "";
    const html = `${top}\n${tabs}\n${sheet}`;
    if (html === this.drawn) return;
    this.drawn = html;
    redrawKeepingFocus(this.top, () => {
      this.top.innerHTML = top;
    });
    this.tabs.innerHTML = tabs;
    this.tabs.hidden = !tabs;
    this.sheet.hidden = !sheet;
    this.sheet.classList.toggle("first", !!tabs && tab === shown[0]);
    redrawKeepingFocus(this.sheet, () => {
      this.sheet.innerHTML = sheet;
    });
  }

  /** What stays at the top for a login: an ask to become a member, who the
   *  user is, and the devices that came in lately. */
  private head(login: Login, hub: HubAccounts, world: WorldNow, now: Date): string {
    const { words } = this.options;
    const me = hub.accounts.find((account) => account.name === login.account);
    return `<section class="me-login" data-hub="${escape(login.hub)}" data-account="${escape(login.account)}">
      ${me?.promotion ? this.promotion(login, world) : ""}
      ${this.card(login, hub, world)}
      ${me?.level === "visitor" ? `<p class="me-note">${escape(words.visitorGives)}</p>` : ""}
      ${this.arrivals(hub.arrivals.filter((arrival) => arrival.account === login.account), hub, world, now)}
    </section>`;
  }

  /** A tab's part for a login, or nothing where it holds nothing; leaving
   *  the network is always on the devices' sheet, last. */
  private pane(tab: Tab, login: Login, hub: HubAccounts, world: WorldNow, now: Date): string {
    const { words } = this.options;
    if (tab === "given") return this.given(login, hub, now) + this.reaching(login, hub, now);
    if (tab === "codes") return this.codes(login, hub, world, now);
    const computer = isComputer(world);
    return `${this.devices(login, hub, world, now)}<div class="me-leave"><button class="text-button danger-text" type="button" data-act="leave">${glyph("logout", 16)}<span>${escape(computer ? words.leave : words.logout)}</span></button></div>`;
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
      me ? `<span${me.level === "visitor" ? ` title="${escape(words.visitorNote)}"` : ""}>${escape(words.level[me.level])}</span>` : "",
      `<span>${escape(hub.name)}</span>`,
      // The administrator may use a member's machines, never a visitor's.
      admin && admin !== login.account ? `<span class="me-admin"${me?.level === "visitor" ? "" : ` title="${escape(words.superNote(admin))}"`}>${glyph("shield", 13)}${escape(words.adminIs(admin))}</span>` : "",
      hub.link.kind === "alone" ? "" : `<span class="me-link ${hub.link.kind}" title="${escape(linkSaid(hub.link, machines))}"><i aria-hidden="true"></i>${escape(state)}</span>`,
    ].filter(Boolean);
    return `<header class="me-card">
      <span class="me-seal" aria-hidden="true">${seal(login.account)}</span>
      <div class="me-who"><h2 class="me-name">${escape(login.account)}</h2><p class="me-facts">${facts.join("")}</p></div>
      <button class="text-button quiet-button me-password" type="button" data-act="password">${glyph("key", 16)}<span>${escape(words.changePassword)}</span></button>
    </header>`;
  }

  /** The administrator asks a visitor to become a member: what it means,
   *  and the two answers, first on the page. */
  private promotion(login: Login, world: WorldNow): string {
    const { words } = this.options;
    const admin = superOf(world, login) ?? "";
    return `<div class="me-promotion" role="alert">
      <div class="me-promotion-text"><b>${escape(words.promotionTitle(admin))}</b><span>${escape(words.promotionWhat)}</span></div>
      <div class="me-promotion-acts"><button class="text-button quiet-button" type="button" data-act="decline">${escape(words.decline)}</button><button class="sheet-go" type="button" data-act="accept">${escape(words.accept)}</button></div>
    </div>`;
  }

  /** The codes that still let devices into this account, of every way in,
   *  and for the administrator every account's: each with how many came
   *  in, until when, and a way to revoke it (`code-rows`). */
  private codes(login: Login, hub: HubAccounts, world: WorldNow, now: Date): string {
    const { words, when } = this.options;
    const codes = codesOf(hub, login, now);
    if (!codes.length) return "";
    const rows = codes.map((token) => codeRow(token, login, world.accounts.machine, words.codeRows, (at) => when(at, now)));
    return `<ul class="me-rows">${rows.join("")}</ul>`;
  }

  private arrivals(all: Arrival[], hub: HubAccounts, world: WorldNow, now: Date): string {
    if (!all.length) return "";
    const { words, when } = this.options;
    const facts = factsByMachine(world);
    // Managed machines that came in by one code are one line: a code given
    // out lets a batch in.
    const codes = [...new Set(all.flatMap((arrival) => (arrival.code ? [arrival.code] : [])))];
    const batches = codes.map((code) => {
      const came = all.filter((arrival) => arrival.code === code);
      const last = came.reduce((late, arrival) => (arrival.noticed_at > late ? arrival.noticed_at : late), came[0]!.noticed_at);
      const token = hub.tokens?.find((one) => one.key === code);
      const named = token ? codeName(token, words.codeRows) : words.someCode;
      return `<li class="me-arrival">${glyph("machine", 16)}<span class="me-arrival-text" title="${escape(came.map((arrival) => arrival.name).join(words.separator))}">${escape(words.arrivedByCode(named, came.length, when(new Date(last), now)))}</span><span class="me-arrival-acts"><button class="text-button quiet-button" type="button" data-act="seen-code" data-code="${escape(code)}">${escape(words.me)}</button><button class="text-button quiet-button danger" type="button" data-act="not-me-code" data-code="${escape(code)}">${escape(words.notMe)}</button></span></li>`;
    });
    const arrivals = all.filter((arrival) => !arrival.code);
    return `<ul class="me-arrivals" aria-live="polite">${batches.join("")}${arrivals
      .map((arrival) => {
        const form = arrival.device ? undefined : facts.get(arrival.name)?.form;
        const kind = form ? words.forms[form] : words.kinds[arrival.device ? "phone" : "computer"];
        return `<li class="me-arrival">${glyph(arrival.device ? "phone" : "machine", 16)}<span class="me-arrival-text">${escape(words.arrived(kind, arrival.name, when(new Date(arrival.noticed_at), now)))}</span><span class="me-arrival-acts"><button class="text-button quiet-button" type="button" data-act="seen" data-name="${escape(arrival.name)}">${escape(words.me)}</button><button class="text-button quiet-button danger" type="button" data-act="not-me" data-name="${escape(arrival.name)}">${escape(words.notMe)}</button></span></li>`;
      })
      .join("")}</ul>`;
  }

  /** The user's machines shared with others and their logins assisting
   *  others: a row for each account given anything, what it was given as
   *  chips to take back. Machines of one group given to it are one chip, as
   *  a room of machines shared one by one would otherwise run on and on. */
  private given(login: Login, hub: HubAccounts, now: Date): string {
    const { words, short } = this.options;
    const mine = new Map(hub.members.filter((member) => member.home === login.account).map((member) => [member.name, member]));
    const taken = (key: string) => this.taking.some((one) => one.hub === hub.hub && one.key === key);
    const shares = hub.shares.filter((share) => mine.has(share.machine) && !taken(`share\n${share.machine}\n${share.account}`));
    const assists = hub.assists.filter((assist) => mine.has(assist.machine) && !taken(`assist\n${assistKey(assist)}`));
    if (!shares.length && !assists.length) return "";
    const chip = (label: string, take: string, title: string, more = "", tip = "", mark = "") =>
      `<span class="chip"${tip ? ` title="${escape(tip)}"` : ""}>${mark}<span class="me-chip-label">${escape(label)}</span>${more}<button class="chip-x" type="button" ${take} aria-label="${escape(title)}" title="${escape(title)}">${glyph("close", 12)}</button></span>`;
    const accounts = [...new Set([...shares.map((share) => share.account), ...assists.map((assist) => assist.account)])].sort((a, b) => a.localeCompare(b));
    const rows = accounts.map((account) => {
      const machines = shares.filter((share) => share.account === account).map((share) => share.machine);
      const grouped = new Map<string, string[]>();
      const alone: string[] = [];
      for (const machine of machines) {
        const group = mine.get(machine)?.group;
        if (group) grouped.set(group, [...(grouped.get(group) ?? []), machine]);
        else alone.push(machine);
      }
      for (const [group, members] of grouped) {
        if (members.length > 1) continue;
        grouped.delete(group);
        alone.push(...members);
      }
      const groupChips = [...grouped].map(([group, members]) =>
        chip(
          group,
          `data-act="unshare-group" data-group="${escape(group)}" data-name="${escape(account)}"`,
          words.takeBack(words.sharedLabel(words.groupOf(group, members.length), account)),
          `<span class="chip-more">${members.length}</span>`,
          members.join(words.separator),
        ),
      );
      const machineChips = alone.map((machine) =>
        chip(machine, `data-act="unshare" data-machine="${escape(machine)}" data-name="${escape(account)}"`, words.takeBack(words.sharedLabel(machine, account))),
      );
      const loginChips = assists
        .filter((assist) => assist.account === account)
        .map((assist) => {
          const home = assist.home ? (assist.home.split(/[\\/]/).filter(Boolean).pop() ?? assist.home) : "";
          // Whose login it is, by its mark: the machine and the folder are
          // what tell one from another. One that ends has a small clock;
          // until when, and how much a day and a week, are said when it is
          // pointed at.
          const label = [assist.machine, home].filter(Boolean).join(" ");
          const whose = agentName(assist.agent);
          const ends = assist.until ? `<span class="chip-more">${glyph("history", 12)}</span>` : "";
          const said = [
            `${whose} ${label}`,
            assist.until ? words.untilShort(short(new Date(assist.until), now)) : "",
            assist.daily ? words.dailyShort(wholeDollars(assist.daily / 1_000_000)) : "",
            assist.weekly ? words.weeklyShort(wholeDollars(assist.weekly / 1_000_000)) : "",
          ].filter(Boolean);
          return chip(label, `data-act="unassist" data-assist="${escape(assistKey(assist))}"`, words.takeBack(words.assistLabel(`${whose} ${assist.machine}`, account)), ends, said.join(" "), glyph(assist.agent === "codex" ? "codex" : "claude", 14));
        });
      return row(glyph("person", 16), escape(account), "", "", [...groupChips, ...machineChips, ...loginChips].join(""));
    });
    return part(words.given, "", `<ul class="me-rows">${rows.join("")}</ul>`);
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
        "",
        "",
        `<button class="text-button quiet-button" type="button" data-act="stop-reaching" data-name="${escape(acting.machine)}">${escape(words.stopReaching)}</button>`,
        "",
        words.reachingSince(when(new Date(acting.since), now)),
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
        escape([words.since(when(new Date(device.added), now)), state].join(" ")),
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
function assistKey(assist: AssistSeen): string {
  return [assist.machine, assist.agent, assist.home ?? "", assist.account].join("\n");
}

/** A seal for the user: a ring drawn by hand round the first letter of
 *  their name. */
function seal(account: string): string {
  const ring = circle([24, 24], 20, 1.6, seeded(`seal:${account}`));
  return `<svg viewBox="0 0 48 48" width="48" height="48"><path class="me-seal-ring" d="${ring}"/><text x="24" y="30.5" text-anchor="middle">${escape(account.slice(0, 1).toUpperCase())}</text></svg>`;
}
