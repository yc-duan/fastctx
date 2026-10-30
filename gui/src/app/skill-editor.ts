// One skill, its folder opened (`world-user-config.md`, 技能): one card on
// the page, its files down the left in the folder's order with SKILL.md
// first, the one chosen washed; beside them that file, written in place.
// SKILL.md is the skill's name, when it applies and what to do; any other
// text file is written as it is, in the code face; a file whose bytes are
// not text is said, not shown. A new file is named where the list ends. On
// a phone the list folds into one line naming the file, its menu rising
// from below. Saving writes the folder whole, against the version it was
// read at.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { bytesSaid } from "../bytes";
import { SKILL_FILE, SKILL_NAME, SKILL_WHEN_MOST, fileSize, fileText, readSkillText, skillText, textFile, type SkillFile } from "../world/skills";
import { ActsMenu } from "./acts-menu";
import { detailOf } from "./problem";
import type { ProfileWords } from "./profile-words";

/** A skill as it is opened: one kept, at its version, or one not yet kept,
 *  new or taken in from a folder. */
export interface SkillOpened {
  /** Its name as kept; none for one not yet kept. */
  kept: string | null;
  files: SkillFile[];
  /** A name to start with where its SKILL.md says none. */
  named?: string;
}

export interface SkillEditorSeams {
  /** The names the account's other skills have. */
  taken: string[];
  /** Saves it as `name`; throws what went wrong. */
  save(name: string, files: SkillFile[]): Promise<void>;
  /** Asks to delete it; only for one kept. */
  remove(): void;
  /** Goes back to the list. */
  leave(): void;
}

/** One file as it is being written: its text, or its bytes where they are
 *  not text. */
interface Edited {
  path: string;
  text: string | null;
  data: string | null;
  executable: boolean;
}

/** Characters no portable path holds (`fastctx_types::skill_files`). */
const UNPORTABLE = /[\\:*?"<>|\u0000-\u001f]/;

const narrow = matchMedia("(max-width: 760px)");
let shared: ActsMenu | null = null;
const menuOf = (): ActsMenu => (shared ??= new ActsMenu());

export class SkillEditor {
  readonly element = document.createElement("div");
  private files: Edited[] = [];
  private chosen = SKILL_FILE;
  private name = "";
  private description = "";
  private instructions = "";
  private opened: SkillOpened | null = null;
  private seams: SkillEditorSeams | null = null;
  /** What it was as opened, to know whether it changed. */
  private was = "";
  private adding = false;
  private busy = false;

  constructor(private readonly words: ProfileWords) {
    this.element.className = "ske";
    this.element.addEventListener("click", (event) => this.click(event));
    this.element.addEventListener("input", (event) => this.input(event));
    this.element.addEventListener("keydown", (event) => this.key(event));
  }

  open(opened: SkillOpened, seams: SkillEditorSeams): void {
    this.opened = opened;
    this.seams = seams;
    this.files = opened.files
      .filter((file) => file.path !== SKILL_FILE)
      .map((file) => ({ path: file.path, text: fileText(file), data: file.data, executable: !!file.executable }))
      .sort((a, b) => a.path.localeCompare(b.path));
    const skill = opened.files.find((file) => file.path === SKILL_FILE);
    const read = readSkillText(skill ? (fileText(skill) ?? "") : "");
    this.name = read.name ?? opened.named ?? "";
    this.description = read.description ?? "";
    this.instructions = read.instructions;
    this.chosen = SKILL_FILE;
    this.adding = false;
    this.busy = false;
    this.was = this.state();
    this.draw();
  }

  /** Whether anything was changed since it was opened. */
  changed(): boolean {
    return this.opened !== null && this.state() !== this.was;
  }

  private state(): string {
    return JSON.stringify([this.name, this.description, this.instructions, this.files.map((file) => [file.path, file.text ?? file.data])]);
  }

  private draw(): void {
    const s = this.words.skills;
    const kept = this.opened?.kept ?? null;
    this.element.innerHTML = `<button class="text-button quiet-button ske-back" type="button" data-ske="back">${glyph("back", 15)}<span>${escape(s.back)}</span></button>
      <div class="ske-card">
        <nav class="ske-files" aria-label="${escape(s.files)}"></nav>
        <div class="ske-rule" aria-hidden="true"></div>
        <div class="ske-file"></div>
      </div>
      <footer class="ske-foot"><p class="ske-note" role="alert" hidden></p>${kept ? `<button class="text-button danger-text" type="button" data-ske="remove">${escape(s.remove)}</button>` : ""}<button class="sheet-go" type="button" data-ske="save">${escape(s.save)}</button></footer>`;
    this.drawFiles();
    this.drawFile();
  }

  /** The folder's files, SKILL.md first, each folder named once above
   *  what is in it; on a phone, one line naming the file chosen. */
  private drawFiles(): void {
    const s = this.words.skills;
    const list = this.element.querySelector<HTMLElement>(".ske-files")!;
    const row = (path: string, depth: number) => {
      const leaf = path.split("/").pop()!;
      const removable = path !== SKILL_FILE;
      return `<li class="ske-row${path === this.chosen ? " chosen" : ""}" style="--depth: ${depth}"><button class="ske-pick" type="button" data-ske-file="${escape(path)}" aria-current="${path === this.chosen}">${glyph(path === SKILL_FILE ? "skill" : "file", 15)}<span class="ske-leaf" title="${escape(path)}">${escape(leaf)}</span></button>${
        removable ? `<button class="icon-button ske-drop" type="button" data-ske-drop="${escape(path)}" title="${escape(s.removeFile)}" aria-label="${escape(`${s.removeFile} ${path}`)}">${glyph("close", 14)}</button>` : ""
      }</li>`;
    };
    const rows = [row(SKILL_FILE, 0)];
    const named = new Set<string>();
    for (const file of this.files) {
      const parts = file.path.split("/");
      for (let depth = 1; depth < parts.length; depth += 1) {
        const folder = parts.slice(0, depth).join("/");
        if (named.has(folder)) continue;
        named.add(folder);
        rows.push(`<li class="ske-folder" style="--depth: ${depth - 1}">${glyph("folder", 15)}<span>${escape(parts[depth - 1]!)}</span></li>`);
      }
      if (file.data !== null) rows.push(row(file.path, parts.length - 1));
    }
    const adding = this.adding
      ? `<li class="ske-adding"><input class="ske-path" type="text" spellcheck="false" autocomplete="off" aria-label="${escape(s.path)}" placeholder="scripts/notes.md"></li>`
      : `<li><button class="text-button quiet-button ske-add" type="button" data-ske="add">${glyph("plus", 15)}<span>${escape(s.addFile)}</span></button></li>`;
    list.innerHTML = `<button class="ske-which" type="button" data-ske="which" data-menu-key="ske-which" aria-haspopup="menu" aria-expanded="false">${glyph(this.chosen === SKILL_FILE ? "skill" : "file", 15)}<span class="ske-leaf">${escape(this.chosen)}</span>${glyph("chevron", 12)}</button><ul class="ske-list">${rows.join("")}${adding}</ul>`;
    if (this.adding) list.querySelector<HTMLInputElement>(".ske-path")?.focus();
  }

  /** The file chosen, written in place. */
  private drawFile(): void {
    const s = this.words.skills;
    const place = this.element.querySelector<HTMLElement>(".ske-file")!;
    if (this.chosen === SKILL_FILE) {
      place.className = "ske-file ske-skill";
      place.innerHTML = `<label class="field ske-field"><span class="field-label">${escape(s.name)}</span><input name="name" type="text" spellcheck="false" autocomplete="off" maxlength="64" value="${escape(this.name)}"><span class="field-hint" data-ske-hint="name" hidden></span></label>
        <label class="field ske-field"><span class="field-label">${escape(s.when)}</span><input name="description" type="text" autocomplete="off" maxlength="${SKILL_WHEN_MOST}" value="${escape(this.description)}"><span class="field-hint" data-ske-hint="description" hidden></span></label>
        <label class="field ske-field ske-grow"><span class="field-label">${escape(s.instructions)}</span><textarea class="ske-text ske-prose" name="instructions" spellcheck="false">${escape(this.instructions)}</textarea><span class="field-hint" data-ske-hint="instructions" hidden></span></label>`;
      return;
    }
    const file = this.files.find((one) => one.path === this.chosen);
    if (!file) return;
    if (file.text === null) {
      place.className = "ske-file ske-binary";
      place.innerHTML = `${glyph("file", 28)}<p>${escape(s.binary(bytesSaid(fileSize(file))))}</p>`;
      return;
    }
    place.className = "ske-file ske-code";
    place.innerHTML = `<textarea class="ske-text" name="file" spellcheck="false" aria-label="${escape(file.path)}">${escape(file.text)}</textarea>`;
  }

  private click(event: Event): void {
    const target = event.target as HTMLElement;
    const drop = target.closest<HTMLElement>("[data-ske-drop]")?.dataset.skeDrop;
    if (drop !== undefined) return this.drop(drop);
    const path = target.closest<HTMLElement>("[data-ske-file]")?.dataset.skeFile;
    if (path !== undefined) return this.choose(path);
    const act = target.closest<HTMLElement>("[data-ske]");
    switch (act?.dataset.ske) {
      case "back":
        return this.seams?.leave();
      case "remove":
        return this.seams?.remove();
      case "save":
        return void this.save();
      case "add":
        this.adding = true;
        return this.drawFiles();
      case "which":
        return this.which(act);
    }
  }

  private input(event: Event): void {
    const field = event.target as HTMLInputElement | HTMLTextAreaElement;
    field.closest(".field")?.querySelector<HTMLElement>(".field-hint")?.setAttribute("hidden", "");
    switch (field.name) {
      case "name":
        this.name = field.value;
        break;
      case "description":
        this.description = field.value;
        break;
      case "instructions":
        this.instructions = field.value;
        break;
      case "file": {
        const file = this.files.find((one) => one.path === this.chosen);
        if (file) file.text = field.value;
        break;
      }
    }
  }

  private key(event: KeyboardEvent): void {
    const field = event.target as HTMLElement;
    if (!field.classList.contains("ske-path")) return;
    if (event.key === "Escape") {
      event.preventDefault();
      this.adding = false;
      this.drawFiles();
      return;
    }
    if (event.key !== "Enter") return;
    event.preventDefault();
    const s = this.words.skills;
    const path = (field as HTMLInputElement).value.trim().replace(/^\/+|\/+$/g, "");
    if (!path) {
      this.adding = false;
      return this.drawFiles();
    }
    if (UNPORTABLE.test(path) || path.split("/").some((part) => !part || part === "." || part === "..")) return this.say(s.pathRule, "");
    if (path === SKILL_FILE || this.files.some((file) => file.path === path || file.path.startsWith(`${path}/`) || path.startsWith(`${file.path}/`))) return this.say(s.pathTaken, "");
    const made = textFile(path, "");
    this.files.push({ path, text: "", data: made.data, executable: false });
    this.files.sort((a, b) => a.path.localeCompare(b.path));
    this.adding = false;
    this.note(null);
    this.choose(path);
  }

  private choose(path: string): void {
    this.chosen = path;
    this.drawFiles();
    this.drawFile();
    this.element.querySelector<HTMLElement>(".ske-file textarea, .ske-file input")?.focus({ preventScroll: true });
  }

  private drop(path: string): void {
    this.files = this.files.filter((file) => file.path !== path);
    if (this.chosen === path) this.chosen = SKILL_FILE;
    this.drawFiles();
    this.drawFile();
  }

  /** A phone's menu of the files, rising from below. */
  private which(anchor: HTMLElement): void {
    const s = this.words.skills;
    const entries = [SKILL_FILE, ...this.files.filter((file) => file.data !== null).map((file) => file.path)].map((path) => ({ act: `file:${path}`, label: path, checked: path === this.chosen }));
    const drop = this.chosen === SKILL_FILE ? [] : [{ act: "drop", icon: "trash" as const, label: s.removeFile, danger: true }];
    menuOf().open(anchor, s.files, [...entries, { rule: true as const }, { act: "add", icon: "plus" as const, label: s.addFile }, ...drop], (act) => {
      if (act === "add") {
        this.adding = true;
        this.drawFiles();
      } else if (act === "drop") this.drop(this.chosen);
      else this.choose(act.slice("file:".length));
    });
  }

  private async save(): Promise<void> {
    const seams = this.seams;
    if (!seams || this.busy) return;
    const s = this.words.skills;
    const name = this.name.trim();
    const description = this.description.replace(/\s+/g, " ").trim();
    const wrong = (field: string, text: string) => {
      if (this.chosen !== SKILL_FILE) this.choose(SKILL_FILE);
      const hint = this.element.querySelector<HTMLElement>(`[data-ske-hint="${field}"]`);
      if (!hint) return;
      hint.textContent = text;
      hint.hidden = false;
      hint.closest(".field")?.querySelector<HTMLElement>("input, textarea")?.focus();
    };
    if (!SKILL_NAME.test(name)) return wrong("name", s.nameRule);
    if (seams.taken.includes(name)) return wrong("name", s.taken);
    if (!description) return wrong("description", s.whenNeeded);
    if (!this.instructions.trim()) return wrong("instructions", s.instructionsNeeded);
    const files: SkillFile[] = [
      textFile(SKILL_FILE, skillText(name, description, this.instructions)),
      ...this.files.map((file) => ({ ...(file.text === null ? { path: file.path, data: file.data } : textFile(file.path, file.text)), ...(file.executable ? { executable: true } : {}) })),
    ];
    this.busy = true;
    const go = this.element.querySelector<HTMLButtonElement>("[data-ske=save]");
    if (go) go.disabled = true;
    this.note(null);
    try {
      await seams.save(name, files);
      this.was = this.state();
    } catch (error) {
      const detail = detailOf(error);
      if (/named .* already/.test(detail)) wrong("name", s.taken);
      else this.say(/changed meanwhile/.test(detail) ? this.words.changedElsewhere : this.words.saveFailed, detail);
    } finally {
      this.busy = false;
      if (go) go.disabled = false;
    }
  }

  /** Says what went wrong beside the buttons, its own words on hover. */
  private say(text: string, detail: string): void {
    this.note({ text, detail });
  }

  private note(said: { text: string; detail: string } | null): void {
    const note = this.element.querySelector<HTMLElement>(".ske-note");
    if (!note) return;
    note.hidden = !said;
    note.textContent = said?.text ?? "";
    if (said?.detail) note.title = said.detail;
    else note.removeAttribute("title");
  }

  /** Whether a phone's width folds the list. */
  static folded(): boolean {
    return narrow.matches;
  }
}
