// The account's own prompt for one family (`world-user-config.md`, 用户级
// 提示词): what it writes to Claude Code, or to Codex, wherever they work for
// it, a sheet of its own filling the page, come into from that family's card
// of the models' settings. A family not written yet shows the prompt every
// agent was given before the two were apart, which it follows until it is
// saved. It is written over the version it was read at; written elsewhere in
// the meantime, the page says so and offers that version in place of its
// own. What is typed and not yet saved stays on this device until it is.

import { escape } from "../conversation/prose";
import { glyph } from "../conversation/glyphs";
import type { Login } from "../world/accounts";
import { PROMPT_MOST, promptOf, type ProfileView } from "../world/profile";
import { AccountChoice, loginKey, type ProfileCalls } from "./profile-account";
import type { ProfileWords } from "./profile-words";
import { detailOf } from "./problem";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";
import { waiting } from "./waiting";

export interface PromptPageOptions {
  store: Store;
  words: ProfileWords;
  agent: "claude" | "codex";
  /** The family's name, as its card heads it. */
  name: string;
  head(title: string, actions?: string): string;
  calls: ProfileCalls;
}

/** How long 「已保存」 stays after saving. */
const SAID_MS = 2400;
/** How many times a save is written over the version standing now before
 *  it is given up. */
const SAVE_TRIES = 3;
/** The share of the most a prompt may hold past which how much it holds is
 *  shown. */
const SHOWN_FROM = 0.75;

type State =
  | { kind: "none" }
  | { kind: "reading" }
  | { kind: "failed"; detail: string }
  | { kind: "older" }
  | { kind: "ready"; view: ProfileView };

export class PromptPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly body: HTMLElement;
  private readonly account: AccountChoice;
  private readonly stop: () => void;
  private state: State = { kind: "none" };
  private shownLogin: string | null = null;
  private text: HTMLTextAreaElement | null = null;
  private saving = false;
  private saidUntil = 0;
  private saidTimer = 0;
  private note: { text: string; detail: string } | null = null;

  constructor(private readonly options: PromptPageOptions) {
    const { words } = options;
    this.title = words.prompt.title(options.name, options.agent);
    this.account = new AccountChoice(words, (login) => void this.load(login));
    this.element = document.createElement("div");
    this.element.className = "prof-page prompt-page";
    // Its way back names the family, and where that keeps its arrow alone,
    // as on a phone, the head does (`profile.css`).
    this.element.innerHTML = `${options.head(`<h1 class="page-title"><span class="pp-of">${escape(words.prompt.of(options.name))}</span>${escape(words.prompt.named[options.agent])}</h1><span class="prof-account-place"></span>`)}
      <div class="prof-body"></div>`;
    this.element.querySelector(".prof-account-place")!.replaceWith(this.account.element);
    this.body = this.element.querySelector<HTMLElement>(".prof-body")!;
    this.body.addEventListener("input", (event) => {
      if (event.target !== this.text) return;
      this.keepDraft();
      this.drawFoot();
    });
    this.body.addEventListener("keydown", (event) => {
      if (event.target === this.text && (event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        void this.save();
      }
    });
    this.body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target.closest("[data-save]")) void this.save();
    });
    this.stop = effect(() => {
      const world = options.store.world.get();
      this.account.draw(world);
      const login = this.account.chosen();
      if (login && loginKey(login) !== this.shownLogin) void this.load(login);
    });
  }

  leave(): void {
    this.stop();
    clearTimeout(this.saidTimer);
    this.account.leave();
  }

  private login(): Login | null {
    return this.account.chosen();
  }

  private async load(login: Login): Promise<void> {
    const key = loginKey(login);
    this.shownLogin = key;
    this.note = null;
    if (this.state.kind !== "ready") this.setState({ kind: "reading" });
    try {
      const view = await this.options.calls.read(login.hubName, login.account);
      if (this.shownLogin !== key) return;
      this.setState(view.kept && view.families ? { kind: "ready", view } : { kind: "older" });
    } catch (error) {
      if (this.shownLogin === key) this.setState({ kind: "failed", detail: detailOf(error) });
    }
  }

  private setState(state: State): void {
    this.state = state;
    const { words } = this.options;
    this.text = null;
    switch (state.kind) {
      case "none":
        this.body.innerHTML = "";
        return;
      case "reading":
        this.body.innerHTML = waiting({ label: words.reading, shape: "lines", rows: 6 });
        return;
      case "failed":
        this.body.innerHTML = `<p class="prof-quiet" title="${escape(state.detail)}">${escape(words.readFailed)}</p>`;
        return;
      case "older":
        this.body.innerHTML = `<div class="prof-empty">${glyph("server", 40)}<p class="prof-quiet">${escape(words.prompt.olderHub)}</p></div>`;
        return;
      case "ready": {
        const saved = this.saved();
        const { name } = this.options;
        this.body.innerHTML = `<div class="prompt-sheet">
            <div class="prompt-paper"><textarea class="prompt-text" spellcheck="false" placeholder="${escape(words.prompt.placeholder(name))}" aria-label="${escape(this.title)}"></textarea></div>
            <footer class="prompt-foot"><span class="prompt-size"></span><span class="prompt-note" role="status"></span><button class="sheet-go prompt-save" type="button" data-save title="${escape(words.prompt.reaches(name))}">${escape(words.prompt.save)}</button></footer>
          </div>`;
        this.text = this.body.querySelector<HTMLTextAreaElement>(".prompt-text")!;
        this.text.value = this.draft() ?? saved;
        this.drawFoot();
      }
    }
  }

  /** The saved prompt of the account shown, for this family. */
  private saved(): string {
    return this.state.kind === "ready" ? promptOf(this.state.view.profile, this.options.agent) : "";
  }

  private draftKey(): string | null {
    return this.shownLogin === null ? null : `profile.prompt.${this.options.agent}.${this.shownLogin}`;
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

  /** What is typed and not saved stays on this device; once it is saved, or
   *  typed back to what is, it goes. */
  private keepDraft(): void {
    const key = this.draftKey();
    if (!key || !this.text) return;
    try {
      if (this.text.value === this.saved()) localStorage.removeItem(key);
      else localStorage.setItem(key, this.text.value);
    } catch {
      // Kept on the page only.
    }
  }

  private bytes(): number {
    return new TextEncoder().encode(this.text?.value ?? "").length;
  }

  private drawFoot(): void {
    const { words } = this.options;
    const foot = this.body.querySelector<HTMLElement>(".prompt-foot");
    if (!foot || !this.text) return;
    const bytes = this.bytes();
    const over = bytes > PROMPT_MOST;
    const size = foot.querySelector<HTMLElement>(".prompt-size")!;
    size.textContent = bytes > PROMPT_MOST * SHOWN_FROM ? words.prompt.size(kilobytes(bytes), kilobytes(PROMPT_MOST)) : "";
    size.classList.toggle("over", over);
    const changed = this.text.value !== this.saved();
    const button = foot.querySelector<HTMLButtonElement>(".prompt-save")!;
    button.disabled = this.saving || over || !changed;
    button.textContent = this.saving ? words.saving : words.prompt.save;
    const note = foot.querySelector<HTMLElement>(".prompt-note")!;
    const html = this.note
      ? `<span class="prof-warn" title="${escape(this.note.detail)}">${escape(this.note.text)}</span>`
      : over
        ? `<span class="prof-warn">${escape(words.prompt.tooLong)}</span>`
        : !changed && Date.now() < this.saidUntil
          ? `<span class="prof-said" title="${escape(words.takesEffect)}">${escape(words.saved)}</span>`
          : "";
    if (note.innerHTML !== html) note.innerHTML = html;
  }

  private async save(): Promise<void> {
    const login = this.login();
    if (this.saving || !login || this.state.kind !== "ready" || !this.text) return;
    const view = this.state.view;
    const prompt = this.text.value;
    if (prompt === this.saved() || new TextEncoder().encode(prompt).length > PROMPT_MOST) return;
    this.saving = true;
    this.note = null;
    this.drawFoot();
    try {
      // What the user saves stands: written elsewhere since it was read, the
      // profile is read again and this prompt written over the version
      // standing now, the rest of it as it stands (the user, 2026-11-18:
      // "我们不应该管用户在不在别处修改，直接读现成的就行了").
      let standing = view;
      for (let tries = 1; ; tries += 1) {
        const prompts = { ...standing.profile.prompts, [this.options.agent]: prompt };
        try {
          const version = await this.options.calls.write(login.hubName, login.account, { ...standing.profile, prompts });
          standing.profile = { ...standing.profile, prompts, version };
          break;
        } catch (error) {
          if (tries >= SAVE_TRIES) throw error;
          const now = await this.options.calls.read(login.hubName, login.account);
          if (now.profile.version === standing.profile.version) throw error;
          standing = now;
          this.state = { kind: "ready", view: now };
        }
      }
      this.keepDraft();
      this.saidUntil = Date.now() + SAID_MS;
      clearTimeout(this.saidTimer);
      this.saidTimer = window.setTimeout(() => this.drawFoot(), SAID_MS + 50);
    } catch (error) {
      this.note = { text: this.options.words.saveFailed, detail: detailOf(error) };
    } finally {
      this.saving = false;
      this.drawFoot();
    }
  }
}

function kilobytes(bytes: number): string {
  return `${(bytes / 1024).toFixed(1)} KB`;
}
