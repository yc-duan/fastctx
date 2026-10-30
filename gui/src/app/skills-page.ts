// The account's skills (`world-user-config.md`, 技能; `world-gui.md`, 智能体页):
// a section of the agents page beside the prompt and the MCP servers, its
// head the page's name and the account's pill. Each skill a row, its name
// and when it applies; a row opens its folder in place (`./skill-editor`),
// 「新建」 writes another, 「导入」 takes in a folder from this device. What
// the conversations working for the account are told follows each change
// at once.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import { SKILL_NAME, folderChanges, type SkillCalls, type SkillFolder, type SkillListed } from "../world/skills";
import type { Confirm } from "./confirm";
import { AccountChoice, loginKey } from "./profile-account";
import type { ProfileWords } from "./profile-words";
import { detailOf } from "./problem";
import type { Page } from "./shell";
import { effect } from "./signals";
import { SkillEditor, type SkillOpened } from "./skill-editor";
import type { Store } from "./store";

export interface SkillsPageOptions {
  store: Store;
  head(title: string, actions?: string): string;
  words: ProfileWords;
  calls: SkillCalls;
  confirm: Confirm;
  close: string;
}

type State = { kind: "none" } | { kind: "reading" } | { kind: "failed"; detail: string } | { kind: "ready"; skills: SkillListed[] };

const SAID_MS = 2400;

export class SkillsPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  /** The body, scrolling on its own. */
  private readonly body: HTMLElement;
  /** What became of the last change, in the head. */
  private readonly status: HTMLElement;
  private readonly choice: AccountChoice;
  private readonly editor: SkillEditor;
  /** The skill open in place of the list, and as it was last read. */
  private editing: { login: Login; kept: SkillFolder | null } | null = null;
  private readonly stop: () => void;
  private state: State = { kind: "none" };
  private shownLogin: string | null = null;
  private said = false;
  private saidTimer = 0;

  constructor(private readonly options: SkillsPageOptions) {
    const { words } = options;
    this.title = words.sections.skills;
    this.choice = new AccountChoice(words, (login) => void this.load(login));
    this.editor = new SkillEditor(words);
    this.element = document.createElement("div");
    this.element.className = "prof-page skl-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(this.title)}</h1><span class="skl-account-place"></span>`, `<span class="prof-status" role="status"></span>`)}
      <div class="host-prefs skl-body scroller"></div>`;
    this.element.querySelector(".skl-account-place")!.replaceWith(this.choice.element);
    this.status = this.element.querySelector<HTMLElement>(".prof-status")!;
    this.body = this.element.querySelector<HTMLElement>(".skl-body")!;
    this.body.addEventListener("click", (event) => void this.click(event));
    this.stop = effect(() => this.draw(options.store.world.get()));
  }

  leave(): void {
    this.stop();
    clearTimeout(this.saidTimer);
    this.choice.leave();
  }

  /** The account pill as the World now stands, and the account's skills
   *  read again once another is chosen. */
  private draw(world: WorldNow | null): void {
    this.choice.draw(world);
    const login = this.choice.chosen();
    if (login && loginKey(login) !== this.shownLogin) void this.load(login);
  }

  private async load(login: Login): Promise<void> {
    const key = loginKey(login);
    // Another account's skills: the one open was this one's.
    if (this.shownLogin !== null && this.shownLogin !== key) this.editing = null;
    this.shownLogin = key;
    if (this.state.kind !== "ready") {
      this.state = { kind: "reading" };
      this.render();
    }
    try {
      const skills = await this.options.calls.list(login.hubName, login.account);
      if (this.shownLogin !== key) return;
      this.state = { kind: "ready", skills };
    } catch (error) {
      if (this.shownLogin === key) this.state = { kind: "failed", detail: detailOf(error) };
    }
    this.render();
  }

  private render(): void {
    const { words } = this.options;
    const state = this.state;
    if (this.editing) {
      if (this.editor.element.parentElement?.parentElement !== this.body) {
        this.body.innerHTML = `<div class="prof-board hp-board skl-board skl-editing"></div>`;
        this.body.firstElementChild!.append(this.editor.element);
      }
      this.drawStatus();
      return;
    }
    const s = words.skills;
    let html: string;
    switch (state.kind) {
      case "none":
        html = "";
        break;
      case "reading":
        html = `<p class="prof-quiet prof-board hp-first">${escape(words.reading)}</p>`;
        break;
      case "failed":
        html = `<p class="prof-quiet prof-board hp-first" title="${escape(state.detail)}">${escape(words.readFailed)}</p>`;
        break;
      case "ready": {
        const add = `${this.options.calls.pick ? `<button class="text-button quiet-button skl-import" type="button" data-skl-import>${glyph("folder", 15)}<span>${escape(s.import)}</span></button>` : ""}<button class="text-button quiet-button skl-add" type="button" data-skl-add>${glyph("plus", 15)}<span>${escape(s.add)}</span></button>`;
        const rows = state.skills.length
          ? `<ul class="skl-rows">${state.skills.map((skill) => this.row(skill)).join("")}</ul>`
          : `<div class="skl-none">${glyph("skill", 40)}<p class="prof-quiet">${escape(s.none)}</p></div>`;
        html = `<div class="prof-board hp-board skl-board"><section class="home-section skl-section"><h2 class="section-title skl-title"><span class="skl-said">${escape(this.title)}</span>${add}</h2>${rows}</section></div>`;
        break;
      }
    }
    if (this.body.innerHTML !== html) this.body.innerHTML = html;
    this.drawStatus();
  }

  /** A skill: its name, and when it applies. */
  private row(skill: SkillListed): string {
    return `<li><button class="skl-row" type="button" data-skl-open="${escape(skill.name)}">
        <span class="skl-name" title="${escape(skill.name)}">${escape(skill.name)}</span>
        <span class="skl-when" title="${escape(skill.description)}">${escape(skill.description)}</span>
      </button></li>`;
  }

  private async click(event: Event): Promise<void> {
    const target = event.target as HTMLElement;
    const state = this.state;
    const login = this.choice.chosen();
    if (state.kind !== "ready" || !login || this.editing) return;
    const name = target.closest<HTMLElement>("[data-skl-open]")?.dataset.sklOpen;
    if (name !== undefined) {
      let kept: SkillFolder | null = null;
      try {
        kept = await this.options.calls.read(login.hubName, login.account, name);
      } catch {
        kept = null;
      }
      if (!kept) return void this.load(login);
      return this.edit(login, kept, { kept: kept.name, files: kept.files });
    }
    if (target.closest("[data-skl-add]")) return this.edit(login, null, { kept: null, files: [] });
    if (target.closest("[data-skl-import]") && this.options.calls.pick) {
      let picked;
      try {
        picked = await this.options.calls.pick();
      } catch (error) {
        this.options.confirm.ask({ title: this.options.words.skills.importUnread, what: detailOf(error), go: this.options.close, gentle: true, act: async () => {}, done: () => {} });
        return;
      }
      if (!picked) return;
      // A folder's own name, as a skill may be named, where its SKILL.md
      // names none.
      const named = picked.folder.toLowerCase().replace(/[^a-z0-9-]+/g, "-").replace(/^[^a-z]+|-+$/g, "").slice(0, 64);
      this.edit(login, null, { kept: null, files: picked.files, ...(SKILL_NAME.test(named) ? { named } : {}) });
    }
  }

  /** Opens a skill in place of the list. */
  private edit(login: Login, kept: SkillFolder | null, opened: SkillOpened): void {
    const state = this.state;
    if (state.kind !== "ready") return;
    this.editing = { login, kept };
    const taken = state.skills.map((one) => one.name).filter((one) => one !== kept?.name);
    this.editor.open(opened, {
      taken,
      save: (name, files) => this.save(name, files),
      remove: () => {
        const editing = this.editing;
        if (editing?.kept) this.remove(editing.login, editing.kept);
      },
      leave: () => this.leaveEditor(),
    });
    this.render();
  }

  /** A new skill is written whole; a kept one is changed by what differs
   *  from the version it was read at, a new name in its SKILL.md renaming it
   *  at once. */
  private async save(name: string, files: SkillFolder["files"]): Promise<void> {
    const editing = this.editing;
    if (!editing) return;
    const { login, kept } = editing;
    const calls = this.options.calls;
    const written = kept
      ? await calls.edit(login.hubName, login.account, kept.name, kept.version, folderChanges(kept.files, files))
      : await calls.write(login.hubName, login.account, name, null, files);
    if (!written) throw new Error("the skill changed meanwhile");
    const now = await calls.read(login.hubName, login.account, name);
    if (this.editing === editing) this.editing = { login, kept: now };
    this.saidNow();
    void this.load(login);
  }

  /** Back to the list, asking first where changes were not saved. */
  private leaveEditor(): void {
    const back = () => {
      this.editing = null;
      this.render();
      const login = this.choice.chosen();
      if (login) void this.load(login);
    };
    if (!this.editor.changed()) return back();
    const s = this.options.words.skills;
    this.options.confirm.ask({ title: s.discardTitle, what: "", go: s.discard, act: async () => {}, done: back });
  }

  private remove(login: Login, kept: SkillFolder): void {
    const s = this.options.words.skills;
    this.options.confirm.ask({
      title: s.removeTitle(kept.name),
      what: s.removeWhat,
      go: s.remove,
      act: async () => {
        if (!(await this.options.calls.remove(login.hubName, login.account, kept.name, kept.version))) throw new Error("the skill changed meanwhile");
      },
      done: () => {
        this.editing = null;
        this.saidNow();
        void this.load(login);
      },
    });
  }

  private saidNow(): void {
    this.said = true;
    clearTimeout(this.saidTimer);
    this.saidTimer = window.setTimeout(() => {
      this.said = false;
      this.drawStatus();
    }, SAID_MS);
    this.drawStatus();
  }

  private drawStatus(): void {
    const { words } = this.options;
    const html = this.said ? `<span class="prof-said" title="${escape(words.skills.takesEffect)}">${escape(words.saved)}</span>` : "";
    if (this.status.innerHTML !== html) this.status.innerHTML = html;
  }
}
