// The account's Claude subagents (`world-user-config.md`, 子代理;
// `world-gui.md`, 智能体页): a page come into from Claude Code's
// preferences, its head the way back, its name and the account's pill. The
// subagents its conversations may hand work to, one row each, the defaults
// Fastctx carries until the account changes any of them; a row opens it to
// rewrite (`./subagent-sheet`). Under them, on a sheet of writing paper as
// the prompt is, a few words to the main agent on how to use them. Each
// change is made over the version read; written elsewhere in the meantime,
// the page shows that version and says so.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { effortLabel } from "../conversation/effort-slider";
import { modelName } from "../conversation/view";
import type { Login } from "../world/accounts";
import type { AgentModel } from "../world/agents";
import type { WorldNow } from "../world/bridge";
import { PROMPT_MOST } from "../world/profile";
import type { SubagentOperation, SubagentSpec, SubagentView } from "../world/subagents";
import type { Confirm } from "./confirm";
import { AccountChoice, loginKey } from "./profile-account";
import type { ProfileWords } from "./profile-words";
import { detailOf } from "./problem";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";
import { SubagentSheet } from "./subagent-sheet";

export interface SubagentsPageOptions {
  store: Store;
  head(title: string, actions?: string): string;
  words: ProfileWords;
  english: boolean;
  /** Reads or changes the account's subagents. */
  call(hub: string, account: string, operation: SubagentOperation): Promise<SubagentView>;
  /** The models Claude Code's logins offer, across the user's machines. */
  models(): Promise<AgentModel[]>;
  confirm: Confirm;
  close: string;
}

type State = { kind: "none" } | { kind: "reading" } | { kind: "failed"; detail: string } | { kind: "older" } | { kind: "ready"; view: SubagentView };

const SAID_MS = 2400;

export class SubagentsPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  /** The body, scrolling on its own. */
  private readonly body: HTMLElement;
  /** What became of the last change, in the head. */
  private readonly status: HTMLElement;
  private readonly choice: AccountChoice;
  private readonly sheet: SubagentSheet;
  private readonly stop: () => void;
  private state: State = { kind: "none" };
  private shownLogin: string | null = null;
  private models: AgentModel[] = [];
  private saving = false;
  private said = false;
  private saidTimer = 0;
  private note: { text: string; detail: string } | null = null;
  private drawn = "";

  constructor(private readonly options: SubagentsPageOptions) {
    const { words } = options;
    this.title = words.subagents.agents;
    this.choice = new AccountChoice(words, (login) => void this.load(login));
    this.sheet = new SubagentSheet(words, options.english, options.close);
    this.element = document.createElement("div");
    this.element.className = "prof-page sa-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(this.title)}</h1><span class="sa-account-place"></span>`, `<span class="prof-status" role="status"></span>`)}
      <div class="host-prefs sa-body scroller"></div>`;
    this.element.querySelector(".sa-account-place")!.replaceWith(this.choice.element);
    this.status = this.element.querySelector<HTMLElement>(".prof-status")!;
    this.body = this.element.querySelector<HTMLElement>(".sa-body")!;
    this.body.addEventListener("click", (event) => this.click(event));
    this.body.addEventListener("input", (event) => {
      if ((event.target as HTMLElement).matches(".sa-text")) {
        this.keepDraft();
        this.drawFoot();
      }
    });
    this.body.addEventListener("keydown", (event) => {
      if ((event.target as HTMLElement).matches(".sa-text") && (event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        void this.saveInstructions();
      }
    });
    this.stop = effect(() => this.draw(options.store.world.get()));
    this.options
      .models()
      .then((models) => {
        this.models = models;
      })
      .catch(() => {})
      .finally(() => this.render());
  }

  leave(): void {
    this.stop();
    clearTimeout(this.saidTimer);
    this.choice.leave();
  }

  /** The account pill as the World now stands, and the account's subagents
   *  read again once another is chosen. */
  private draw(world: WorldNow | null): void {
    this.choice.draw(world);
    const login = this.choice.chosen();
    if (login && loginKey(login) !== this.shownLogin) void this.load(login);
  }

  private async load(login: Login): Promise<void> {
    const key = loginKey(login);
    this.shownLogin = key;
    this.note = null;
    if (this.state.kind !== "ready") {
      this.state = { kind: "reading" };
      this.render();
    }
    try {
      const view = await this.options.call(login.hubName, login.account, { operation: "list" });
      if (this.shownLogin !== key) return;
      this.state = view.supported ? { kind: "ready", view } : { kind: "older" };
    } catch (error) {
      if (this.shownLogin === key) this.state = { kind: "failed", detail: detailOf(error) };
    }
    this.render(true);
  }

  /** Lays the page out; the paper is drawn again only when what it holds
   *  was saved or read anew (`fresh`), never over what is being typed. */
  private render(fresh = false): void {
    const { words } = this.options;
    const state = this.state;
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
      case "older":
        html = `<div class="prof-empty">${glyph("server", 40)}<p class="prof-quiet">${escape(words.subagents.olderHub)}</p></div>`;
        break;
      case "ready":
        html = this.board(state.view);
        break;
    }
    const paper = this.element.querySelector<HTMLTextAreaElement>(".sa-text");
    const typed = paper?.value;
    if (html !== this.drawn) {
      this.drawn = html;
      this.body.innerHTML = html;
      const text = this.element.querySelector<HTMLTextAreaElement>(".sa-text");
      if (text) text.value = fresh || typed === undefined ? (this.draft() ?? this.saved()) : typed;
    } else if (fresh && paper) paper.value = this.draft() ?? this.saved();
    this.drawFoot();
    this.drawStatus();
  }

  private board(view: SubagentView): string {
    const s = this.options.words.subagents;
    const agents = view.configuration.agents;
    const tag = view.configured ? "" : `<span class="tag sa-defaults" title="${escape(s.defaultsTip)}">${escape(s.defaults)}</span>`;
    const add = `<button class="text-button quiet-button sa-add" type="button" data-sa-add>${glyph("plus", 15)}<span>${escape(s.add)}</span></button>`;
    const rows = agents.length
      ? `<ul class="sa-rows">${agents.map((agent) => this.row(agent)).join("")}</ul>`
      : `<p class="prof-quiet sa-none">${escape(s.none)}</p>`;
    const reset = view.configured ? `<button class="text-button quiet-button sa-reset" type="button" data-sa-reset>${escape(s.reset)}</button>` : "";
    return `<div class="prof-board hp-board sa-board">
        <section class="home-section sa-section"><h2 class="section-title sa-title"><span>${escape(s.agents)}</span>${tag}${add}</h2>${rows}</section>
        <section class="home-section sa-section"><h2 class="section-title">${escape(s.instructions)}</h2>
          <div class="prompt-paper sa-paper"><textarea class="prompt-text sa-text" spellcheck="false" placeholder="${escape(s.instructionsPlaceholder)}" aria-label="${escape(s.instructions)}"></textarea></div>
          <footer class="prompt-foot sa-foot"><span class="prompt-size"></span><span class="prompt-note" role="status"></span><button class="sheet-go" type="button" data-sa-save title="${escape(s.takesEffect)}">${escape(s.save)}</button></footer>
        </section>
        ${reset ? `<div class="sa-end">${reset}</div>` : ""}
      </div>`;
  }

  /** A subagent: its name, when it is handed work, and what it runs on. */
  private row(agent: SubagentSpec): string {
    const model = typeof agent.model === "string" ? (this.models.find((one) => one.model === agent.model)?.display_name ?? modelName(agent.model)) : "";
    const effort = agent.effort === undefined ? "" : effortLabel(String(agent.effort), this.options.english);
    return `<li><button class="sa-row" type="button" data-sa-open="${escape(agent.name)}">
        <span class="sa-name">${escape(agent.name)}</span>
        <span class="sa-use" title="${escape(agent.description)}">${escape(agent.description)}</span>
        <span class="sa-runs">${model ? `<span class="tag">${escape(model)}</span>` : ""}${effort ? `<span class="tag">${escape(effort)}</span>` : ""}</span>
      </button></li>`;
  }

  private click(event: Event): void {
    const target = event.target as HTMLElement;
    const state = this.state;
    if (state.kind !== "ready") return;
    if (target.closest("[data-sa-save]")) {
      void this.saveInstructions();
      return;
    }
    if (target.closest("[data-sa-theirs]")) {
      const text = this.element.querySelector<HTMLTextAreaElement>(".sa-text");
      if (text) text.value = this.saved();
      this.note = null;
      this.keepDraft();
      this.drawFoot();
      return;
    }
    if (target.closest("[data-sa-reset]")) {
      const s = this.options.words.subagents;
      this.options.confirm.ask({
        title: s.resetTitle,
        what: s.resetWhat,
        go: s.reset,
        act: () => this.change({ operation: "reset", version: this.version() }),
        done: () => {},
      });
      return;
    }
    const taken = (except: string | null) => state.view.configuration.agents.map((agent) => agent.name).filter((name) => name !== except);
    const open = target.closest<HTMLElement>("[data-sa-open]")?.dataset.saOpen;
    const agent = open === undefined ? undefined : state.view.configuration.agents.find((one) => one.name === open);
    if (agent || target.closest("[data-sa-add]")) {
      this.sheet.open({
        agent: agent ?? null,
        taken: taken(agent?.name ?? null),
        models: this.models,
        save: (spec, previous) => this.change({ operation: "save", version: this.version(), previous, agent: spec }),
        remove: () => agent && this.remove(agent.name),
      });
    }
  }

  private remove(name: string): void {
    const s = this.options.words.subagents;
    this.options.confirm.ask({
      title: s.removeTitle(name),
      what: s.removeWhat,
      go: s.remove,
      act: () => this.change({ operation: "delete", version: this.version(), name }),
      done: () => {},
    });
  }

  private version(): number {
    return this.state.kind === "ready" ? this.state.view.version : 0;
  }

  /** Makes `operation` as the account shown, and shows what it left; a
   *  refusal is the caller's to say, after the tab shows the version that
   *  stands now. */
  private async change(operation: SubagentOperation): Promise<void> {
    const login = this.choice.chosen();
    if (!login) return;
    this.saving = true;
    this.note = null;
    this.drawStatus();
    try {
      const view = await this.options.call(login.hubName, login.account, operation);
      this.state = { kind: "ready", view };
      // The defaults come back whole, over anything typed.
      if (operation.operation === "reset") this.forgetDraft();
      this.saidNow();
    } catch (error) {
      await this.reread(login);
      throw error;
    } finally {
      this.saving = false;
      this.render(operation.operation !== "save" && operation.operation !== "delete");
    }
  }

  /** The version that stands now, after a change did not go through. */
  private async reread(login: Login): Promise<void> {
    try {
      const view = await this.options.call(login.hubName, login.account, { operation: "list" });
      if (view.supported) this.state = { kind: "ready", view };
    } catch {
      // What was shown stays.
    }
  }

  private saidNow(): void {
    this.said = true;
    clearTimeout(this.saidTimer);
    this.saidTimer = window.setTimeout(() => {
      this.said = false;
      this.drawStatus();
      this.drawFoot();
    }, SAID_MS);
  }

  private saved(): string {
    return this.state.kind === "ready" ? this.state.view.configuration.instructions : "";
  }

  private draftKey(): string | null {
    return this.shownLogin === null ? null : `profile.subagents.${this.shownLogin}`;
  }

  private draft(): string | null {
    const key = this.draftKey();
    if (!key) return null;
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  }

  private forgetDraft(): void {
    const key = this.draftKey();
    try {
      if (key) localStorage.removeItem(key);
    } catch {
      // Nothing was kept.
    }
  }

  /** What is typed and not saved stays on this device until it is. */
  private keepDraft(): void {
    const key = this.draftKey();
    const text = this.element.querySelector<HTMLTextAreaElement>(".sa-text");
    if (!key || !text) return;
    try {
      if (text.value === this.saved()) localStorage.removeItem(key);
      else localStorage.setItem(key, text.value);
    } catch {
      // Kept on the page only.
    }
  }

  private async saveInstructions(): Promise<void> {
    const text = this.element.querySelector<HTMLTextAreaElement>(".sa-text");
    if (!text || this.saving || this.state.kind !== "ready") return;
    const value = text.value;
    if (value === this.saved() || new TextEncoder().encode(value).length > PROMPT_MOST) return;
    try {
      await this.change({ operation: "instructions", version: this.version(), text: value });
      this.keepDraft();
    } catch (error) {
      const detail = detailOf(error);
      this.note = { text: /the profile changed/.test(detail) ? this.options.words.changedElsewhere : this.options.words.saveFailed, detail };
      this.drawFoot();
    }
  }

  private drawFoot(): void {
    const { words } = this.options;
    const text = this.element.querySelector<HTMLTextAreaElement>(".sa-text");
    const foot = this.element.querySelector<HTMLElement>(".sa-foot");
    if (!text || !foot) return;
    const bytes = new TextEncoder().encode(text.value).length;
    const over = bytes > PROMPT_MOST;
    const changed = text.value !== this.saved();
    const button = foot.querySelector<HTMLButtonElement>("[data-sa-save]")!;
    button.disabled = this.saving || over || !changed;
    const note = foot.querySelector<HTMLElement>(".prompt-note")!;
    const html = this.note
      ? `<span class="prof-warn" title="${escape(this.note.detail)}">${escape(this.note.text)}</span>${/the profile changed/.test(this.note.detail) ? `<button class="text-button" type="button" data-sa-theirs>${escape(words.loadTheirs)}</button>` : ""}`
      : over
        ? `<span class="prof-warn">${escape(words.prompt.tooLong)}</span>`
        : "";
    if (note.innerHTML !== html) note.innerHTML = html;
  }

  private drawStatus(): void {
    const { words } = this.options;
    const html = this.saving
      ? `<span class="prof-said">${escape(words.saving)}</span>`
      : this.said
        ? `<span class="prof-said" title="${escape(words.subagents.takesEffect)}">${escape(words.saved)}</span>`
        : "";
    if (this.status.innerHTML !== html) this.status.innerHTML = html;
  }
}
