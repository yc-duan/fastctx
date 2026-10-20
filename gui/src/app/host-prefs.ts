// The account's preferences for one agent (`world-user-config.md`, 偏好;
// `world-gui.md`, 智能体页): the tab beside that agent's logins. What is
// asked of an agent first comes first: the model a conversation begins with
// and how hard it reasons, then how far its context grows and when it is
// compacted; the rest below. A row of steps is the hand-drawn slider the
// conversation's own effort uses (`../conversation/effort-slider`), with
// 「默认」 before it: left to the login's own setting, the knob rests faint
// where that stands, and a step pressed or dragged to makes it the
// account's. A choice is saved as it is made, over the version read;
// written elsewhere in the meantime, the page shows that version and says
// so.

import { EffortSlider, effortLabel, type EffortStep } from "../conversation/effort-slider";
import { glyph } from "../conversation/glyphs";
import { Popover } from "../conversation/popover";
import { escape } from "../conversation/prose";
import type { AgentModel } from "../world/agents";
import type { Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { Profile, ProfileView } from "../world/profile";
import { AccountChoice, loginKey, type ProfileCalls } from "./profile-account";
import type { ProfileWords } from "./profile-words";
import { detailOf } from "./problem";
import { hostContext, type HostContext } from "./host-context";

export type Host = "claude" | "codex";
type Table = Record<string, unknown>;

export interface HostPrefsOptions {
  agent: Host;
  words: ProfileWords;
  english: boolean;
  calls: ProfileCalls;
  /** The models this agent's logins offer, across the user's machines. */
  models(agent: Host): Promise<AgentModel[]>;
}

/** A preference laid out as a slider of steps, and how it sits in the
 *  agent's table of settings. */
interface Slid {
  kind: "slider";
  id: string;
  label: string;
  steps(table: Table): EffortStep[];
  /** The step set in `table`, or null where it is left to the default. */
  get(table: Table): string | null;
  set(table: Table, value: string | null): Table;
  /** Where the knob rests with none set, in steps, and what it says. */
  resting(table: Table): { at: number; label: string };
  /** Said on hover over the slider. */
  tip(table: Table): string;
}

type Row =
  | { kind: "model"; id: string; label: string; key: string }
  | Slid
  | { kind: "choices"; id: string; label: string; key: string; choices: [unknown, string][] }
  | { kind: "word"; id: string; label: string; key: string; placeholder: string };

interface Section {
  title: string;
  rows: Row[];
}

type State = { kind: "none" } | { kind: "reading" } | { kind: "failed"; detail: string } | { kind: "older" } | { kind: "ready"; view: ProfileView };

const SAID_MS = 2400;

/** The efforts each host's settings take when no model says. */
const EFFORTS: Record<Host, string[]> = {
  claude: ["low", "medium", "high", "xhigh"],
  codex: ["low", "medium", "high", "xhigh"],
};
/** The setting each host keeps a model and an effort in. */
const MODEL_KEY: Record<Host, string> = { claude: "model", codex: "model" };
const EFFORT_KEY: Record<Host, string> = { claude: "effortLevel", codex: "model_reasoning_effort" };

export class HostPrefs {
  /** The tab's body, scrolling on its own. */
  readonly element: HTMLElement;
  /** The head's account pill, where there is a choice of accounts. */
  readonly account: HTMLElement;
  /** What became of the last choice, in the head. */
  readonly status: HTMLElement;
  private readonly choice: AccountChoice;
  private readonly sections: Section[];
  private readonly context: HostContext;
  private readonly sliders = new Map<string, EffortSlider>();
  private state: State = { kind: "none" };
  private shownLogin: string | null = null;
  private visible = false;
  private built = false;
  private saving = false;
  private said = false;
  private saidTimer = 0;
  private note: { text: string; detail: string } | null = null;
  private models: AgentModel[] | null = null;
  private asked = false;
  private menu: { element: HTMLElement; popover: Popover } | null = null;

  constructor(private readonly options: HostPrefsOptions) {
    const { words, agent } = options;
    const p = words.prefs;
    this.context = hostContext(agent, options.english, p, (table) => this.model(table));
    this.sections = [
      { title: p.modelSection, rows: [{ kind: "model", id: "model", label: p.model, key: MODEL_KEY[agent] }, this.effortRow()] },
      { title: p.contextSection, rows: [this.context.window, this.context.compact] },
      { title: p.otherSection, rows: otherRows(agent, p) },
    ];
    this.choice = new AccountChoice(words, (login) => void this.load(login));
    this.account = this.choice.element;
    this.status = document.createElement("span");
    this.status.className = "prof-status";
    this.status.setAttribute("role", "status");
    this.element = document.createElement("div");
    this.element.className = "host-prefs scroller";
    this.element.hidden = true;
    this.element.addEventListener("click", (event) => this.click(event));
    // A word is saved once it is left, or on Enter.
    this.element.addEventListener("change", (event) => {
      const input = (event.target as HTMLElement).closest<HTMLInputElement>("input[data-hp-word]");
      const row = input ? this.row(input.dataset.hpWord!) : undefined;
      if (!input || row?.kind !== "word") return;
      void this.save((table) => put(table, row.key, input.value.trim() || undefined));
    });
    this.element.addEventListener("keydown", (event) => {
      const input = (event.target as HTMLElement).closest<HTMLInputElement>("input[data-hp-word]");
      if (input && event.key === "Enter") input.blur();
    });
  }

  /** Whether there is an account whose preferences these are. */
  available(): boolean {
    return this.choice.chosen() !== null;
  }

  /** The World as it now stands: the account pill, and the account's
   *  preferences read again once another is chosen. */
  draw(world: WorldNow | null): void {
    this.choice.draw(world);
    const login = this.choice.chosen();
    if (this.visible && login && loginKey(login) !== this.shownLogin) void this.load(login);
  }

  /** The tab is shown: what it shows is read the first time. */
  shown(): void {
    this.visible = true;
    const login = this.choice.chosen();
    if (login && loginKey(login) !== this.shownLogin) void this.load(login);
    if (!this.asked) {
      this.asked = true;
      this.options
        .models(this.options.agent)
        .then((models) => {
          this.models = models;
        })
        .catch(() => {
          this.models = [];
        })
        .finally(() => this.refresh());
    }
  }

  leave(): void {
    clearTimeout(this.saidTimer);
    this.choice.leave();
    this.menu?.element.remove();
    this.menu = null;
  }

  private row(id: string): Row | undefined {
    return this.sections.flatMap((section) => section.rows).find((row) => row.id === id);
  }

  private table(): Table {
    return this.state.kind === "ready" ? { ...(this.state.view.profile[this.options.agent] ?? {}) } : {};
  }

  /** The model a session would begin with: the account's, else the one
   *  its logins call their default. */
  private model(table: Table): AgentModel | null {
    const models = this.models ?? [];
    const chosen = table[MODEL_KEY[this.options.agent]];
    return models.find((model) => model.model === chosen) ?? models.find((model) => model.is_default) ?? null;
  }

  private effortRow(): Slid {
    const agent = this.options.agent;
    const key = EFFORT_KEY[agent];
    const steps = (table: Table): EffortStep[] => {
      const offered = this.model(table)?.reasoning_efforts.map((effort) => effort.effort) ?? [];
      const efforts = offered.length ? offered.filter((effort) => EFFORTS[agent].includes(effort) || agent === "codex") : EFFORTS[agent];
      return efforts.map((value) => ({ value, label: effortLabel(value, this.options.english) }));
    };
    return {
      kind: "slider",
      id: "effort",
      label: this.options.words.prefs.effort,
      steps,
      get: (table) => (typeof table[key] === "string" ? (table[key] as string) : null),
      set: (table, value) => put(table, key, value ?? undefined),
      resting: (table) => {
        const shown = steps(table);
        const standard = this.model(table)?.default_reasoning_effort;
        const at = shown.findIndex((step) => step.value === standard);
        return at >= 0 ? { at, label: shown[at]!.label } : { at: Math.floor((shown.length - 1) / 2), label: this.options.words.prefs.standard };
      },
      tip: () => "",
    };
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
      const view = await this.options.calls.read(login.hubName, login.account);
      if (this.shownLogin !== key) return;
      this.state = view.kept ? { kind: "ready", view } : { kind: "older" };
    } catch (error) {
      if (this.shownLogin === key) this.state = { kind: "failed", detail: detailOf(error) };
    }
    this.render();
  }

  /** Lays the tab out for the state it is in; a ready one is built once and
   *  then brought up to date in place, its sliders kept. */
  private render(): void {
    const { words } = this.options;
    const state = this.state;
    if (state.kind === "ready") {
      if (!this.built) this.build();
      this.refresh();
      return;
    }
    this.built = false;
    this.sliders.clear();
    this.element.innerHTML =
      state.kind === "reading"
        ? `<p class="prof-quiet prof-board hp-first">${escape(words.reading)}</p>`
        : state.kind === "failed"
          ? `<p class="prof-quiet prof-board hp-first" title="${escape(state.detail)}">${escape(words.readFailed)}</p>`
          : state.kind === "older"
            ? `<div class="prof-empty">${glyph("server", 40)}<p class="prof-quiet">${escape(words.olderHub)}</p></div>`
            : "";
    this.drawStatus();
  }

  private build(): void {
    this.element.innerHTML = `<div class="prof-board hp-board">${this.sections
      .map(
        (section) =>
          `<section class="home-section hp-section"><h2 class="section-title">${escape(section.title)}</h2>${section.rows.map((row) => this.rowHtml(row)).join("")}</section>`,
      )
      .join("")}</div>`;
    this.sliders.clear();
    for (const row of this.sections.flatMap((section) => section.rows)) {
      if (row.kind !== "slider") continue;
      const slider = new EffortSlider(row.label, (value) => void this.save((table) => row.set(table, value)));
      slider.element.classList.add("hp-slider");
      this.element.querySelector(`[data-hp-slider="${row.id}"]`)!.replaceWith(slider.element);
      this.sliders.set(row.id, slider);
    }
    this.built = true;
  }

  private rowHtml(row: Row): string {
    const { words } = this.options;
    const label = `<span class="choice-label">${escape(row.label)}</span>`;
    switch (row.kind) {
      case "model":
        return `<div class="choice-row hp-row">${label}<div class="choice-options"><span class="hp-model-place"><button class="choice hp-model" type="button" data-hp-model aria-haspopup="menu"><span class="hp-model-name"></span><span class="hp-model-sub"></span>${glyph("chevron", 14)}</button></span></div></div>`;
      case "slider":
        return `<div class="choice-row hp-row hp-slid" data-hp-row="${row.id}">${label}<div class="choice-options hp-slid-options"><button class="choice hp-reset" type="button" data-hp-reset="${row.id}" title="${escape(words.prefs.standardTip)}">${escape(words.prefs.standard)}</button><span data-hp-slider="${row.id}"></span></div></div>`;
      case "choices":
        return `<div class="choice-row hp-row" role="radiogroup" aria-label="${escape(row.label)}">${label}<div class="choice-options" data-hp-choices="${row.id}"></div></div>`;
      case "word":
        return `<label class="choice-row hp-row">${label}<span class="choice-options"><input class="prefs-word" type="text" data-hp-word="${row.id}" placeholder="${escape(row.placeholder)}" spellcheck="false" autocomplete="off"></span></label>`;
    }
  }

  /** Brings every row up to what is kept now. */
  private refresh(): void {
    if (this.state.kind !== "ready" || !this.built) {
      this.drawStatus();
      return;
    }
    const { words } = this.options;
    const table = this.table();
    for (const row of this.sections.flatMap((section) => section.rows)) {
      switch (row.kind) {
        case "model": {
          const chosen = table[row.key];
          const model = (this.models ?? []).find((one) => one.model === chosen);
          const standard = (this.models ?? []).find((one) => one.is_default);
          const name = this.element.querySelector<HTMLElement>(".hp-model-name")!;
          const sub = this.element.querySelector<HTMLElement>(".hp-model-sub")!;
          name.textContent = typeof chosen === "string" ? (model?.display_name ?? chosen) : words.prefs.standard;
          sub.textContent = typeof chosen === "string" ? "" : (standard?.display_name ?? "");
          this.element.querySelector(".hp-model")!.classList.toggle("chosen", typeof chosen === "string");
          break;
        }
        case "slider": {
          const slider = this.sliders.get(row.id)!;
          const steps = row.steps(table);
          const value = row.get(table);
          if (value !== null && steps.some((step) => step.value === value)) slider.set(steps, value);
          else {
            const rest = row.resting(table);
            slider.unset(steps, rest.at, rest.label);
          }
          slider.element.title = row.tip(table);
          const reset = this.element.querySelector<HTMLElement>(`[data-hp-reset="${row.id}"]`)!;
          reset.classList.toggle("chosen", value === null);
          reset.setAttribute("aria-pressed", String(value === null));
          break;
        }
        case "choices": {
          const value = table[row.key];
          const known = row.choices.some(([choice]) => same(choice, value));
          // A value set elsewhere that is none of the choices is shown as itself.
          const choices = known || value === undefined ? row.choices : [...row.choices, [value, String(value)] as [unknown, string]];
          const html = choices
            .map(([choice, label], at) => {
              const on = same(choice, value);
              return `<button type="button" class="choice${on ? " chosen" : ""}" role="radio" aria-checked="${on}" data-hp-choice="${row.id}" data-hp-at="${at}">${escape(label)}</button>`;
            })
            .join("");
          const place = this.element.querySelector<HTMLElement>(`[data-hp-choices="${row.id}"]`)!;
          if (place.dataset.drawn !== html) {
            place.innerHTML = html;
            place.dataset.drawn = html;
          }
          break;
        }
        case "word": {
          const input = this.element.querySelector<HTMLInputElement>(`[data-hp-word="${row.id}"]`)!;
          // A word being typed is not drawn over.
          if (document.activeElement !== input) input.value = typeof table[row.key] === "string" ? (table[row.key] as string) : "";
          break;
        }
      }
    }
    this.element.classList.toggle("hp-saving", this.saving);
    this.drawStatus();
  }

  private click(event: Event): void {
    const target = event.target as HTMLElement;
    if (this.state.kind !== "ready") return;
    const reset = target.closest<HTMLElement>("[data-hp-reset]");
    if (reset) {
      const row = this.row(reset.dataset.hpReset!);
      if (row?.kind === "slider" && row.get(this.table()) !== null) void this.save((table) => row.set(table, null));
      return;
    }
    const choice = target.closest<HTMLElement>("[data-hp-choice]");
    if (choice) {
      const row = this.row(choice.dataset.hpChoice!);
      if (row?.kind !== "choices") return;
      const value = row.choices[Number(choice.dataset.hpAt)]?.[0];
      void this.save((table) => put(table, row.key, value));
      return;
    }
    const pick = target.closest<HTMLElement>("[data-hp-pick]");
    if (pick) {
      this.menu?.popover.hide();
      const value = pick.dataset.hpPick || undefined;
      void this.save((table) => put(table, MODEL_KEY[this.options.agent], value));
      return;
    }
    if (target.closest("[data-hp-model]")) this.openModels();
  }

  /** The models to choose from: 「默认」, the login's own, then each a
   *  login offers. */
  private openModels(): void {
    const anchor = this.element.querySelector<HTMLElement>(".hp-model");
    const place = this.element.querySelector<HTMLElement>(".hp-model-place");
    if (!anchor || !place) return;
    const { words } = this.options;
    const chosen = this.table()[MODEL_KEY[this.options.agent]];
    const models = this.models ?? [];
    const standard = models.find((model) => model.is_default);
    const item = (value: string, name: string, sub: string, on: boolean) =>
      `<button class="agents-pick-item${on ? " current" : ""}" type="button" role="menuitemradio" aria-checked="${on}" data-hp-pick="${escape(value)}"><span class="agents-pick-name">${escape(name)}</span>${sub ? `<span class="agents-pick-sub">${escape(sub)}</span>` : ""}</button>`;
    const html = `<div class="agents-pick-group" role="group">${item("", words.prefs.standard, standard?.display_name ?? "", typeof chosen !== "string")}</div>${
      models.length
        ? `<div class="agents-pick-group" role="group">${models.map((model) => item(model.model, model.display_name, "", model.model === chosen)).join("")}</div>`
        : `<p class="agents-pick-note">${escape(this.models === null ? words.reading : words.prefs.noModels)}</p>`
    }`;
    if (this.menu) {
      this.menu.element.innerHTML = html;
      this.menu.popover.toggle();
      return;
    }
    const element = document.createElement("div");
    element.className = "map-menu agents-pick-menu hp-model-menu";
    element.setAttribute("role", "menu");
    element.innerHTML = html;
    place.append(element);
    const popover = new Popover(element, anchor, () => {});
    this.menu = { element, popover };
    popover.show();
    element.querySelector<HTMLElement>(".agents-pick-item.current")?.focus({ preventScroll: true });
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

  /** Writes the agent's table as `change` leaves it, over the version read. */
  private async save(change: (table: Table) => Table): Promise<void> {
    const login = this.choice.chosen();
    if (this.saving || !login || this.state.kind !== "ready") {
      this.refresh();
      return;
    }
    const view = this.state.view;
    const agent = this.options.agent;
    const before = this.table();
    const after = change({ ...before });
    if (same(before, after)) {
      this.refresh();
      return;
    }
    const profile: Profile = { ...view.profile, [agent]: after };
    this.saving = true;
    this.note = null;
    this.said = false;
    this.refresh();
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
      this.refresh();
    }
  }
}

/** The quieter preferences, below the model and the context. */
function otherRows(agent: Host, p: ProfileWords["prefs"]): Row[] {
  const toggle = (id: string, key: string, label: string): Row => ({ kind: "choices", id, key, label, choices: [[undefined, p.standard], [true, p.on], [false, p.off]] });
  if (agent === "claude") {
    return [
      { kind: "word", id: "language", key: "language", label: p.language, placeholder: p.languagePlaceholder },
      { kind: "choices", id: "outputStyle", key: "outputStyle", label: p.outputStyle, choices: [[undefined, p.standard], ["Explanatory", p.styles.explanatory], ["Learning", p.styles.learning]] },
      toggle("summaries", "showThinkingSummaries", p.summaries),
      toggle("git", "includeGitInstructions", p.git),
      { kind: "choices", id: "attribution", key: "attribution", label: p.attribution, choices: [[undefined, p.standard], [{ commit: "", pr: "" }, p.unsigned]] },
    ];
  }
  return [
    {
      kind: "choices",
      id: "summary",
      key: "model_reasoning_summary",
      label: p.reasoningSummary,
      choices: [[undefined, p.standard], ["auto", p.summaryKinds.auto], ["concise", p.summaryKinds.concise], ["detailed", p.summaryKinds.detailed], ["none", p.summaryKinds.none]],
    },
    { kind: "choices", id: "verbosity", key: "model_verbosity", label: p.verbosity, choices: [[undefined, p.standard], ["low", p.verbosityKinds.low], ["medium", p.verbosityKinds.medium], ["high", p.verbosityKinds.high]] },
    toggle("hide", "hide_agent_reasoning", p.hideReasoning),
    toggle("raw", "show_raw_agent_reasoning", p.rawReasoning),
  ];
}

/** `table` with `key` set to `value`, or without it where there is none. */
export function put(table: Table, key: string, value: unknown): Table {
  const next = { ...table };
  if (value === undefined) delete next[key];
  else next[key] = value;
  return next;
}

/** Whether two settings' values are one: a table by what it holds. */
function same(one: unknown, other: unknown): boolean {
  return JSON.stringify(one) === JSON.stringify(other);
}
