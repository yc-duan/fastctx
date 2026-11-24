// One preset of model collaboration, written or rewritten
// (`world-colleagues.md`): its name and when to ask it, which the calling
// model chooses by; the login it runs in, Codex's or Claude Code's, with its
// model and effort; the text its callers are given, the World's as it reads
// for that login's host until rewritten, and what the account adds to its rules;
// under 「更多」, how many work at once and whether it may use the screen. A
// new one starts from one of the World's presets or from nothing. Where
// presets are not kept, it is a callee as before them, without its texts.
// A rule broken is said beside its field; what the World refuses, under the
// fields, its own words a touch away.

import { effortLabel } from "../conversation/effort-slider";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { inHost, kindOf, sameAgentLogin, type Callee, type PresetKind, type PresetTexts } from "../world/collab";
import { atOnceChoices, atOnceOf, atOnceSaid, type CalleeLogin, type Shown } from "./collab-presets";
import { Sheet } from "./confirm";
import type { FeaturesWords } from "./features-words";
import { detailOf } from "./problem";

export interface PresetOpening {
  /** The one rewritten; none for a new one. */
  shown: Shown | null;
  /** The names the account's others have. */
  taken: string[];
  logins: CalleeLogin[];
  /** The World's texts for its presets. */
  texts: PresetTexts[];
  /** Whether presets are not kept where the settings are. */
  legacy: boolean;
  /** A new preset of a kind, as the World starts one. */
  fresh(kind: PresetKind): Callee;
  /** Saves it over the one it was, or as a new one. */
  save(callee: Callee, previous: string | null): Promise<void>;
  /** Asks to remove the one rewritten. */
  remove(): void;
}

const NAME = /^[a-z0-9-]+$/;
const KINDS: PresetKind[] = ["review", "scout", "own"];

type Choice = "kind" | "login" | "model" | "effort" | "atOnce" | "screen";

export class PresetSheet {
  private readonly sheet = new Sheet("ps-sheet");
  private opening: PresetOpening | null = null;
  /** What the choices came to; the words are read from their fields. */
  private draft: Callee | null = null;

  constructor(
    private readonly words: FeaturesWords,
    private readonly english: boolean,
  ) {
    const body = this.sheet.body;
    body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const choice = target.closest<HTMLElement>("[data-ps-choice]");
      if (choice) {
        this.choose(choice.dataset.psChoice as Choice, choice.dataset.value ?? "");
        return;
      }
      if (target.closest("[data-ps-reset]")) {
        const text = body.querySelector<HTMLTextAreaElement>("[name=prompt]");
        if (text) text.value = this.worldText() ?? "";
        this.drawReset();
        return;
      }
      if (target.closest("[data-ps-remove]")) {
        this.sheet.close();
        this.opening?.remove();
      }
    });
    body.addEventListener("input", (event) => {
      const field = (event.target as HTMLElement).closest<HTMLElement>(".field");
      field?.querySelector<HTMLElement>(".field-hint")?.setAttribute("hidden", "");
      if ((event.target as HTMLElement).matches("[name=prompt]")) this.drawReset();
    });
    body.onsubmit = (event) => {
      event.preventDefault();
      void this.submit();
    };
  }

  open(opening: PresetOpening): void {
    this.opening = opening;
    this.draft = structuredClone(opening.shown?.callee ?? opening.fresh("review"));
    this.draw();
    this.sheet.show();
    // Laid out only once shown.
    this.revealLogin();
    this.sheet.body.querySelector<HTMLElement>(opening.shown ? "textarea[name=use]" : "input[name=name]")?.focus();
  }

  /** The World's text for the callers of the kind drawn, as it reads for
   *  the host of the login chosen; none for one of the account's own. */
  private worldText(): string | undefined {
    const kind = this.draft ? kindOf(this.draft) : "own";
    const text = this.opening?.texts.find((one) => one.kind === kind)?.prompt;
    return text === undefined ? undefined : inHost(text, this.draft?.login.agent ?? "codex");
  }

  private draw(): void {
    const opening = this.opening;
    const callee = this.draft;
    if (!opening || !callee) return;
    const { words } = this;
    const p = words.presets;
    const kind = kindOf(callee);
    const own = kind === "own";
    const field = (name: string, label: string, control: string) =>
      `<label class="field"><span class="field-label">${escape(label)}</span>${control}<span class="field-hint" data-ps-hint="${name}" hidden></span></label>`;
    const choices = (name: Choice, label: string, said = "") =>
      `<div class="field"><span class="field-label"${said ? ` title="${escape(said)}"` : ""}>${escape(label)}</span><div class="choice-options${name === "login" ? " ps-logins" : ""}" data-ps-choices="${name}" role="radiogroup" aria-label="${escape(label)}"></div><span class="field-hint" data-ps-hint="${name}" hidden></span></div>`;
    const text = callee.preset?.prompt ?? this.worldText() ?? "";
    const texts = opening.legacy
      ? ""
      : `<div class="field"><span class="field-label ps-label"><span title="${escape(p.promptSaid)}">${escape(p.prompt)}</span><button class="text-button quiet-button ps-reset" type="button" data-ps-reset hidden>${escape(words.promptReset)}</button></span>
          <textarea class="ps-text" name="prompt" rows="7" spellcheck="false" placeholder="${escape(p.promptHint)}">${escape(text)}</textarea></div>
        ${field("rules", own ? p.ownRules : p.addedRules, `<textarea class="ps-text" name="rules" rows="4" spellcheck="false" placeholder="${escape(own ? p.ownRulesHint : p.addedRulesHint)}">${escape(callee.preset?.rules ?? "")}</textarea>`)}`;
    const kept = opening.shown && !opening.shown.offered;
    this.sheet.body.innerHTML = `${this.sheet.head(opening.shown ? p.editTitle(p.kinds[kind]) : p.newTitle, p.close)}
      ${opening.shown || opening.legacy ? "" : choices("kind", p.startFrom)}
      ${field("name", words.name, `<input name="name" type="text" spellcheck="false" autocomplete="off" autocapitalize="off" maxlength="64" value="${escape(callee.name)}">`)}
      ${field("use", words.useWhen, `<textarea name="use" rows="2" placeholder="${escape(own ? p.useHint : p.lines[kind])}">${escape(callee.use_when)}</textarea>`)}
      ${choices("login", words.login)}
      ${choices("model", words.model)}
      ${choices("effort", words.effort)}
      ${choices("atOnce", words.atOnce, words.atOnceSaid)}
      ${texts}
      <details class="sa-more"><summary>${glyph("chevron", 14)}<span>${escape(p.more)}</span></summary>
        ${choices("screen", words.screen, words.screenSaid)}
      </details>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot">${kept ? `<button class="text-button danger-text" type="button" data-ps-remove>${escape(words.removeCallee)}</button>` : "<span></span>"}<button class="sheet-go" type="submit">${escape(words.save)}</button></footer>`;
    this.drawChoices();
    this.revealLogin();
    this.drawReset();
    this.sheet.anew();
  }

  /** The login chosen, where many scroll in their place, scrolled into it. */
  private revealLogin(): void {
    const list = this.sheet.body.querySelector<HTMLElement>('[data-ps-choices="login"]');
    const chosen = list?.querySelector<HTMLElement>(".choice.chosen");
    if (!list || !chosen) return;
    const top = chosen.offsetTop;
    if (top < list.scrollTop || top + chosen.offsetHeight > list.scrollTop + list.clientHeight) list.scrollTop = Math.max(0, top - 4);
  }

  /** The way back to the World's text, while what is written is another. */
  private drawReset(): void {
    const reset = this.sheet.body.querySelector<HTMLElement>("[data-ps-reset]");
    const text = this.sheet.body.querySelector<HTMLTextAreaElement>("[name=prompt]");
    const world = this.worldText();
    if (reset && text) reset.hidden = world === undefined || text.value === world;
  }

  /** The models of the login chosen, and the one its effort is chosen of. */
  private models() {
    const callee = this.draft;
    const login = callee ? this.opening?.logins.find((one) => sameAgentLogin(one.login, callee.login)) : undefined;
    const models = login?.models ?? [];
    const model = models.find((one) => one.model === callee?.model) ?? models.find((one) => one.is_default) ?? models[0];
    return { models, model };
  }

  /** The rows of choices, as chosen now. */
  private drawChoices(): void {
    const opening = this.opening;
    const callee = this.draft;
    if (!opening || !callee) return;
    const { words } = this;
    const p = words.presets;
    const row = (kind: Choice, choices: [string, string][], chosen: string) =>
      choices
        .map(([value, label]) => {
          const on = value === chosen;
          return `<button type="button" class="choice${on ? " chosen" : ""}" role="radio" aria-checked="${on}" data-ps-choice="${kind}" data-value="${escape(value)}">${escape(label)}</button>`;
        })
        .join("");
    const logins: [string, string][] = opening.logins.map((one, at) => [String(at), one.label]);
    let login = String(opening.logins.findIndex((one) => sameAgentLogin(one.login, callee.login)));
    // A login its machine does not tell of now stays chosen, as it is kept.
    if (login === "-1" && callee.login.machine) logins.push([(login = "kept"), `${callee.login.machine} ${callee.login.home ?? ""}`.trim()]);
    const { models, model } = this.models();
    const offered: [string, string][] = models.map((one) => [one.model, one.display_name]);
    if (callee.model && !offered.some(([value]) => value === callee.model)) offered.push([callee.model, callee.model]);
    const efforts: [string, string][] = (model?.reasoning_efforts ?? []).map((one) => [one.effort, effortLabel(one.effort, this.english)]);
    if (callee.effort && !efforts.some(([value]) => value === callee.effort)) efforts.push([callee.effort, effortLabel(callee.effort, this.english)]);
    const atOnce = atOnceChoices(atOnceOf(callee), words.noLimit);
    const rows: Record<Choice, string> = {
      kind: row("kind", KINDS.map((kind) => [kind, p.kinds[kind]]), kindOf(callee)),
      login: logins.length ? row("login", logins, login) : `<span class="choice-none">${escape(words.noLogins)}</span>`,
      model: row("model", [["", words.modelDefault], ...offered], callee.model ?? ""),
      effort: row("effort", [["", words.modelDefault], ...efforts], callee.effort ?? ""),
      atOnce: row("atOnce", atOnce.choices, atOnce.chosen),
      screen: row("screen", [["on", p.allowed], ["off", p.refused]], callee.screen ? "on" : "off"),
    };
    for (const [kind, html] of Object.entries(rows)) {
      const place = this.sheet.body.querySelector<HTMLElement>(`[data-ps-choices="${kind}"]`);
      if (place && place.innerHTML !== html) place.innerHTML = html;
    }
    // The screen is Codex's to use: a login of Claude Code's has no such choice.
    const screen = this.sheet.body.querySelector<HTMLElement>('[data-ps-choices="screen"]')?.closest<HTMLElement>(".field");
    if (screen) screen.hidden = callee.login.agent !== "codex";
  }

  private choose(choice: Choice, value: string): void {
    const opening = this.opening;
    const callee = this.draft;
    if (!opening || !callee) return;
    this.sheet.body.querySelector<HTMLElement>(`[data-ps-hint="${choice}"]`)?.setAttribute("hidden", "");
    switch (choice) {
      case "kind": {
        // Another kind is another start: its name unless one was typed,
        // and its own texts.
        const typed = this.sheet.body.querySelector<HTMLInputElement>("[name=name]")?.value ?? "";
        const next = opening.fresh(value as PresetKind);
        this.draft = { ...next, name: typed && typed !== callee.name ? typed : next.name, login: callee.login.machine ? callee.login : next.login };
        this.draw();
        return;
      }
      case "login": {
        const login = opening.logins[Number(value)]?.login;
        if (!login || sameAgentLogin(login, callee.login)) return;
        // The World's text as it read for the login before reads for this one.
        const text = this.sheet.body.querySelector<HTMLTextAreaElement>("[name=prompt]");
        const world = text !== null && text.value === this.worldText();
        callee.login = login;
        callee.model = null;
        callee.effort = null;
        if (world && text) text.value = this.worldText() ?? "";
        this.drawReset();
        break;
      }
      case "model":
        callee.model = value || null;
        // An effort the model chosen does not offer goes back to its own.
        if (callee.effort && !this.models().model?.reasoning_efforts.some((one) => one.effort === callee.effort)) callee.effort = null;
        break;
      case "effort":
        callee.effort = value || null;
        break;
      case "atOnce":
        callee.at_once = atOnceSaid(value);
        break;
      case "screen":
        callee.screen = value === "on";
        break;
    }
    this.drawChoices();
  }

  /** Says `text` beside the field `name`, and goes to it. */
  private wrong(name: string, text: string): void {
    const hint = this.sheet.body.querySelector<HTMLElement>(`[data-ps-hint="${name}"]`);
    if (!hint) return;
    hint.textContent = text;
    hint.hidden = false;
    hint.closest(".field")?.querySelector<HTMLElement>("input, textarea, .choice")?.focus();
  }

  private async submit(): Promise<void> {
    const opening = this.opening;
    const draft = this.draft;
    if (!opening || !draft || this.sheet.busy) return;
    const p = this.words.presets;
    const form = this.sheet.body;
    const value = (name: string) => form.querySelector<HTMLInputElement | HTMLTextAreaElement>(`[name="${name}"]`)?.value ?? "";
    const name = value("name").trim();
    if (!NAME.test(name)) return this.wrong("name", this.words.namePattern);
    if (opening.taken.includes(name)) return this.wrong("name", p.taken);
    const kind = kindOf(draft);
    const use_when = value("use").trim();
    if (kind === "own" && !use_when) return this.wrong("use", p.useNeeded);
    if (!draft.login.machine) return this.wrong("login", p.loginNeeded);
    const callee: Callee = { ...draft, name, use_when };
    if (opening.legacy) delete callee.preset;
    else {
      const prompt = value("prompt");
      const rules = value("rules").trim();
      // The World's own words are not kept: its preset follows each release.
      callee.preset = { kind: draft.preset?.kind ?? kind, ...(prompt === this.worldText() ? {} : { prompt }), ...(rules ? { rules } : {}) };
    }
    this.sheet.busy = true;
    const go = form.querySelector<HTMLButtonElement>(".sheet-go");
    if (go) go.disabled = true;
    try {
      await opening.save(callee, opening.shown && !opening.shown.offered ? opening.shown.callee.name : null);
      this.sheet.busy = false;
      this.sheet.close();
    } catch (error) {
      this.sheet.busy = false;
      if (go) go.disabled = false;
      const note = form.querySelector<HTMLElement>(".sheet-note");
      if (note) {
        note.innerHTML = `${escape(this.words.failed(detailOf(error)))}<span class="sheet-note-detail">${escape(detailOf(error))}</span>`;
        note.hidden = false;
      }
    }
  }
}
