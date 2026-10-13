// The super administrator's page of a hub (`world-gui.md`, 账号、机器、共享与
// 协助): for each hub this computer or device is logged in to as its super
// administrator, what being it means and handing it to another account,
// whether anyone may open an account, every account, and every project
// with the account it belongs to. Accounts are opened, always as members,
// locked and unlocked, have their passwords reset and are deleted here; a
// project is handed to another account. Handing on and deleting say first
// what they mean.

import { newPasswordFields, passwordReady, sayPassword } from "./new-password";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { loginsOf, type AccountChange, type HubAccounts, type Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { AdminWords } from "./account-words";
import { Confirm, Sheet } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";

export interface AdminOptions {
  store: Store;
  words: AdminWords;
  head(title: string, actions?: string): string;
  when(at: Date, now: Date): string;
  change(login: Login, change: AccountChange): Promise<void>;
  confirm: Confirm;
}

const accountRule = "[a-z][a-z0-9\\-]{0,63}";

/** The logins that are their hub's super administrator. */
function administering(world: WorldNow | null): { login: Login; hub: HubAccounts }[] {
  return loginsOf(world?.accounts).flatMap((login) => {
    const hub = world?.accounts.hubs.find((hub) => hub.hub === login.hub);
    const me = hub?.accounts.find((account) => account.name === login.account);
    return hub && me?.level === "super" && !me.locked ? [{ login, hub }] : [];
  });
}

export class AdminPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly list: HTMLElement;
  private readonly stop: () => void;
  private drawn = "";
  private readonly sheet = new Sheet("device-sheet admin-sheet");
  private said: { hub: string; text: string } | null = null;

  constructor(private readonly options: AdminOptions) {
    const { words } = options;
    this.title = words.title;
    this.element = document.createElement("div");
    this.element.className = "settings-page admin-page";
    this.element.innerHTML = `
      ${options.head(`<h1 class="page-title">${escape(words.title)}</h1>`)}
      <div class="settings admin scroller"></div>`;
    this.list = this.element.querySelector<HTMLElement>(".admin")!;
    this.list.addEventListener("click", (event) => this.click(event));
    this.list.addEventListener("change", (event) => {
      const box = event.target as HTMLInputElement;
      if (box.name !== "self-registration") return;
      const at = this.at(box);
      if (!at) return;
      if (box.checked) {
        // Opening it lets anyone who reaches the server in: said first, and
        // only done once agreed.
        box.checked = false;
        this.options.confirm.ask({
          title: words.signUpTitle,
          what: words.signUpWhat,
          go: words.signUpGo,
          act: () => this.options.change(at.login, { change: "settings", self_registration: true }),
          done: () => void this.options.store.refresh(),
        });
        return;
      }
      void this.quietly(box, at.login, { change: "settings", self_registration: false });
    });
    this.stop = effect(() => this.draw(options.store.world.get(), options.store.clock.get()));
  }

  leave(): void {
    this.stop();
  }

  private at(element: HTMLElement): { login: Login; hub: HubAccounts } | undefined {
    const hub = element.closest<HTMLElement>("[data-hub]")?.dataset.hub;
    return administering(this.options.store.world.peek()).find((at) => at.login.hub === hub);
  }

  private click(event: Event): void {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]");
    if (!button || button.disabled) return;
    const at = this.at(button);
    if (!at) return;
    const { login, hub } = at;
    const { words } = this.options;
    const name = button.dataset.name ?? "";
    const account = hub.accounts.find((found) => found.name === name);
    switch (button.dataset.act) {
      case "lock":
        if (account) void this.quietly(button, login, { change: "set_account", name, locked: !account.locked });
        return;
      case "transfer": {
        const others = hub.accounts.filter((found) => found.name !== login.account && !found.locked);
        this.form(
          words.transferTitle,
          `<p class="confirm-what">${escape(words.transferWhat)}</p>
          ${
            others.length
              ? `<label class="field"><span class="field-label">${escape(words.transferTo)}</span><select name="to" required>${others
                  .map((found) => `<option value="${escape(found.name)}">${escape(found.name)}</option>`)
                  .join("")}</select></label>`
              : `<p class="field-hint">${escape(words.noOther)}</p>`
          }`,
          words.transferGo,
          async (form) => {
            const to = form.querySelector<HTMLSelectElement>("[name=to]")?.value;
            if (!to) return words.noOther;
            await this.options.change(login, { change: "transfer", to });
          },
        );
        return;
      }
      case "reset":
        this.form(
          words.resetTitle(name),
          `<p class="confirm-what">${escape(words.resetWhat)}</p>${newPasswordFields(words, { pair: false })}`,
          words.resetGo,
          async (form) => {
            if (!passwordReady(form, words, name)) return false;
            const password = form.querySelector<HTMLInputElement>("[name=password]")!.value;
            await this.options.change(login, { change: "set_password", target: name, password });
          },
        );
        this.sheet.body.oninput = () => sayPassword(this.sheet.body, words, name);
        return;
      case "delete": {
        const machines = hub.members.filter((member) => member.home === name).map((member) => member.name);
        const projects = hub.projects.filter((project) => project.owner === name).map((project) => project.name);
        this.options.confirm.ask({
          title: words.deleteTitle(name),
          what: words.deleteWhat(name, machines, projects),
          go: words.deleteGo,
          act: () => this.options.change(login, { change: "delete_account", name }),
          done: () => void this.options.store.refresh(),
        });
        return;
      }
      case "open":
        this.form(
          words.openTitle,
          `<p class="confirm-what">${escape(words.openWhat)}</p>
          <label class="field"><span class="field-label">${escape(words.name)}</span><input name="name" required autocomplete="off" spellcheck="false" autocapitalize="off" pattern="${accountRule}"><span class="field-hint">${escape(words.nameHint)}</span></label>
          ${newPasswordFields(words, { pair: false })}`,
          words.openGo,
          async (form) => {
            const name = form.querySelector<HTMLInputElement>("[name=name]")!.value.trim();
            if (!passwordReady(form, words, name)) return false;
            const password = form.querySelector<HTMLInputElement>("[name=password]")!.value;
            await this.options.change(login, { change: "open_account", name, password });
          },
        );
        // The account's name counts against its password as it is typed.
        this.sheet.body.oninput = () => sayPassword(this.sheet.body, words, this.sheet.body.querySelector<HTMLInputElement>("[name=name]")?.value.trim() ?? "");
        return;
      case "hand": {
        const town = button.dataset.town!;
        const owner = hub.projects.find((project) => project.name === town)?.owner;
        const others = hub.accounts.filter((found) => found.name !== owner);
        this.form(
          words.handTitle(town),
          `<p class="confirm-what">${escape(words.handWhat)}</p>
          <label class="field"><span class="field-label">${escape(words.owner)}</span><select name="to" required>${others
            .map((found) => `<option value="${escape(found.name)}">${escape(found.name)}</option>`)
            .join("")}</select></label>`,
          words.handGo,
          async (form) => {
            const to = form.querySelector<HTMLSelectElement>("[name=to]")!.value;
            await this.options.change(login, { change: "move_project", town, to });
          },
        );
        return;
      }
    }
  }

  /** A form on the sheet: one button, the sheet open with why until the
   *  hub does it. */
  private form(title: string, fields: string, go: string, act: (form: HTMLElement) => Promise<string | false | void>): void {
    const { words } = this.options;
    const { sheet } = this;
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
      const stay = () => {
        sheet.busy = false;
        button.disabled = false;
        button.textContent = go;
      };
      const done = (wrong: string | null) => {
        sheet.busy = false;
        button.disabled = false;
        button.textContent = go;
        if (wrong !== null) {
          note.textContent = wrong;
          note.hidden = false;
          return;
        }
        sheet.close();
        void this.options.store.refresh();
      };
      sheet.busy = true;
      button.disabled = true;
      button.textContent = words.working;
      note.hidden = true;
      act(sheet.body).then(
        // What is wrong was said beside the field it is about.
        (wrong) => (wrong === false ? stay() : done(wrong ?? null)),
        (error: unknown) => done(words.failed(error instanceof Error ? error.message : String(error))),
      );
    };
    sheet.show();
    sheet.body.querySelector<HTMLElement>("input, select")?.focus();
  }

  private async quietly(control: HTMLInputElement | HTMLButtonElement, login: Login, change: AccountChange): Promise<void> {
    control.disabled = true;
    try {
      await this.options.change(login, change);
      this.said = null;
    } catch (error) {
      this.said = { hub: login.hub, text: this.options.words.failed(error instanceof Error ? error.message : String(error)) };
    }
    await this.options.store.refresh();
    this.drawn = "";
    this.draw(this.options.store.world.peek(), this.options.store.clock.peek());
  }

  private draw(world: WorldNow | null, now: Date): void {
    if (!world) return;
    const { words } = this.options;
    const hubs = administering(world);
    const html = hubs.length
      ? hubs.map(({ login, hub }) => this.section(login, hub, now)).join("")
      : `<section class="home-section"><p class="quiet">${escape(words.notAdmin)}</p></section>`;
    if (html === this.drawn) return;
    this.drawn = html;
    redrawKeepingFocus(this.list, () => {
      this.list.innerHTML = html;
    });
  }

  private section(login: Login, hub: HubAccounts, now: Date): string {
    const { words } = this.options;
    const when = (at: string) => this.options.when(new Date(at), now);
    const button = (act: string, label: string, name: string, extra = "") =>
      `<button class="text-button quiet-button" type="button" data-act="${act}" data-name="${escape(name)}"${extra}>${escape(label)}</button>`;
    const accounts = hub.accounts
      .map((account) => {
        const you = account.name === login.account;
        const top = account.level === "super";
        const tags = [you ? words.you : "", top ? words.superTag : "", account.locked ? words.locked : ""].filter(Boolean);
        return `<li class="admin-account${account.locked ? " locked" : ""}">
          <div class="admin-account-line"><span class="admin-account-name">${escape(account.name)}</span>${tags
            .map((tag) => `<span class="tag">${escape(tag)}</span>`)
            .join("")}<span class="admin-account-opened">${escape(words.opened(when(account.opened)))}</span></div>
          <div class="machine-acts">
            ${you || top ? "" : button("lock", account.locked ? words.unlock : words.lock, account.name)}
            ${button("reset", words.resetPassword, account.name)}
            ${you || top ? "" : button("delete", words.deleteAccount, account.name, " data-danger")}
          </div>
        </li>`;
      })
      .join("");
    const projects = hub.projects
      .map(
        (project) => `<li class="admin-account">
          <div class="admin-account-line"><span class="admin-account-name">${escape(project.name)}</span><span class="admin-account-opened">${escape(`${words.owner} ${project.owner}`)}</span></div>
          <div class="machine-acts"><button class="text-button quiet-button" type="button" data-act="hand" data-town="${escape(project.name)}">${escape(words.handTo)}</button></div>
        </li>`,
      )
      .join("");
    const said = this.said?.hub === hub.hub ? `<p class="settings-said" role="status">${escape(this.said.text)}</p>` : "";
    return `<section class="home-section admin-section" data-hub="${escape(hub.hub)}">
      <h2 class="section-title account-title">${glyph("settings", 18)}<span>${escape(words.hub(hub.name))}</span></h2>
      ${said}
      <h3 class="account-part">${escape(words.superPart)}</h3>
      <p class="admin-super">${glyph("shield", 15)}<span>${escape(words.superWhat)}</span></p>
      <div class="account-acts"><button class="text-button" type="button" data-act="transfer">${glyph("move", 16)}<span>${escape(words.transfer)}</span></button></div>
      <label class="check-row" title="${escape(words.selfRegistrationWhat)}"><input type="checkbox" name="self-registration"${hub.self_registration ? " checked" : ""}><span class="check-title">${escape(words.selfRegistration)}</span></label>
      <h3 class="account-part">${escape(words.accounts)}</h3>
      <ul class="admin-accounts">${accounts}</ul>
      <div class="account-acts"><button class="text-button" type="button" data-act="open">${glyph("plus", 16)}<span>${escape(words.openAccount)}</span></button></div>
      ${projects ? `<h3 class="account-part">${escape(words.projects)}</h3><ul class="admin-accounts">${projects}</ul>` : ""}
    </section>`;
  }
}
