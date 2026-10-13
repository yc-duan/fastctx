// The people of a network on the machines page (`world-gui.md`, 账号、机器、
// 共享与协助), for its administrator: adding a person from the dashed box
// after the people's boxes, and each person's "⋯" — resetting their
// password, locking and unlocking them, handing them the administrator's
// part, removing them. What is done shows on the drawing at once: a person
// added stands in a box of their own before the server has answered, a
// person locked fades; what the server refuses goes back and says why.
// Whether anyone may sign up by themselves is the server's switch among its
// own actions.

import { escape } from "../conversation/prose";
import type { AccountChange, HubAccounts, Login } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { Confirm } from "./confirm";
import type { MapPerson } from "./map-nodes";
import { newPasswordFields, passwordReady, sayPassword } from "./new-password";
import type { PeopleWords } from "./people-words";
import { isSuper } from "./reach";
import type { Store } from "./store";

/** What the machines page lends the people: its cards beside the drawing,
 *  its moment to undo, and drawing again. */
export interface PeopleHost {
  words: PeopleWords;
  store: Store;
  confirm: Confirm;
  change(login: Login, change: AccountChange): Promise<void>;
  /** Opens a card beside `key`; `go` runs when it is confirmed, and the
   *  card stays open when it answers "keep". */
  card(key: string, html: string, go: () => Promise<void | "keep">): HTMLElement;
  undo(said: string, redo: () => Promise<void>): void;
  /** Says on the page what went wrong. */
  failed(said: string): void;
  redraw(): void;
  /** A person's box's name, as a machine of theirs is grouped. */
  ownedBy(account: string): string;
}

/** One of a person's actions, as their "⋯" lists it. */
export interface PersonAct {
  act: "reset" | "lock" | "admin" | "remove";
  label: string;
  icon: "key" | "lock" | "unlock" | "shield" | "trash";
  danger?: boolean;
}

/** A name an account may have: what the server takes. */
const ACCOUNT = /^[a-z][a-z0-9-]{0,63}$/;

export class MapPeople {
  /** People added here that the server has not answered for yet. */
  private readonly adding: MapPerson[] = [];
  /** Whether a person is locked, as changed here until the server says so. */
  private readonly locks = new Map<string, boolean>();

  constructor(private readonly host: PeopleHost) {}

  /** The people as the drawing shows them: those the server knows, those
   *  being added, and each as locked as it was last made. */
  shown(people: readonly MapPerson[]): MapPerson[] {
    const known = people.map((person) => {
      const locked = this.locks.get(keyOf(person.hub, person.account));
      if (locked === person.locked) this.locks.delete(keyOf(person.hub, person.account));
      return locked === undefined ? person : { ...person, locked };
    });
    const coming = this.adding.filter((one) => !people.some((person) => person.hub === one.hub && person.account === one.account));
    return [...known, ...coming];
  }

  /** The hubs whose administrator this device is logged in as. */
  hubs(world: WorldNow): string[] {
    return loginsOf(world.accounts)
      .filter((login) => isSuper(world, login))
      .map((login) => login.hub);
  }

  /** What may be done to `account`, where this device is the
   *  administrator of `hub`. */
  acts(world: WorldNow, hub: string, account: string): PersonAct[] {
    const { words } = this.host;
    if (!this.admin(world, hub)) return [];
    const locked = this.lockedOf(world, hub, account);
    return [
      { act: "reset", label: words.resetPassword, icon: "key" },
      { act: "lock", label: locked ? words.unlock : words.lock, icon: locked ? "unlock" : "lock" },
      ...(locked ? [] : [{ act: "admin" as const, label: words.makeAdmin, icon: "shield" as const }]),
      { act: "remove", label: words.removePerson, icon: "trash", danger: true },
    ];
  }

  /** Does `act` to `account` on `hub`; `key` is where its card stands. */
  run(act: string, hub: string, account: string, key: string): void {
    const world = this.host.store.world.peek();
    const at = world ? this.admin(world, hub) : null;
    if (!world || !at) return;
    switch (act) {
      case "reset":
        return this.reset(at.login, account, key);
      case "lock":
        return void this.lock(at.login, hub, account, !this.lockedOf(world, hub, account));
      case "admin":
        return this.makeAdmin(at.login, account);
      case "remove":
        return this.remove(at.login, at.hub, account);
    }
  }

  /** The card that adds a person on `hub`: their name and first password.
   *  Pressed, the person's box stands on the drawing at once; once the
   *  server has them, the card offers what they log in with. */
  add(hub: string, key: string): void {
    const world = this.host.store.world.peek();
    const at = world ? this.admin(world, hub) : null;
    if (!at) return;
    const { words } = this.host;
    const card = this.host.card(
      key,
      `<p class="map-card-title">${escape(words.addTitle)}</p>
       <label class="field"><span class="field-label">${escape(words.name)}</span><input name="name" required autocomplete="off" spellcheck="false" autocapitalize="off"><span class="field-hint">${escape(words.nameHint)}</span></label>
       ${newPasswordFields(words, { pair: false })}
       <p class="map-card-wrong" role="alert" hidden></p>
       <footer class="map-card-acts"><button class="text-button" type="button" data-card="cancel">${escape(words.cancel)}</button><button class="text-button map-card-go" type="submit">${escape(words.addGo)}</button></footer>`,
      async () => {
        const name = card.querySelector<HTMLInputElement>("[name=name]")!.value.trim();
        const wrong = card.querySelector<HTMLElement>(".map-card-wrong")!;
        if (!ACCOUNT.test(name)) {
          wrong.textContent = words.nameHint;
          wrong.hidden = false;
          return "keep";
        }
        if (!passwordReady(card, words, name)) return "keep";
        const password = card.querySelector<HTMLInputElement>("[name=password]")!.value;
        const person: MapPerson = { hub, account: name, label: this.host.ownedBy(name), locked: false };
        this.adding.push(person);
        this.host.redraw();
        try {
          await this.host.change(at.login, { change: "open_account", name, password });
        } finally {
          this.adding.splice(this.adding.indexOf(person), 1);
        }
        await this.host.store.refresh();
        this.host.redraw();
        this.added(card, at.hub, name);
        return "keep";
      },
    );
    // The name counts against the password as it is typed.
    card.oninput = () => sayPassword(card, words, card.querySelector<HTMLInputElement>("[name=name]")?.value.trim() ?? "");
    card.querySelector<HTMLInputElement>("[name=name]")?.focus();
  }

  /** The card once a person is added: where they log in, to be copied and
   *  passed on. */
  private added(card: HTMLElement, hub: HubAccounts, account: string): void {
    const { words } = this.host;
    const address = hub.addresses[0] ?? hub.name;
    card.oninput = null;
    card.innerHTML = `<p class="map-card-title">${escape(words.added(account))}</p>
      <dl class="map-card-facts"><dt>${escape(words.hub)}</dt><dd class="mono">${escape(address)}</dd><dt>${escape(words.account)}</dt><dd class="mono">${escape(account)}</dd></dl>
      <footer class="map-card-acts"><button class="text-button" type="button" data-card="copy">${escape(words.copyLogin)}</button><button class="text-button map-card-go" type="button" data-card="cancel">${escape(words.done)}</button></footer>`;
    card.querySelector<HTMLButtonElement>("[data-card=copy]")!.addEventListener("click", (event) => {
      const button = event.currentTarget as HTMLButtonElement;
      void navigator.clipboard.writeText(words.loginText(address, account)).then(() => {
        button.textContent = words.copied;
      });
    });
    card.querySelector<HTMLButtonElement>("[data-card=cancel]")?.focus();
  }

  /** The card that sets a person's password anew. */
  private reset(login: Login, account: string, key: string): void {
    const { words } = this.host;
    const card = this.host.card(
      key,
      `<p class="map-card-title">${escape(words.resetTitle(account))}</p>
       <p class="map-card-said">${escape(words.resetWhat)}</p>
       ${newPasswordFields(words, { pair: false })}
       <p class="map-card-wrong" role="alert" hidden></p>
       <footer class="map-card-acts"><button class="text-button" type="button" data-card="cancel">${escape(words.cancel)}</button><button class="text-button map-card-go" type="submit">${escape(words.resetGo)}</button></footer>`,
      async () => {
        if (!passwordReady(card, words, account)) return "keep";
        const password = card.querySelector<HTMLInputElement>("[name=password]")!.value;
        await this.host.change(login, { change: "set_password", target: account, password });
      },
    );
    card.oninput = () => sayPassword(card, words, account);
    card.querySelector<HTMLInputElement>("[name=password]")?.focus();
  }

  /** Locks a person or lets them in again, at once; for a moment it can be
   *  undone. */
  private async lock(login: Login, hub: string, account: string, locked: boolean): Promise<void> {
    const { words } = this.host;
    const key = keyOf(hub, account);
    this.locks.set(key, locked);
    this.host.redraw();
    try {
      await this.host.change(login, { change: "set_account", name: account, locked });
    } catch (error) {
      this.locks.delete(key);
      this.host.redraw();
      this.host.failed(words.failed(error instanceof Error ? error.message : String(error)));
      return;
    }
    this.host.undo(locked ? words.lockedSaid(account) : words.unlockedSaid(account), async () => {
      this.locks.set(key, !locked);
      this.host.redraw();
      await this.host.change(login, { change: "set_account", name: account, locked: !locked });
    });
    await this.host.store.refresh();
    this.host.redraw();
  }

  private makeAdmin(login: Login, account: string): void {
    const { words } = this.host;
    this.host.confirm.ask({
      title: words.makeAdminTitle(account),
      what: words.makeAdminWhat(account),
      go: words.makeAdminGo,
      act: () => this.host.change(login, { change: "transfer", to: account }),
      done: () => void this.host.store.refresh(),
    });
  }

  private remove(login: Login, hub: HubAccounts, account: string): void {
    const { words } = this.host;
    const machines = hub.members.filter((member) => member.home === account).map((member) => member.name);
    const projects = hub.projects.filter((project) => project.owner === account).map((project) => project.name);
    this.host.confirm.ask({
      title: words.removeTitle(account),
      what: words.removeWhat(machines, projects),
      go: words.removeGo,
      act: () => this.host.change(login, { change: "delete_account", name: account }),
      done: () => void this.host.store.refresh().then(() => this.host.redraw()),
    });
  }

  /** Opens sign-up to anyone who reaches the server, once agreed, or closes
   *  it at once with a moment to undo. */
  signUp(hub: string): void {
    const world = this.host.store.world.peek();
    const at = world ? this.admin(world, hub) : null;
    if (!at) return;
    const { words } = this.host;
    const set = (open: boolean) => this.host.change(at.login, { change: "settings", self_registration: open });
    if (!at.hub.self_registration) {
      this.host.confirm.ask({
        title: words.signUpTitle,
        what: words.signUpWhat,
        go: words.signUpGo,
        act: () => set(true),
        done: () => void this.host.store.refresh().then(() => this.host.redraw()),
      });
      return;
    }
    void set(false).then(
      async () => {
        this.host.undo(words.signUpClosed, () => set(true));
        await this.host.store.refresh();
        this.host.redraw();
      },
      (error: unknown) => this.host.failed(words.failed(error instanceof Error ? error.message : String(error))),
    );
  }

  /** This device's administrator login on `hub`, with what the hub says. */
  private admin(world: WorldNow, hub: string): { login: Login; hub: HubAccounts } | null {
    const login = loginsOf(world.accounts).find((one) => one.hub === hub && isSuper(world, one));
    const seen = world.accounts.hubs.find((one) => one.hub === hub);
    return login && seen ? { login, hub: seen } : null;
  }

  private lockedOf(world: WorldNow, hub: string, account: string): boolean {
    return this.locks.get(keyOf(hub, account)) ?? world.accounts.hubs.find((one) => one.hub === hub)?.accounts.find((one) => one.name === account)?.locked ?? false;
  }
}

const keyOf = (hub: string, account: string) => `${hub}\n${account}`;
