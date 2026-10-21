// One of the account's Claude subagents, written or rewritten
// (`world-user-config.md`, 子代理): its name, when to hand it work, its
// prompt, and the model and effort it runs with; under 「更多」, the tools it
// may or may not use, how many turns it takes at most, whether it runs in
// the background and whether it reads CLAUDE.md. Whatever of it the sheet
// does not show is kept as it was. A rule broken is said beside its field;
// what the World refuses, under the fields, its own words a touch away.

import { effortLabel } from "../conversation/effort-slider";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { modelName } from "../conversation/view";
import type { AgentModel } from "../world/agents";
import { SUBAGENT_EFFORTS, SUBAGENT_NAME, type SubagentSpec } from "../world/subagents";
import { Sheet } from "./confirm";
import { detailOf } from "./problem";
import type { ProfileWords } from "./profile-words";

export interface SubagentOpening {
  /** The one rewritten; none for a new one. */
  agent: SubagentSpec | null;
  /** The names the account's others have. */
  taken: string[];
  models: AgentModel[];
  /** Saves it over the one it was, or as a new one. */
  save(agent: SubagentSpec, previous: string | null): Promise<void>;
  /** Asks to delete the one rewritten. */
  remove(): void;
}

/** Each choice as the sheet keeps it: none is the default. */
interface Chosen {
  model: string | null;
  effort: string | null;
  background: boolean;
  claudeMd: boolean;
}

export class SubagentSheet {
  private readonly sheet = new Sheet("sa-sheet");
  private opening: SubagentOpening | null = null;
  private chosen: Chosen = { model: null, effort: null, background: false, claudeMd: true };
  private problem: { text: string; detail: string } | null = null;

  constructor(
    private readonly words: ProfileWords,
    private readonly english: boolean,
    private readonly close: string,
  ) {
    const body = this.sheet.body;
    body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const choice = target.closest<HTMLElement>("[data-sa-choice]");
      if (choice) {
        const kind = choice.dataset.saChoice as keyof Chosen;
        const value = choice.dataset.value ?? "";
        if (kind === "background" || kind === "claudeMd") this.chosen[kind] = value === "on";
        else this.chosen[kind] = value || null;
        this.drawChoices();
        return;
      }
      if (target.closest("[data-sa-remove]")) {
        this.sheet.close();
        this.opening?.remove();
      }
    });
    body.addEventListener("input", (event) => {
      const field = (event.target as HTMLElement).closest<HTMLElement>(".field");
      field?.querySelector<HTMLElement>(".field-hint")?.setAttribute("hidden", "");
    });
    body.onsubmit = (event) => {
      event.preventDefault();
      void this.submit();
    };
  }

  open(opening: SubagentOpening): void {
    this.opening = opening;
    this.problem = null;
    const agent = opening.agent;
    this.chosen = {
      model: typeof agent?.model === "string" ? agent.model : null,
      effort: agent?.effort === undefined ? null : String(agent.effort),
      background: agent?.background === true,
      claudeMd: agent?.omitClaudeMd !== true,
    };
    this.draw();
    this.sheet.show();
    this.sheet.body.querySelector<HTMLElement>(agent ? "textarea[name=description]" : "input[name=name]")?.focus();
  }

  private draw(): void {
    const opening = this.opening;
    if (!opening) return;
    const s = this.words.subagents;
    const agent = opening.agent;
    const list = (tools: unknown) => (Array.isArray(tools) ? tools.join(", ") : "");
    const field = (name: string, label: string, control: string) =>
      `<label class="field"><span class="field-label">${escape(label)}</span>${control}<span class="field-hint" data-sa-hint="${name}" hidden></span></label>`;
    this.sheet.body.innerHTML = `${this.sheet.head(agent ? s.editTitle(agent.name) : s.newTitle, this.close)}
      ${field("name", s.name, `<input name="name" type="text" spellcheck="false" autocomplete="off" maxlength="64" value="${escape(agent?.name ?? "")}">`)}
      ${field("description", s.description, `<textarea name="description" rows="2">${escape(agent?.description ?? "")}</textarea>`)}
      ${field("prompt", s.prompt, `<textarea class="sa-prompt" name="prompt" rows="5" spellcheck="false">${escape(agent?.prompt ?? "")}</textarea>`)}
      <div class="field"><span class="field-label">${escape(s.model)}</span><div class="choice-options" data-sa-choices="model"></div></div>
      <div class="field"><span class="field-label">${escape(s.effort)}</span><div class="choice-options" data-sa-choices="effort"></div></div>
      <details class="sa-more"><summary>${glyph("chevron", 14)}<span>${escape(s.more)}</span></summary>
        ${field("tools", s.tools, `<input name="tools" type="text" spellcheck="false" autocomplete="off" placeholder="${escape(s.toolsPlaceholder)}" value="${escape(list(agent?.tools))}">`)}
        ${field("disallowed", s.disallowed, `<input name="disallowed" type="text" spellcheck="false" autocomplete="off" placeholder="${escape(s.toolsPlaceholder)}" value="${escape(list(agent?.disallowedTools))}">`)}
        ${field("maxTurns", s.maxTurns, `<input name="maxTurns" type="text" inputmode="numeric" autocomplete="off" value="${agent?.maxTurns === undefined ? "" : escape(String(agent.maxTurns))}">`)}
        <div class="field"><span class="field-label">${escape(s.background)}</span><div class="choice-options" data-sa-choices="background"></div></div>
        <div class="field"><span class="field-label">${escape(s.claudeMd)}</span><div class="choice-options" data-sa-choices="claudeMd"></div></div>
      </details>
      <p class="sheet-note" role="alert"${this.problem ? "" : " hidden"}>${this.problem ? `${escape(this.problem.text)}<span class="sheet-note-detail">${escape(this.problem.detail)}</span>` : ""}</p>
      <footer class="sheet-foot">${agent ? `<button class="text-button danger-text" type="button" data-sa-remove>${escape(s.remove)}</button>` : "<span></span>"}<button class="sheet-go" type="submit">${escape(s.save)}</button></footer>`;
    this.drawChoices();
    this.sheet.anew();
  }

  /** The rows of choices, as chosen now. */
  private drawChoices(): void {
    const opening = this.opening;
    if (!opening) return;
    const { words } = this;
    const s = words.subagents;
    const row = (kind: keyof Chosen, choices: [string, string][], chosen: string) =>
      choices
        .map(([value, label]) => {
          const on = value === chosen;
          return `<button type="button" class="choice${on ? " chosen" : ""}" role="radio" aria-checked="${on}" data-sa-choice="${kind}" data-value="${escape(value)}">${escape(label)}</button>`;
        })
        .join("");
    const models: [string, string][] = opening.models.map((model) => [model.model, model.display_name]);
    const model = this.chosen.model;
    if (model && !models.some(([value]) => value === model)) models.push([model, modelName(model)]);
    const efforts: [string, string][] = SUBAGENT_EFFORTS.map((effort) => [effort, effortLabel(effort, this.english)]);
    const effort = this.chosen.effort;
    if (effort && !efforts.some(([value]) => value === effort)) efforts.push([effort, effort]);
    const onOff: [string, string][] = [["on", words.prefs.on], ["off", words.prefs.off]];
    const rows: Record<keyof Chosen, string> = {
      model: row("model", [["", s.inherit], ...models], model ?? ""),
      effort: row("effort", [["", words.prefs.standard], ...efforts], effort ?? ""),
      background: row("background", onOff, this.chosen.background ? "on" : "off"),
      claudeMd: row("claudeMd", onOff, this.chosen.claudeMd ? "on" : "off"),
    };
    for (const [kind, html] of Object.entries(rows)) {
      const place = this.sheet.body.querySelector<HTMLElement>(`[data-sa-choices="${kind}"]`);
      if (place && place.innerHTML !== html) place.innerHTML = html;
    }
  }

  /** Says `text` beside the field `name`, and goes to it. */
  private wrong(name: string, text: string): void {
    const hint = this.sheet.body.querySelector<HTMLElement>(`[data-sa-hint="${name}"]`);
    if (!hint) return;
    hint.textContent = text;
    hint.hidden = false;
    hint.closest("details")?.setAttribute("open", "");
    hint.closest(".field")?.querySelector<HTMLElement>("input, textarea")?.focus();
  }

  private async submit(): Promise<void> {
    const opening = this.opening;
    if (!opening || this.sheet.busy) return;
    const s = this.words.subagents;
    const form = this.sheet.body;
    const value = (name: string) => form.querySelector<HTMLInputElement | HTMLTextAreaElement>(`[name="${name}"]`)?.value ?? "";
    const name = value("name").trim();
    if (!SUBAGENT_NAME.test(name)) return this.wrong("name", s.nameRule);
    if (opening.taken.includes(name)) return this.wrong("name", s.taken);
    const description = value("description").trim();
    if (!description) return this.wrong("description", s.descriptionNeeded);
    const turns = value("maxTurns").trim();
    if (turns && !/^[1-9]\d*$/.test(turns)) return this.wrong("maxTurns", s.maxTurnsRule);
    const agent: SubagentSpec = { ...(opening.agent ?? {}), name, description, prompt: value("prompt") };
    const set = (key: string, kept: unknown) => {
      if (kept === undefined) delete agent[key];
      else agent[key] = kept;
    };
    const tools = (text: string) => {
      const listed = text.split(",").map((tool) => tool.trim()).filter(Boolean);
      return listed.length ? listed : undefined;
    };
    set("model", this.chosen.model ?? undefined);
    set("effort", this.chosen.effort ?? undefined);
    set("tools", tools(value("tools")));
    set("disallowedTools", tools(value("disallowed")));
    set("maxTurns", turns ? Number(turns) : undefined);
    set("background", this.chosen.background ? true : undefined);
    set("omitClaudeMd", this.chosen.claudeMd ? undefined : true);
    this.sheet.busy = true;
    const go = form.querySelector<HTMLButtonElement>(".sheet-go");
    if (go) go.disabled = true;
    try {
      await opening.save(agent, opening.agent?.name ?? null);
      this.sheet.busy = false;
      this.sheet.close();
    } catch (error) {
      this.sheet.busy = false;
      const detail = detailOf(error);
      if (go) go.disabled = false;
      if (/already exists/.test(detail)) {
        this.wrong("name", s.taken);
        return;
      }
      this.problem = { text: /the profile changed/.test(detail) ? this.words.changedElsewhere : this.words.saveFailed, detail };
      const note = form.querySelector<HTMLElement>(".sheet-note");
      if (note) {
        note.innerHTML = `${escape(this.problem.text)}<span class="sheet-note-detail">${escape(detail)}</span>`;
        note.hidden = false;
      }
    }
  }
}
