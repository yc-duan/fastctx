// Opening a project, or writing the next version of its record
// (`world-projects.md`, 开一个项目): its name, its short name, what it is,
// the machines it spans, the address book of where things are, and where
// its decisions are kept. The World stamps who wrote each version, where
// and when; every version is kept. Logged in to more than one account, a
// new project is opened in the one chosen, and spans only machines that
// account may work on (`world-network.md`, 项目也照同一个规矩).

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { redrawKeepingFocus } from "./focus";
import type { RecordFields, TownRecord } from "../world/bridge";
import type { Login } from "../world/accounts";
import type { ProjectRecord } from "../world/world";
import { groupsOf, loginLabel, machinesOf, rightsOn, sameLogin } from "./reach";
import type { Store } from "./store";
import type { ProjectFormWords } from "./words";

export interface ProjectFormOptions {
  store: Store;
  words: ProjectFormWords;
  /** Opens a project in `login`'s account, or in the one account. */
  create(town: string, fields: RecordFields, login: Login | null): Promise<TownRecord>;
  edit(town: string, fields: RecordFields): Promise<TownRecord>;
  /** The project was written: go to it, or show it again. */
  done(town: string): void;
}

interface Place {
  what: string;
  machine: string;
  path: string;
}

const slugRule = /^[a-z][a-z0-9-]{0,31}$/;

/** A short name suggested from a Latin name; nothing from other scripts. */
function suggest(name: string): string {
  const slug = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^[^a-z]+/, "")
    .replace(/-+$/, "")
    .slice(0, 32);
  return slugRule.test(slug) ? slug : "";
}

export class ProjectForm {
  private readonly dialog: HTMLDialogElement;
  private readonly body: HTMLElement;
  private editing: { town: string; record: ProjectRecord } | null = null;
  /** The account a new project opens in. */
  private login: Login | null = null;
  private machines = new Set<string>();
  private places: Place[] = [];
  private slugTouched = false;
  private busy = false;

  constructor(private readonly options: ProjectFormOptions) {
    this.dialog = document.createElement("dialog");
    this.dialog.className = "sheet form-sheet";
    this.dialog.innerHTML = `<form class="sheet-body" method="dialog"></form>`;
    document.body.append(this.dialog);
    this.body = this.dialog.querySelector("form")!;
    this.dialog.addEventListener("click", (event) => {
      if (event.target === this.dialog) this.close();
      const target = event.target as HTMLElement;
      if (target.closest(".sheet-close")) this.close();
      const account = target.closest<HTMLButtonElement>("[data-login]");
      if (account) {
        this.read();
        this.login = groupsOf(this.options.store.world.peek()).map((group) => group.login).find((login) => `${login.hub}/${login.account}` === account.dataset.login) ?? null;
        redrawKeepingFocus(this.body, () => this.draw());
        return;
      }
      const machine = target.closest<HTMLButtonElement>("[data-machine]");
      if (machine) {
        const name = machine.dataset.machine!;
        if (this.machines.has(name)) this.machines.delete(name);
        else this.machines.add(name);
        machine.classList.toggle("chosen", this.machines.has(name));
        machine.setAttribute("aria-checked", String(this.machines.has(name)));
      }
      if (target.closest(".add-place")) {
        this.read();
        this.places.push({ what: "", machine: [...this.machines][0] ?? this.options.store.world.peek()?.here ?? "", path: "" });
        this.draw();
        this.body.querySelector<HTMLInputElement>(".address-row:last-of-type .place-what-input")?.focus();
      }
      const remove = target.closest<HTMLButtonElement>("[data-remove]");
      if (remove) {
        this.read();
        this.places.splice(Number(remove.dataset.remove), 1);
        redrawKeepingFocus(this.body, () => this.draw());
      }
    });
    this.body.addEventListener("input", (event) => {
      const target = event.target as HTMLInputElement;
      if (target.name === "slug") this.slugTouched = true;
      if (target.name === "name" && !this.editing && !this.slugTouched) {
        const slug = this.body.querySelector<HTMLInputElement>("[name=slug]");
        if (slug) slug.value = suggest(target.value);
      }
    });
    this.body.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.submit();
    });
  }

  /** Opens the form for a new project, in `login`'s account where one is
   *  named, or for the next version of one. */
  open(editing: { town: string; record: ProjectRecord } | null = null, login: Login | null = null): void {
    this.editing = editing;
    const logins = groupsOf(this.options.store.world.peek()).map((group) => group.login);
    this.login = (login && logins.find((known) => sameLogin(known, login))) ?? logins[0] ?? null;
    this.slugTouched = false;
    const here = this.options.store.world.peek()?.here;
    this.machines = new Set(editing ? editing.record.machines : here ? [here] : []);
    this.places = editing ? editing.record.places.map((place) => ({ ...place })) : [];
    this.draw(editing?.record ?? null);
    if (!this.dialog.open) this.dialog.showModal();
    this.body.querySelector<HTMLInputElement>("[name=name]")?.focus();
  }

  close(): void {
    if (!this.busy) this.dialog.close();
  }

  /** Keeps what was typed in the address book before it is drawn again. */
  private read(): void {
    this.places = [...this.body.querySelectorAll<HTMLElement>(".address-row")].map((row) => ({
      what: row.querySelector<HTMLInputElement>(".place-what-input")!.value,
      machine: row.querySelector<HTMLSelectElement>(".place-machine-input")!.value,
      path: row.querySelector<HTMLInputElement>(".place-path-input")!.value,
    }));
  }

  private draw(record: ProjectRecord | null = null): void {
    const { words, store } = this.options;
    const world = store.world.peek();
    // A project spans machines its account may work on: the chosen one for a
    // new project; for one already open, the account it is in.
    const groups = groupsOf(world);
    const group = this.editing
      ? groups.find((group) => group.towns.includes(this.editing!.town))
      : groups.find((group) => this.login && sameLogin(group.login, this.login));
    const nodes = (group ? group.machines.filter((machine) => rightsOn(machine, group.login).hands) : machinesOf(world)).map((machine) => machine.name);
    // Another account chosen: what it may not work on is let go.
    if (!this.editing) for (const machine of [...this.machines]) if (!nodes.includes(machine)) this.machines.delete(machine);
    // A machine the record names that this one no longer knows stays a
    // choice, so writing the next version does not drop it unawares.
    const names = [...new Set([...nodes, ...this.machines])];
    const kept = {
      name: this.body.querySelector<HTMLInputElement>("[name=name]")?.value ?? record?.name ?? "",
      slug: this.body.querySelector<HTMLInputElement>("[name=slug]")?.value ?? "",
      what: this.body.querySelector<HTMLTextAreaElement>("[name=what]")?.value ?? record?.what ?? "",
    };
    const options = (chosen: string) =>
      `${names
        .map((name) => `<option value="${escape(name)}"${name === chosen ? " selected" : ""}>${escape(name)}</option>`)
        .join("")}`;
    this.body.innerHTML = `
      <header class="sheet-head">
        <h2 class="sheet-title">${escape(this.editing ? words.editTitle(this.editing.record.name) : words.newTitle)}</h2>
        <button class="icon-button sheet-close" type="button" aria-label="${escape(words.cancel)}">${glyph("close", 18)}</button>
      </header>
      <div class="form-scroll">
        ${
          !this.editing && groups.length > 1
            ? `<div class="field"><span class="field-label">${escape(words.account)}</span>
          <div class="choice-options" role="radiogroup" aria-label="${escape(words.account)}">${groups
            .map((group) => {
              const on = !!this.login && sameLogin(group.login, this.login);
              return `<button type="button" class="choice${on ? " chosen" : ""}" role="radio" aria-checked="${on}" data-login="${escape(`${group.login.hub}/${group.login.account}`)}">${escape(loginLabel(group.login))}</button>`;
            })
            .join("")}</div></div>`
            : ""
        }
        <label class="field"><span class="field-label">${escape(words.name)}</span>
          <input name="name" autocomplete="off" required placeholder="${escape(words.namePlaceholder)}" value="${escape(kept.name)}"></label>
        ${
          this.editing
            ? ""
            : `<label class="field"><span class="field-label">${escape(words.slug)}</span>
          <input name="slug" autocomplete="off" required pattern="[a-z][a-z0-9-]{0,31}" spellcheck="false" class="mono" value="${escape(kept.slug)}">
          <span class="field-hint">${escape(words.slugHint)}</span></label>`
        }
        <div class="field"><span class="field-label">${escape(words.what)}<span class="field-tip" tabindex="0" aria-label="${escape(words.whatTip)}">?<span class="field-tip-card" role="tooltip">${escape(words.whatTip)}</span></span></span>
          <textarea name="what" rows="6" required aria-label="${escape(words.what)}" placeholder="${escape(words.whatPlaceholder)}">${escape(kept.what)}</textarea></div>
        <div class="field"><span class="field-label">${escape(words.machines)}</span>
          <div class="choice-options" role="group" aria-label="${escape(words.machines)}">${names
            .map(
              (name) =>
                `<button type="button" class="choice${this.machines.has(name) ? " chosen" : ""}" role="checkbox" aria-checked="${this.machines.has(name)}" data-machine="${escape(name)}">${escape(name)}</button>`,
            )
            .join("")}</div></div>
        <div class="field"><span class="field-label">${escape(words.places)}</span>
          <span class="field-hint">${escape(words.placesHint)}</span>
          <div class="address-rows">${this.places
            .map(
              (place, index) => `<div class="address-row">
                <input class="place-what-input" placeholder="${escape(words.placeWhat)}" aria-label="${escape(words.placeWhat)}" value="${escape(place.what)}">
                <select class="place-machine-input" aria-label="${escape(words.placeMachine)}">${options(place.machine)}</select>
                <input class="place-path-input mono" placeholder="${escape(words.placePath)}" aria-label="${escape(words.placePath)}" spellcheck="false" value="${escape(place.path)}">
                <button class="icon-button" type="button" data-remove="${index}" aria-label="${escape(words.removePlace)}">${glyph("close", 16)}</button>
              </div>`,
            )
            .join("")}</div>
          <button class="text-button add-place" type="button">${glyph("plus", 16)}<span>${escape(words.addPlace)}</span></button></div>
      </div>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(this.editing ? words.save : words.open)}</button></footer>`;
  }

  private async submit(): Promise<void> {
    if (this.busy) return;
    const { words } = this.options;
    this.read();
    const value = (name: string) => this.body.querySelector<HTMLInputElement>(`[name="${name}"]`)?.value.trim() ?? "";
    const fields: RecordFields = {
      name: value("name"),
      what: value("what"),
      machines: [...this.machines],
      places: this.places
        .map((place) => ({ what: place.what.trim(), machine: place.machine, path: place.path.trim() }))
        .filter((place) => place.what || place.path),
      // Where decisions are kept is project memory, not asked for until it
      // is finished (`world-journals.md`); a record that names it keeps it.
      canon: this.editing?.record.canon ?? null,
    };
    const town = this.editing?.town ?? value("slug");
    const note = this.body.querySelector<HTMLElement>(".sheet-note")!;
    const go = this.body.querySelector<HTMLButtonElement>(".sheet-go")!;
    this.busy = true;
    go.disabled = true;
    go.textContent = words.saving;
    note.hidden = true;
    try {
      const written = this.editing ? await this.options.edit(town, fields) : await this.options.create(town, fields, this.login);
      this.busy = false;
      this.dialog.close();
      this.options.done(written.town);
    } catch (error) {
      this.busy = false;
      note.textContent = words.failed(String(error));
      note.title = String(error);
      note.hidden = false;
      go.disabled = false;
      go.textContent = this.editing ? words.save : words.open;
    }
  }
}
