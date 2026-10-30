// One of the account's skills, written or rewritten (`world-user-config.md`,
// 技能): its name, when it applies in a line, and its instructions; or read
// in from a SKILL.md as hosts write them. A rule broken is said beside its
// field; what the World refuses, under the fields, its own words a touch
// away.

import { escape } from "../conversation/prose";
import { SKILL_INSTRUCTIONS_MOST, SKILL_NAME, SKILL_WHEN_MOST, skillFromFile, type Skill } from "../world/skills";
import { Sheet } from "./confirm";
import { detailOf } from "./problem";
import type { ProfileWords } from "./profile-words";

export interface SkillOpening {
  /** The one rewritten; none for a new one. */
  skill: Skill | null;
  /** The names the account's others have. */
  taken: string[];
  /** Saves it over the one it was, or as a new one. */
  save(skill: Skill, previous: string | null): Promise<void>;
  /** Asks to delete the one rewritten. */
  remove(): void;
}

export class SkillSheet {
  private readonly sheet = new Sheet("skl-sheet");
  private opening: SkillOpening | null = null;
  private readonly file: HTMLInputElement;

  constructor(
    private readonly words: ProfileWords,
    private readonly close: string,
  ) {
    const body = this.sheet.body;
    this.file = document.createElement("input");
    this.file.type = "file";
    this.file.accept = ".md,.markdown,.txt,text/markdown,text/plain";
    this.file.hidden = true;
    this.file.addEventListener("change", () => void this.read());
    body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target.closest("[data-skl-remove]")) {
        this.sheet.close();
        this.opening?.remove();
      }
      if (target.closest("[data-skl-file]")) this.file.click();
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

  open(opening: SkillOpening): void {
    this.opening = opening;
    this.draw();
    this.sheet.show();
    this.sheet.body.querySelector<HTMLElement>(opening.skill ? "textarea[name=instructions]" : "input[name=name]")?.focus();
  }

  private draw(): void {
    const opening = this.opening;
    if (!opening) return;
    const s = this.words.skills;
    const skill = opening.skill;
    const field = (name: string, label: string, control: string) =>
      `<label class="field"><span class="field-label">${escape(label)}</span>${control}<span class="field-hint" data-skl-hint="${name}" hidden></span></label>`;
    this.sheet.body.innerHTML = `${this.sheet.head(skill ? s.editTitle(skill.name) : s.newTitle, this.close)}
      ${field("name", s.name, `<input name="name" type="text" spellcheck="false" autocomplete="off" maxlength="64" value="${escape(skill?.name ?? "")}">`)}
      ${field("description", s.when, `<input name="description" type="text" autocomplete="off" maxlength="${SKILL_WHEN_MOST}" value="${escape(skill?.description ?? "")}">`)}
      ${field("instructions", s.instructions, `<textarea class="skl-instructions" name="instructions" rows="10" spellcheck="false">${escape(skill?.instructions ?? "")}</textarea>`)}
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot">${
        skill
          ? `<button class="text-button danger-text" type="button" data-skl-remove>${escape(s.remove)}</button>`
          : `<button class="text-button" type="button" data-skl-file>${escape(s.fromFile)}</button>`
      }<button class="sheet-go" type="submit">${escape(s.save)}</button></footer>`;
    this.sheet.body.append(this.file);
    this.sheet.anew();
  }

  /** Fills the fields from the file chosen, keeping a name already typed
   *  where the file names none. */
  private async read(): Promise<void> {
    const chosen = this.file.files?.[0];
    this.file.value = "";
    if (!chosen) return;
    const form = this.sheet.body;
    try {
      const read = skillFromFile(await chosen.text(), chosen.name);
      const set = (name: string, value: string | undefined) => {
        const control = form.querySelector<HTMLInputElement | HTMLTextAreaElement>(`[name="${name}"]`);
        if (control && value !== undefined) control.value = value;
      };
      set("name", read.name && SKILL_NAME.test(read.name) ? read.name : undefined);
      set("description", read.description?.replace(/\s+/g, " ").slice(0, SKILL_WHEN_MOST));
      set("instructions", read.instructions);
    } catch {
      this.say(this.words.skills.fileUnread, "");
    }
  }

  /** Says `text` beside the field `name`, and goes to it. */
  private wrong(name: string, text: string): void {
    const hint = this.sheet.body.querySelector<HTMLElement>(`[data-skl-hint="${name}"]`);
    if (!hint) return;
    hint.textContent = text;
    hint.hidden = false;
    hint.closest(".field")?.querySelector<HTMLElement>("input, textarea")?.focus();
  }

  private say(text: string, detail: string): void {
    const note = this.sheet.body.querySelector<HTMLElement>(".sheet-note");
    if (!note) return;
    note.innerHTML = `${escape(text)}${detail ? `<span class="sheet-note-detail">${escape(detail)}</span>` : ""}`;
    note.hidden = false;
  }

  private async submit(): Promise<void> {
    const opening = this.opening;
    if (!opening || this.sheet.busy) return;
    const s = this.words.skills;
    const form = this.sheet.body;
    const value = (name: string) => form.querySelector<HTMLInputElement | HTMLTextAreaElement>(`[name="${name}"]`)?.value ?? "";
    const name = value("name").trim();
    if (!SKILL_NAME.test(name)) return this.wrong("name", s.nameRule);
    if (opening.taken.includes(name)) return this.wrong("name", s.taken);
    const description = value("description").replace(/\s+/g, " ").trim();
    if (!description) return this.wrong("description", s.whenNeeded);
    const instructions = value("instructions");
    if (!instructions.trim()) return this.wrong("instructions", s.instructionsNeeded);
    if (new TextEncoder().encode(instructions).length > SKILL_INSTRUCTIONS_MOST) return this.wrong("instructions", this.words.prompt.tooLong);
    this.sheet.busy = true;
    const go = form.querySelector<HTMLButtonElement>(".sheet-go");
    if (go) go.disabled = true;
    try {
      await opening.save({ name, description, instructions }, opening.skill?.name ?? null);
      this.sheet.busy = false;
      this.sheet.close();
    } catch (error) {
      this.sheet.busy = false;
      if (go) go.disabled = false;
      const detail = detailOf(error);
      if (/named .* already/.test(detail)) return this.wrong("name", s.taken);
      this.say(/changed meanwhile/.test(detail) ? this.words.changedElsewhere : this.words.saveFailed, detail);
    }
  }
}
