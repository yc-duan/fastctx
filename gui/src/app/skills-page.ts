// The account's skills (`world-user-config.md`, 技能; `world-gui.md`, 智能体页):
// a section of the agents page beside the prompt and the MCP servers, its
// head the page's name and the account's pill. Each skill a row, its name
// and when it applies; a row opens it to rewrite (`./skill-sheet`), 「新建」
// writes another. What the conversations working for the account are told
// follows each change at once.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { Skill, SkillCalls, SkillListed } from "../world/skills";
import type { Confirm } from "./confirm";
import { AccountChoice, loginKey } from "./profile-account";
import type { ProfileWords } from "./profile-words";
import { detailOf } from "./problem";
import type { Page } from "./shell";
import { effect } from "./signals";
import { SkillSheet } from "./skill-sheet";
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
  private readonly sheet: SkillSheet;
  private readonly stop: () => void;
  private state: State = { kind: "none" };
  private shownLogin: string | null = null;
  private said = false;
  private saidTimer = 0;

  constructor(private readonly options: SkillsPageOptions) {
    const { words } = options;
    this.title = words.sections.skills;
    this.choice = new AccountChoice(words, (login) => void this.load(login));
    this.sheet = new SkillSheet(words, options.close);
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
        const add = `<button class="text-button quiet-button skl-add" type="button" data-skl-add>${glyph("plus", 15)}<span>${escape(s.add)}</span></button>`;
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
    if (state.kind !== "ready" || !login) return;
    const name = target.closest<HTMLElement>("[data-skl-open]")?.dataset.sklOpen;
    if (name === undefined && !target.closest("[data-skl-add]")) return;
    let skill: Skill | null = null;
    if (name !== undefined) {
      try {
        skill = await this.options.calls.read(login.hubName, login.account, name);
      } catch {
        skill = null;
      }
      if (!skill) return void this.load(login);
    }
    const taken = state.skills.map((one) => one.name).filter((one) => one !== name);
    this.sheet.open({
      skill,
      taken,
      save: async (made, previous) => {
        await this.options.calls.save(login.hubName, login.account, previous, made);
        this.saidNow();
        await this.load(login);
      },
      remove: () => skill && this.remove(login, skill.name),
    });
  }

  private remove(login: Login, name: string): void {
    const s = this.options.words.skills;
    this.options.confirm.ask({
      title: s.removeTitle(name),
      what: s.removeWhat,
      go: s.remove,
      act: () => this.options.calls.remove(login.hubName, login.account, name),
      done: () => {
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
