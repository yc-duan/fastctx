// The account's preferences (`world-user-config.md`, 设置): the few Claude
// Code and Codex settings it may set for the sessions that work for it, laid
// out as the device's settings are, each a row of choices whose first is
// 「默认」, the login's own. A choice is saved as it is pressed, over the
// version read; written elsewhere in the meantime, the page shows that
// version and says so.

import { escape } from "../conversation/prose";
import { glyph } from "../conversation/glyphs";
import type { Login } from "../world/accounts";
import type { Profile, ProfileView } from "../world/profile";
import { AccountChoice, loginKey, type ProfileCalls } from "./profile-account";
import type { ProfileWords } from "./profile-words";
import { detailOf } from "./problem";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";

export interface PrefsPageOptions {
  store: Store;
  words: ProfileWords;
  head(title: string, actions?: string): string;
  calls: ProfileCalls;
}

type Host = "claude" | "codex";

/** One setting: a row of choices, the first the login's own; or a word. */
type Pref =
  | { host: Host; key: string; label: string; kind: "choices"; choices: [unknown, string][] }
  | { host: Host; key: string; label: string; kind: "word"; placeholder: string };

const SAID_MS = 2400;

type State = { kind: "none" } | { kind: "reading" } | { kind: "failed"; detail: string } | { kind: "older" } | { kind: "ready"; view: ProfileView };

export class PrefsPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly body: HTMLElement;
  private readonly status: HTMLElement;
  private readonly account: AccountChoice;
  private readonly stop: () => void;
  private state: State = { kind: "none" };
  private shownLogin: string | null = null;
  private saving = false;
  private saidTimer = 0;
  private note: { text: string; detail: string } | null = null;
  private said = false;
  private readonly prefs: Pref[];

  constructor(private readonly options: PrefsPageOptions) {
    const { words } = options;
    const p = words.prefs;
    const toggle = (host: Host, key: string, label: string): Pref => ({ host, key, label, kind: "choices", choices: [[undefined, p.standard], [true, p.on], [false, p.off]] });
    this.prefs = [
      { host: "claude", key: "language", label: p.language, kind: "word", placeholder: p.languagePlaceholder },
      { host: "claude", key: "outputStyle", label: p.outputStyle, kind: "choices", choices: [[undefined, p.standard], ["Explanatory", p.styles.explanatory], ["Learning", p.styles.learning]] },
      toggle("claude", "alwaysThinkingEnabled", p.thinking),
      toggle("claude", "showThinkingSummaries", p.summaries),
      toggle("claude", "includeGitInstructions", p.git),
      { host: "claude", key: "attribution", label: p.attribution, kind: "choices", choices: [[undefined, p.standard], [{ commit: "", pr: "" }, p.unsigned]] },
      {
        host: "codex",
        key: "model_reasoning_summary",
        label: p.reasoningSummary,
        kind: "choices",
        choices: [[undefined, p.standard], ["auto", p.summaryKinds.auto], ["concise", p.summaryKinds.concise], ["detailed", p.summaryKinds.detailed], ["none", p.summaryKinds.none]],
      },
      { host: "codex", key: "model_verbosity", label: p.verbosity, kind: "choices", choices: [[undefined, p.standard], ["low", p.verbosityKinds.low], ["medium", p.verbosityKinds.medium], ["high", p.verbosityKinds.high]] },
      toggle("codex", "hide_agent_reasoning", p.hideReasoning),
      toggle("codex", "show_raw_agent_reasoning", p.rawReasoning),
    ];
    this.title = words.sections.prefs;
    this.account = new AccountChoice(words, (login) => void this.load(login));
    this.element = document.createElement("div");
    this.element.className = "prof-page prefs-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(words.sections.prefs)}</h1><span class="prof-account-place"></span>`, `<span class="prof-status" role="status"></span>`)}
      <div class="prof-body scroller"></div>`;
    this.element.querySelector(".prof-account-place")!.replaceWith(this.account.element);
    this.body = this.element.querySelector<HTMLElement>(".prof-body")!;
    this.status = this.element.querySelector<HTMLElement>(".prof-status")!;
    this.body.addEventListener("click", (event) => {
      const choice = (event.target as HTMLElement).closest<HTMLElement>("[data-pref]");
      if (!choice) return;
      const pref = this.prefs[Number(choice.dataset.pref)];
      if (pref?.kind !== "choices") return;
      void this.set(pref, pref.choices[Number(choice.dataset.choice)]?.[0]);
    });
    // A word is saved once it is left, or on Enter.
    this.body.addEventListener("change", (event) => {
      const input = (event.target as HTMLElement).closest<HTMLInputElement>("input[data-pref]");
      const pref = input ? this.prefs[Number(input.dataset.pref)] : undefined;
      if (!input || pref?.kind !== "word") return;
      const word = input.value.trim();
      void this.set(pref, word || undefined);
    });
    this.body.addEventListener("keydown", (event) => {
      const input = (event.target as HTMLElement).closest<HTMLInputElement>("input[data-pref]");
      if (input && event.key === "Enter") input.blur();
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

  private async load(login: Login): Promise<void> {
    const key = loginKey(login);
    this.shownLogin = key;
    this.note = null;
    if (this.state.kind !== "ready") {
      this.state = { kind: "reading" };
      this.draw();
    }
    try {
      const view = await this.options.calls.read(login.hubName, login.account);
      if (this.shownLogin !== key) return;
      this.state = view.kept ? { kind: "ready", view } : { kind: "older" };
    } catch (error) {
      if (this.shownLogin === key) this.state = { kind: "failed", detail: detailOf(error) };
    }
    this.draw();
  }

  private draw(): void {
    const { words } = this.options;
    const state = this.state;
    let html: string;
    switch (state.kind) {
      case "none":
        html = "";
        break;
      case "reading":
        html = `<p class="prof-quiet prof-board">${escape(words.reading)}</p>`;
        break;
      case "failed":
        html = `<p class="prof-quiet prof-board" title="${escape(state.detail)}">${escape(words.readFailed)}</p>`;
        break;
      case "older":
        html = `<div class="prof-empty">${glyph("server", 40)}<p class="prof-quiet">${escape(words.olderHub)}</p></div>`;
        break;
      case "ready":
        html = (["claude", "codex"] as const)
          .map(
            (host) => `<section class="home-section prof-board"><h2 class="section-title"><span class="prefs-host">${glyph(host, 16)}${escape(words.sections[host])}</span></h2>${this.prefs
              .map((pref, index) => (pref.host === host ? this.row(pref, index, state.view.profile) : ""))
              .join("")}</section>`,
          )
          .join("");
        break;
    }
    // A word being typed is not drawn over.
    const typing = this.body.querySelector<HTMLInputElement>("input[data-pref]:focus");
    if (typing && state.kind === "ready") {
      for (const button of this.body.querySelectorAll<HTMLElement>("[data-pref][data-choice]")) {
        const pref = this.prefs[Number(button.dataset.pref)];
        if (pref?.kind !== "choices") continue;
        const on = same(valueOf(state.view.profile, pref), pref.choices[Number(button.dataset.choice)]?.[0]);
        button.classList.toggle("chosen", on);
        button.setAttribute("aria-checked", String(on));
      }
    } else if (this.body.dataset.drawn !== html) {
      this.body.innerHTML = html;
      this.body.dataset.drawn = html;
    }
    this.drawStatus();
  }

  private row(pref: Pref, index: number, profile: Profile): string {
    const value = valueOf(profile, pref);
    if (pref.kind === "word") {
      return `<label class="choice-row"><span class="choice-label">${escape(pref.label)}</span><span class="choice-options"><input class="prefs-word" type="text" data-pref="${index}" value="${escape(typeof value === "string" ? value : "")}" placeholder="${escape(pref.placeholder)}" spellcheck="false" autocomplete="off"></span></label>`;
    }
    // A value set elsewhere that is none of the choices is shown as itself.
    const known = pref.choices.some(([choice]) => same(choice, value));
    const choices = known || value === undefined ? pref.choices : [...pref.choices, [value, String(value)] as [unknown, string]];
    return `<div class="choice-row" role="radiogroup" aria-label="${escape(pref.label)}"><span class="choice-label">${escape(pref.label)}</span><div class="choice-options">${choices
      .map(([choice, label], at) => {
        const on = same(choice, value);
        return `<button type="button" class="choice${on ? " chosen" : ""}" role="radio" aria-checked="${on}" data-pref="${index}" data-choice="${at}"${this.saving ? " disabled" : ""}>${escape(label)}</button>`;
      })
      .join("")}</div></div>`;
  }

  private drawStatus(): void {
    const { words } = this.options;
    const html = this.note
      ? `<span class="prof-warn" title="${escape(this.note.detail)}">${escape(this.note.text)}</span>`
      : this.saving
        ? `<span class="prof-said">${escape(words.saving)}</span>`
        : this.said
          ? `<span class="prof-said" title="${escape(words.takesEffect)}">${escape(words.saved)}</span>`
          : "";
    if (this.status.innerHTML !== html) this.status.innerHTML = html;
  }

  private async set(pref: Pref, value: unknown): Promise<void> {
    const login = this.account.chosen();
    if (this.saving || !login || this.state.kind !== "ready") return;
    const view = this.state.view;
    if (same(valueOf(view.profile, pref), value)) return;
    const table = { ...(view.profile[pref.host] ?? {}) };
    if (value === undefined) delete table[pref.key];
    else table[pref.key] = value;
    const profile: Profile = { ...view.profile, [pref.host]: table };
    this.saving = true;
    this.note = null;
    this.said = false;
    this.draw();
    try {
      const version = await this.options.calls.write(login.hubName, login.account, profile);
      view.profile = { ...profile, version };
      this.said = true;
      clearTimeout(this.saidTimer);
      this.saidTimer = window.setTimeout(() => {
        this.said = false;
        this.drawStatus();
      }, SAID_MS);
    } catch (error) {
      const detail = detailOf(error);
      try {
        const now = await this.options.calls.read(login.hubName, login.account);
        const elsewhere = now.profile.version !== view.profile.version;
        this.state = { kind: "ready", view: now };
        this.note = { text: elsewhere ? this.options.words.changedElsewhere : this.options.words.saveFailed, detail };
      } catch {
        this.note = { text: this.options.words.saveFailed, detail };
      }
    } finally {
      this.saving = false;
      this.draw();
    }
  }
}

function valueOf(profile: Profile, pref: Pref): unknown {
  return profile[pref.host]?.[pref.key];
}

/** Whether two settings' values are one: a table by what it holds. */
function same(one: unknown, other: unknown): boolean {
  return JSON.stringify(one) === JSON.stringify(other);
}
