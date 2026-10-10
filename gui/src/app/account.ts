// The accounts this computer or device is logged in to (`world-gui.md`,
// 账号、机器、共享与借出), one after another: the hub and how it is reached,
// and whether the account is its super administrator or a member, a member
// told what the super administrator may do; who came into the account
// lately, each with "that wasn't me"; the devices logged in, each
// removable; the machines of others whose agents may reach the account's
// machines, since it used one there, each to take back; letting another
// device in with a one-time token; the account's projects and whom they are
// shared with; its password; logging out; and, for the super administrator,
// the way to the hub's page.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { loginsOf, type AccountChange, type Arrival, type HubAccounts, type Login, type MemberSeen } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { AccountWords, MachinesWords } from "./account-words";
import { Confirm, Sheet } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { linkSaid } from "./machines";
import { link } from "./places";
import { loginLabel, superOf } from "./reach";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";
import type { TokenKind } from "./tokens";

export interface AccountOptions {
  store: Store;
  words: AccountWords;
  machines: MachinesWords;
  head(title: string, actions?: string): string;
  /** When, as the front page says it. */
  when(at: Date, now: Date): string;
  /** Makes a change as a login. */
  change(login: Login, change: AccountChange): Promise<void>;
  /** Opens the sheet that lets another device into `login`. */
  invite(login: Login, kind: TokenKind): void;
  confirm: Confirm;
}

const accountRule = "[a-z][a-z0-9\\-]{0,63}";

export class AccountPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly list: HTMLElement;
  private readonly stop: () => void;
  private drawn = "";
  private readonly password = new Sheet("device-sheet password-sheet");
  private readonly sharing = new Sheet("device-sheet share-sheet");
  /** What came of the last change, under the login it was made as. */
  private said: { login: string; text: string } | null = null;

  constructor(private readonly options: AccountOptions) {
    const { words } = options;
    this.title = words.title;
    this.element = document.createElement("div");
    this.element.className = "settings-page account-page";
    this.element.innerHTML = `
      ${options.head(
        `<h1 class="page-title">${escape(words.title)}</h1>`,
        `<a class="text-button" href="${link({ kind: "login" })}">${glyph("plus", 16)}<span>${escape(words.another)}</span></a>`,
      )}
      <div class="settings account scroller"></div>`;
    this.list = this.element.querySelector<HTMLElement>(".account")!;
    this.list.addEventListener("click", (event) => this.click(event));
    this.stop = effect(() => this.draw(options.store.world.get(), options.store.clock.get()));
  }

  leave(): void {
    this.stop();
  }

  private loginAt(element: HTMLElement): Login | undefined {
    const at = element.closest<HTMLElement>("[data-hub]");
    return loginsOf(this.options.store.world.peek()?.accounts).find((l) => l.hub === at?.dataset.hub && l.account === at?.dataset.account);
  }

  private click(event: Event): void {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]");
    if (!button || button.disabled) return;
    const login = this.loginAt(button);
    if (!login) return;
    const { words } = this.options;
    const name = button.dataset.name ?? "";
    switch (button.dataset.act) {
      case "seen":
        void this.quietly(button, login, { change: "seen", name });
        return;
      case "not-me":
        this.options.confirm.ask({
          title: words.removeDeviceTitle(name),
          what: words.notMeWhat(name),
          go: words.removeDeviceGo,
          act: async () => {
            await this.options.change(login, { change: "remove", name, reason: "not me" });
            await this.options.change(login, { change: "seen", name });
          },
          done: () => void this.options.store.refresh(),
        });
        return;
      case "remove-device":
        this.options.confirm.ask({
          title: words.removeDeviceTitle(name),
          what: words.removeDeviceWhat(name, login.account, button.dataset.machine === "1"),
          go: words.removeDeviceGo,
          act: () => this.options.change(login, { change: "remove", name, reason: "removed" }),
          done: () => void this.options.store.refresh(),
        });
        return;
      case "invite":
        this.options.invite(login, button.dataset.kind as TokenKind);
        return;
      case "stop-reaching": {
        const owner = this.hubOf(login)?.members.find((member) => member.name === name)?.home ?? null;
        this.options.confirm.ask({
          title: words.stopReachingTitle(name),
          what: words.stopReachingWhat(name, owner),
          go: words.stopReachingGo,
          act: () => this.options.change(login, { change: "stop_acting", machine: name }),
          done: () => void this.options.store.refresh(),
        });
        return;
      }
      case "unshare":
        void this.quietly(button, login, { change: "share_project", town: button.dataset.town!, with: name, shared: false });
        return;
      case "share":
        this.share(login, button.dataset.town!);
        return;
      case "password":
        this.changePassword(login);
        return;
      case "logout": {
        const registered = this.hubOf(login)?.home === login.account;
        this.options.confirm.ask({
          title: words.logoutTitle(loginLabel(login)),
          what: words.logoutWhat(loginLabel(login), registered),
          go: words.logoutGo,
          act: () => this.options.change(login, { change: "logout" }),
          done: () => void this.options.store.refresh(),
        });
        return;
      }
    }
  }

  private hubOf(login: Login): HubAccounts | null {
    return this.options.store.world.peek()?.accounts.hubs.find((hub) => hub.hub === login.hub) ?? null;
  }

  /** A change made at once, with nothing to read first. */
  private async quietly(button: HTMLButtonElement, login: Login, change: AccountChange): Promise<void> {
    button.disabled = true;
    try {
      await this.options.change(login, change);
      this.said = null;
    } catch (error) {
      this.said = { login: loginLabel(login), text: this.options.words.failed(error instanceof Error ? error.message : String(error)) };
    }
    await this.options.store.refresh();
    this.drawn = "";
    this.draw(this.options.store.world.peek(), this.options.store.clock.peek());
  }

  /** A form on a sheet: one button, the sheet open with why until the
   *  World does it. */
  private form(sheet: Sheet, title: string, fields: string, go: string, act: (form: HTMLElement) => Promise<string | void>): void {
    const { words } = this.options;
    sheet.body.innerHTML = `
      ${sheet.head(title, words.close)}
      ${fields}
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(go)}</button></footer>`;
    sheet.body.onsubmit = (event) => {
      event.preventDefault();
      if (sheet.busy) return;
      const button = sheet.body.querySelector<HTMLButtonElement>(".sheet-go")!;
      const note = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
      sheet.busy = true;
      button.disabled = true;
      button.textContent = words.working;
      note.hidden = true;
      act(sheet.body)
        .then((wrong) => {
          sheet.busy = false;
          button.disabled = false;
          button.textContent = go;
          if (wrong) {
            note.textContent = wrong;
            note.hidden = false;
            return;
          }
          sheet.close();
          void this.options.store.refresh();
        })
        .catch((error: unknown) => {
          sheet.busy = false;
          button.disabled = false;
          button.textContent = go;
          note.textContent = words.failed(error instanceof Error ? error.message : String(error));
          note.hidden = false;
        });
    };
    sheet.show();
    sheet.body.querySelector<HTMLInputElement>("input")?.focus();
  }

  private changePassword(login: Login): void {
    const { words } = this.options;
    this.form(
      this.password,
      words.passwordTitle,
      `<p class="confirm-what">${escape(words.passwordWhat)}</p>
      <label class="field"><span class="field-label">${escape(words.newPassword)}</span><input name="password" type="password" required autocomplete="new-password"></label>
      <label class="field"><span class="field-label">${escape(words.passwordAgain)}</span><input name="again" type="password" required autocomplete="new-password"></label>`,
      words.passwordGo,
      async (form) => {
        const password = form.querySelector<HTMLInputElement>("[name=password]")!.value;
        if (password !== form.querySelector<HTMLInputElement>("[name=again]")!.value) return words.passwordsDiffer;
        await this.options.change(login, { change: "set_password", target: login.account, password });
        this.said = { login: loginLabel(login), text: words.passwordDone };
      },
    );
  }

  private share(login: Login, town: string): void {
    const { words } = this.options;
    this.form(
      this.sharing,
      words.shareTitle(this.options.store.world.peek()?.towns.find((t) => t.town === town)?.record.name ?? town),
      `<label class="field"><span class="field-label">${escape(words.shareWith)}</span><input name="with" required autocomplete="off" spellcheck="false" autocapitalize="off" pattern="${accountRule}"></label>`,
      words.shareGo,
      async (form) => {
        const to = form.querySelector<HTMLInputElement>("[name=with]")!.value.trim();
        await this.options.change(login, { change: "share_project", town, with: to, shared: true });
      },
    );
  }

  private draw(world: WorldNow | null, now: Date): void {
    if (!world) return;
    const { words } = this.options;
    const logins = loginsOf(world.accounts);
    const html = logins.length
      ? logins.map((login) => this.section(login, world, now)).join("")
      : `<section class="home-section"><p class="quiet">${escape(words.none)} <a class="text-link" href="${link({ kind: "login" })}">${escape(words.logIn)}</a></p></section>`;
    if (html === this.drawn) return;
    this.drawn = html;
    redrawKeepingFocus(this.list, () => {
      this.list.innerHTML = html;
    });
  }

  private section(login: Login, world: WorldNow, now: Date): string {
    const { words } = this.options;
    const hub = world.accounts.hubs.find((hub) => hub.hub === login.hub);
    if (!hub) return "";
    const me = hub.accounts.find((account) => account.name === login.account);
    const when = (at: string) => this.options.when(new Date(at), now);
    const linked = linkSaid(hub.link, this.options.machines);
    const facts: [string, string][] = [
      [words.hub, [hub.name, linked].filter(Boolean).join(" · ")],
      [words.account, [login.account, me ? words.level[me.level] : ""].filter(Boolean).join(" · ")],
    ];
    const admin = superOf(world, login);
    const superNote = admin && admin !== login.account ? `<p class="machines-note account-super">${glyph("shield", 14)}<span>${escape(words.superNote(admin))}</span></p>` : "";
    const reaching = hub.acting.filter((acting) => acting.account === login.account);
    const arrivals = hub.arrivals.filter((arrival) => arrival.account === login.account);
    const devices = hub.members.filter((member) => member.logins.includes(login.account));
    const projects = hub.projects.filter((project) => project.owner === login.account || project.granted.includes(login.account));
    const said = this.said?.login === loginLabel(login) ? `<p class="settings-said" role="status">${escape(this.said.text)}</p>` : "";
    const kinds: TokenKind[] = ["phone", "computer", "server"];
    const inviteLabel = (kind: TokenKind) => (kind === "phone" ? words.invitePhone : kind === "computer" ? words.inviteComputer : words.inviteServer);
    const inviteGlyph = (kind: TokenKind) => glyph(kind === "phone" ? "phone" : kind === "computer" ? "machine" : "command", 16);
    return `<section class="home-section account-section" data-hub="${escape(login.hub)}" data-account="${escape(login.account)}">
      <h2 class="section-title account-title">${glyph("person", 18)}<span>${escape(loginLabel(login))}</span></h2>
      <dl class="facts">${facts.map(([name, value]) => `<dt>${escape(name)}</dt><dd>${escape(value)}</dd>`).join("")}</dl>
      ${superNote}
      ${said}
      ${arrivals.length ? `<ul class="joined-lines" aria-live="polite">${arrivals.map((arrival) => this.arrival(arrival, now)).join("")}</ul>` : ""}
      <h3 class="account-part" title="${escape(words.devicesWhat)}">${escape(words.devices)}</h3>
      <ul class="account-devices">${devices.map((device) => this.device(device, login, world, when)).join("")}</ul>
      <h3 class="account-part" title="${escape(words.reachingWhat)}">${escape(words.reaching)}</h3>
      ${
        reaching.length
          ? `<ul class="account-reaching">${reaching
              .map((acting) => {
                const owner = hub.members.find((member) => member.name === acting.machine)?.home ?? null;
                return `<li class="account-reach">
                  <span class="reach-mark" aria-hidden="true">${glyph("shield", 15)}</span>
                  <span class="account-device-text"><span class="account-device-name">${escape(words.reachingLine(acting.machine, owner))}</span><span class="account-device-facts">${escape(words.reachingSince(when(acting.since)))}</span></span>
                  <button class="text-button quiet-button" type="button" data-act="stop-reaching" data-name="${escape(acting.machine)}">${escape(words.stopReaching)}</button>
                </li>`;
              })
              .join("")}</ul>`
          : `<p class="account-empty">${escape(words.reachingNone)}</p>`
      }
      <h3 class="account-part" title="${escape(words.inviteWhat)}">${escape(words.invite)}</h3>
      <div class="account-acts">${kinds
        .map((kind) => `<button class="text-button" type="button" data-act="invite" data-kind="${kind}">${inviteGlyph(kind)}<span>${escape(inviteLabel(kind))}</span></button>`)
        .join("")}</div>
      <h3 class="account-part">${escape(words.projects)}</h3>
      ${
        projects.length
          ? `<ul class="account-projects">${projects
              .map((project) => {
                const name = world.towns.find((town) => town.town === project.name)?.record.name ?? project.name;
                const owned = project.owner === login.account;
                return `<li class="account-project">
                  <a class="account-project-name" href="${link({ kind: "project", town: project.name })}">${escape(name)}</a>
                  ${
                    owned
                      ? `<span class="account-shared">${project.granted.length ? `<span class="field-hint">${escape(words.sharedWith)}</span>${project.granted
                          .map(
                            (account) =>
                              `<span class="chip">${escape(account)}<button class="chip-x" type="button" data-act="unshare" data-town="${escape(project.name)}" data-name="${escape(account)}" aria-label="${escape(words.unshare(account))}" title="${escape(words.unshare(account))}">${glyph("close", 12)}</button></span>`,
                          )
                          .join("")}` : ""}<button class="text-button quiet-button" type="button" data-act="share" data-town="${escape(project.name)}">${escape(words.share)}</button></span>`
                      : `<span class="field-hint">${escape(words.sharedBy(project.owner))}</span>`
                  }
                </li>`;
              })
              .join("")}</ul>`
          : `<p class="field-hint">${escape(words.noProjects)}</p>`
      }
      <h3 class="account-part">${escape(words.password)}</h3>
      <div class="account-acts">
        <button class="text-button" type="button" data-act="password">${glyph("key", 16)}<span>${escape(words.changePassword)}</span></button>
        ${me?.level === "super" && !me.locked ? `<a class="text-button" href="${link({ kind: "admin" })}">${glyph("settings", 16)}<span>${escape(words.adminPage)}</span></a>` : ""}
        <button class="text-button account-logout" type="button" data-act="logout">${glyph("logout", 16)}<span>${escape(words.logout)}</span></button>
      </div>
    </section>`;
  }

  private arrival(arrival: Arrival, now: Date): string {
    const { words } = this.options;
    const kind = words.kinds[arrival.device ? "phone" : "computer"];
    return `<li class="joined-line">
      <span class="joined-text">${escape(words.arrived(kind, arrival.name, this.options.when(new Date(arrival.noticed_at), now)))}</span>
      <span class="joined-acts"><button class="text-button quiet-button" type="button" data-act="seen" data-name="${escape(arrival.name)}">${escape(words.me)}</button><button class="text-button quiet-button danger" type="button" data-act="not-me" data-name="${escape(arrival.name)}">${escape(words.notMe)}</button></span>
    </li>`;
  }

  private device(device: MemberSeen, login: Login, world: WorldNow, when: (at: string) => string): string {
    const { words } = this.options;
    const here = device.name === world.accounts.machine;
    const machine = device.home === login.account;
    const state = device.online ? words.onlineNow : device.since ? words.lastSeen(when(device.since)) : words.neverSeen;
    return `<li class="account-device ${device.online ? "online" : "offline"}">
      <span class="dot" aria-hidden="true"></span>
      <span class="account-device-text">
        <span class="account-device-name">${escape(device.name)}${here ? `<span class="tag">${escape(words.thisOne)}</span>` : ""}${machine ? `<span class="tag">${escape(words.machineTag)}</span>` : ""}</span>
        <span class="account-device-facts">${[words.kinds[device.kind], words.added(when(device.added)), state, words.key(device.key)]
          .map((fact) => `<span class="account-fact">${escape(fact)}</span>`)
          .join(`<span class="sep">·</span>`)}</span>
      </span>
      ${here ? "" : `<button class="text-button quiet-button" type="button" data-act="remove-device" data-name="${escape(device.name)}" data-machine="${machine ? "1" : "0"}">${escape(words.removeDevice)}</button>`}
    </li>`;
  }
}
