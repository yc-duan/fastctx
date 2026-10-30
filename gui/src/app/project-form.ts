// Opening a project, or writing the next version of its record
// (`world-projects.md`, 开一个项目): its name and short name side by side,
// what it is, and the machines it spans with the places named on each
// (`world-gui.md`, 项目). The machines chosen stand first, each with its
// places under it, each place a folder chosen in a window of that
// machine's own (`folder-picker.ts`) and a word on what is there, its
// folder's name until the user writes another; under them every machine the
// account may work on, in its owner's groups, narrowed by what is typed
// (`machine-choice.ts`). The World stamps who wrote each version, where and
// when; every version is kept. Logged in to more than one account, a new
// project is opened in the one chosen, and spans only machines that account
// may work on (`world-network.md`, 项目也照同一个规矩).

import { leave } from "./leave";
import { onShade, watchShade } from "./shade-press";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { redrawKeepingFocus } from "./focus";
import type { RecordFields, TownRecord } from "../world/bridge";
import type { Login } from "../world/accounts";
import { folderName, type Folders } from "../world/focus";
import type { ProjectRecord } from "../world/world";
import { FolderPicker } from "./folder-picker";
import { focusPlace } from "./focus-places";
import type { FocusPlace } from "./focus-panel";
import { foldChoice, keptFolds, machineChoice, type ChoiceDrawn } from "./machine-choice";
import { groupsOf, loginLabel, machinesOf, rightsOn, sameLogin, type Machine } from "./reach";
import { whenNear } from "./read-on";
import type { Store } from "./store";
import type { FocusWords, ProjectFormWords } from "./words";

/** How many of the machines' rows are drawn at first, and how many more
 *  each time the end of those comes near. */
const CHOICE_ROWS = 120;

export interface ProjectFormOptions {
  store: Store;
  words: ProjectFormWords;
  /** The machines' own words, shared with the machines and folders panel. */
  focusWords: FocusWords;
  /** A machine's folders, read by its node. */
  folders(machine: string, path: string | null): Promise<Folders>;
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

const slugRule = /^[a-z][a-z0-9]*(-[a-z0-9]+)*$/;

/** A new project's short name, which commands and each machine's folders
 *  know it by (`TownName`) and the user never types: its name's own words
 *  where it is written in Latin letters, else `project`; with an ending of
 *  its own after a first try found the name taken, here or on the hub. At
 *  most 32 characters, as every release reads them. */
function shortName(name: string, attempt: number): string {
  const words = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^[^a-z]+/, "")
    .slice(0, 24)
    .replace(/-+$/, "");
  const base = slugRule.test(words) ? words : "project";
  if (attempt === 0 && base !== "project") return base;
  const ending = Array.from(crypto.getRandomValues(new Uint8Array(4)), (byte) => "abcdefghijkmnpqrstuvwxyz23456789"[byte % 32]).join("");
  return `${base}-${ending}`;
}

/** Whether opening a project failed only because its short name is taken. */
const nameTaken = (error: unknown) => /exists already|exists on this hub already/.test(String(error));

export class ProjectForm {
  private readonly dialog: HTMLDialogElement;
  private readonly body: HTMLElement;
  private readonly picker: FolderPicker;
  private editing: { town: string; record: ProjectRecord } | null = null;
  /** The account a new project opens in. */
  private login: Login | null = null;
  /** The machines it spans, by name, in the order chosen. */
  private machines: string[] = [];
  private places: Place[] = [];
  private busy = false;
  /** What narrows the machines to choose among. */
  private query = "";
  private readonly folds = keptFolds();
  /** How many of the machines' rows are drawn, how many each person's
   *  opens onto as last drawn, and what stops watching the end of those
   *  coming near. */
  private reach = CHOICE_ROWS;
  private inside = new Map<string, number>();
  private unwatch = () => {};

  constructor(private readonly options: ProjectFormOptions) {
    this.picker = new FolderPicker(options.focusWords.picker);
    this.dialog = document.createElement("dialog");
    this.dialog.className = "sheet form-sheet project-sheet";
    // The form says itself what a field still needs, in the page's words.
    this.dialog.innerHTML = `<form class="sheet-body" method="dialog" novalidate></form>`;
    document.body.append(this.dialog);
    this.body = this.dialog.querySelector("form")!;
    watchShade(this.dialog);
    this.dialog.addEventListener("click", (event) => this.click(event));
    // Escape closes it as the close button does, going as it came.
    this.dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      this.close();
    });
    this.body.addEventListener("input", (event) => {
      const target = event.target as HTMLInputElement;
      // A field filled in no longer needs anything.
      if (target.name === "name" || target.name === "what") {
        const said = this.body.querySelector<HTMLElement>(`[data-need="${target.name}"]`);
        if (said && target.value.trim()) said.hidden = true;
      }
      if (target.classList.contains("pf-filter")) {
        this.query = target.value.trim().toLowerCase();
        this.reach = CHOICE_ROWS;
        this.drawChoice();
      }
    });
    this.body.addEventListener("keydown", (event) => {
      // Enter in the field that narrows the machines submits nothing.
      if (event.key === "Enter" && (event.target as HTMLElement).classList.contains("pf-filter")) event.preventDefault();
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
    this.query = "";
    this.reach = CHOICE_ROWS;
    const here = this.options.store.world.peek()?.here;
    this.machines = editing ? [...editing.record.machines] : here ? [here] : [];
    this.places = editing ? editing.record.places.map((place) => ({ ...place })) : [];
    // A form opened anew starts from its record, never from what the last
    // one opened left in the fields.
    this.body.replaceChildren();
    this.draw(editing?.record ?? null);
    if (!this.dialog.open) this.dialog.showModal();
    this.body.querySelector<HTMLInputElement>("[name=name]")?.focus();
  }

  close(): void {
    if (!this.busy && this.dialog.open) leave(this.dialog, () => this.dialog.close(), { as: "sheet-leaving", moving: this.body });
  }

  private click(event: Event): void {
    if (onShade(this.dialog, event)) {
      this.close();
      return;
    }
    const target = event.target as HTMLElement;
    if (target.closest(".sheet-close")) {
      this.close();
      return;
    }
    const account = target.closest<HTMLButtonElement>("[data-login]");
    if (account) {
      this.login = groupsOf(this.options.store.world.peek()).map((group) => group.login).find((login) => `${login.hub}/${login.account}` === account.dataset.login) ?? null;
      redrawKeepingFocus(this.body, () => this.draw());
      return;
    }
    const act = target.closest<HTMLButtonElement>("[data-act]");
    if (!act || act.disabled) return;
    const machine = act.dataset.machine ?? "";
    switch (act.dataset.act) {
      case "toggle":
        if (this.machines.includes(machine)) this.drop(machine);
        else this.machines.push(machine);
        if (this.machines.length) this.body.querySelector<HTMLElement>('[data-need="machines"]')?.setAttribute("hidden", "");
        this.drawMachines();
        break;
      case "drop":
        this.drop(machine);
        this.drawMachines();
        break;
      case "fold":
        // A person opened is drawn whole, the rows after it as they were.
        if (act.getAttribute("aria-expanded") !== "true") this.reach += this.inside.get(act.dataset.key ?? "") ?? 0;
        foldChoice(this.body, act, this.folds, () => this.choice().html);
        break;
      case "place":
        this.choosePlace(machine);
        break;
      case "unplace":
        this.places.splice(Number(act.dataset.index), 1);
        this.drawChosen();
        break;
    }
  }

  /** Lets a machine go, and the places named on it. */
  private drop(machine: string): void {
    this.machines = this.machines.filter((one) => one !== machine);
    this.places = this.places.filter((place) => place.machine !== machine);
  }

  /** A folder on `machine`, chosen in its own window, named by its folder's
   *  name until the user names it otherwise. */
  private choosePlace(machine: string): void {
    const named = this.places.filter((place) => place.machine === machine);
    this.picker.open({
      machine,
      folders: (path) => this.options.folders(machine, path),
      marks: named.map((place) => ({ name: place.what || folderName(place.path), path: place.path })),
      start: named.at(-1)?.path ?? null,
      chosen: (path) => {
        if (!this.places.some((place) => place.machine === machine && place.path === path)) this.places.push({ what: folderName(path), machine, path });
        this.drawChosen();
      },
    });
  }

  /** The machines it may span: those its account may work on, and any the
   *  record names that this one no longer knows, so writing the next version
   *  does not drop them unawares. */
  private choices(): FocusPlace[] {
    const world = this.options.store.world.peek();
    const places = this.allowed().map((machine) => ({ ...focusPlace(world, machine), written: machine.name }));
    const gone = this.machines.filter((name) => !places.some((place) => place.written === name)).map((name): FocusPlace => ({ written: name, name, here: false, online: false }));
    return [...places, ...gone];
  }

  /** The machines its account may work on: the chosen one's for a new
   *  project; for one already open, the account it is in. */
  private allowed(): Machine[] {
    const world = this.options.store.world.peek();
    const groups = groupsOf(world);
    const group = this.editing
      ? groups.find((one) => one.towns.includes(this.editing!.town))
      : groups.find((one) => this.login && sameLogin(one.login, this.login));
    return group ? group.machines.filter((machine) => rightsOn(machine, group.login).hands) : machinesOf(world);
  }

  private draw(record: ProjectRecord | null = null): void {
    const { words, store } = this.options;
    const world = store.world.peek();
    const groups = groupsOf(world);
    // Another account chosen: what it may not work on is let go.
    if (!this.editing) {
      const may = new Set(this.allowed().map((machine) => machine.name));
      for (const machine of [...this.machines]) if (!may.has(machine)) this.drop(machine);
    }
    // Drawn again in place, as when another account is chosen, the form keeps
    // what was typed; `open` empties it first.
    const kept = {
      name: this.body.querySelector<HTMLInputElement>("[name=name]")?.value ?? record?.name ?? "",
      what: this.body.querySelector<HTMLTextAreaElement>("[name=what]")?.value ?? record?.what ?? "",
    };
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
        <div class="pf-names">
          <label class="field pf-name"><span class="field-label">${escape(words.name)}</span>
            <input name="name" autocomplete="off" required placeholder="${escape(words.namePlaceholder)}" value="${escape(kept.name)}">
            <span class="field-hint pf-need" data-need="name" hidden>${escape(words.needName)}</span></label>
        </div>
        <div class="field"><span class="field-label">${escape(words.what)}<span class="field-tip" tabindex="0" aria-label="${escape(words.whatTip)}">?<span class="field-tip-card" role="tooltip">${escape(words.whatTip)}</span></span></span>
          <textarea name="what" rows="5" required aria-label="${escape(words.what)}" placeholder="${escape(words.whatPlaceholder)}">${escape(kept.what)}</textarea>
          <span class="field-hint pf-need" data-need="what" hidden>${escape(words.needWhat)}</span></div>
        <div class="field pf-machines"><span class="field-label">${escape(words.machinesPlaces)}</span>
          <span class="field-hint pf-need" data-need="machines" hidden>${escape(words.needMachine)}</span>
          <div class="pf-chosen"></div>
          <div class="pf-choose">
            <label class="focus-search">${glyph("search", 16)}<input class="pf-filter focus-filter" type="search" placeholder="${escape(this.options.focusWords.search)}" aria-label="${escape(this.options.focusWords.search)}" autocomplete="off" spellcheck="false" value="${escape(this.query)}"></label>
            <div class="pf-list"></div>
          </div>
        </div>
      </div>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(this.editing ? words.save : words.open)}</button></footer>`;
    this.drawMachines();
  }

  private drawMachines(): void {
    this.drawChosen();
    this.drawChoice();
  }

  /** The machines chosen, each with its places and a way to add one. */
  private drawChosen(): void {
    const { words, focusWords } = this.options;
    const holder = this.body.querySelector<HTMLElement>(".pf-chosen");
    if (!holder) return;
    const choices = this.choices();
    const html = this.machines.length
      ? this.machines
          .map((machine) => {
            const place = choices.find((one) => one.written === machine);
            const away = place?.online === false;
            const rows = this.places
              .map((one, index) => ({ one, index }))
              .filter(({ one }) => one.machine === machine)
              .map(
                ({ one, index }) =>
                  `<div class="pf-place">${glyph("folder", 16)}<span class="pf-path" title="${escape(one.path)}">${escape(one.path)}</span><button type="button" class="focus-remove" data-act="unplace" data-index="${index}" aria-label="${escape(`${words.removePlace} ${one.path}`)}" title="${escape(words.removePlace)}">${glyph("close", 12)}</button></div>`,
              )
              .join("");
            return `<div class="focus-held pf-machine${away ? " offline" : ""}"${away ? ` title="${escape(focusWords.offline)}"` : ""}>
              <div class="focus-held-head"><span class="focus-mark">${glyph(place?.form ?? "machine", 16)}</span><span class="focus-name">${escape(machine)}</span>${place?.here ? `<span class="focus-note">${escape(focusWords.here)}</span>` : ""}<button type="button" class="focus-unpick" data-act="drop" data-machine="${escape(machine)}" aria-label="${escape(`${focusWords.remove} ${machine}`)}" title="${escape(focusWords.remove)}">${glyph("close", 14)}</button></div>
              <div class="pf-places">${rows}<button type="button" class="focus-add" data-act="place" data-machine="${escape(machine)}"${away ? " disabled" : ""}>${glyph("plus", 14)}<span>${escape(words.addPlace)}</span></button></div>
            </div>`;
          })
          .join("")
      : `<p class="focus-none">${escape(words.noMachines)}</p>`;
    redrawKeepingFocus(holder, () => {
      holder.innerHTML = html;
    });
  }

  /** Every machine it may span, narrowed to what is typed. */
  private drawChoice(): void {
    const holder = this.body.querySelector<HTMLElement>(".pf-list");
    if (!holder) return;
    const html = this.choice().html || `<p class="focus-none">${escape(this.options.focusWords.noMatch)}</p>`;
    redrawKeepingFocus(holder, () => {
      holder.innerHTML = html;
    });
    this.unwatch();
    this.unwatch = whenNear(holder, holder, () => {
      this.reach += CHOICE_ROWS;
      this.drawChoice();
    });
  }

  /** The machines to choose among, as far as they are drawn. */
  private choice(): ChoiceDrawn {
    const drawn = machineChoice(this.choices(), new Set(this.machines), this.query, this.folds, this.options.focusWords, this.reach);
    this.inside = drawn.inside;
    return drawn;
  }

  /** Opens the new project under a short name of its own, another one
   *  each time the one tried is taken. */
  private async opened(name: string, fields: RecordFields): Promise<{ town: string }> {
    for (let attempt = 0; ; attempt += 1) {
      try {
        return await this.options.create(shortName(name, attempt), fields, this.login);
      } catch (error) {
        if (attempt >= 4 || !nameTaken(error)) throw error;
      }
    }
  }

  private async submit(): Promise<void> {
    if (this.busy) return;
    const { words } = this.options;
    const value = (name: string) => this.body.querySelector<HTMLInputElement>(`[name="${name}"]`)?.value.trim() ?? "";
    // What is missing is said beside it, and the first of it takes the focus.
    const needs: [string, boolean][] = [
      ["name", !value("name")],
      ["what", !value("what")],
      ["machines", !this.machines.length],
    ];
    for (const [need, missing] of needs) {
      const said = this.body.querySelector<HTMLElement>(`[data-need="${need}"]`);
      if (said) said.hidden = !missing;
    }
    const first = needs.find(([, missing]) => missing)?.[0];
    if (first) {
      this.body.querySelector<HTMLElement>(first === "machines" ? ".pf-filter" : `[name="${first}"]`)?.focus();
      return;
    }
    const fields: RecordFields = {
      name: value("name"),
      what: value("what"),
      machines: [...this.machines],
      // A place keeps its folder's name where its word was cleared.
      places: this.places.map((place) => ({ what: place.what.trim() || folderName(place.path), machine: place.machine, path: place.path })),
      // Where decisions are kept is project memory, not asked for until it
      // is finished (`world-journals.md`); a record that names it keeps it.
      canon: this.editing?.record.canon ?? null,
    };
    const note = this.body.querySelector<HTMLElement>(".sheet-note")!;
    const go = this.body.querySelector<HTMLButtonElement>(".sheet-go")!;
    this.busy = true;
    go.disabled = true;
    go.textContent = words.saving;
    note.hidden = true;
    try {
      const written = this.editing ? await this.options.edit(this.editing.town, fields) : await this.opened(value("name"), fields);
      this.busy = false;
      this.close();
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
