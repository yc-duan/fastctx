// A conversation's plan as a page of the panel beside it (`world-plan.md`):
// its version and when it was written at the head, the plan below. What is
// drawn is not words to edit, so the user edits the plan's own text: "Edit"
// lays it out on the same sheet, in the same type, and saving puts it up as
// the next version.

import { escape } from "../conversation/prose";
import type { SidePage } from "../side/pane";
import { noteElement } from "./blocks";
import { drawPlan, planElement, type PlanSeams } from "./render";
import type { PlanWords } from "./words";
import "./plan.css";

export interface PlanShown {
  source: string | null;
  version: number;
  /** When it was last written, in the interface's words. */
  when: string;
}

/** How a save ended: up as a new version, or met a newer one written
 *  elsewhere, whose version comes back so the next save can begin from it. */
export type PlanSaved = { ok: true } | { ok: false; current: number | null };

export interface PageSeams extends Omit<PlanSeams, "words"> {
  /** The user saved their edit of the plan's text. `expected` is the version
   *  the edit began at; a newer version in the meantime is a conflict. */
  save: (source: string, expected: number) => Promise<PlanSaved>;
}

export class PlanPage implements SidePage {
  readonly element: HTMLElement;
  private readonly sheet: HTMLElement;
  private readonly meta: HTMLElement;
  private readonly acts: HTMLElement;
  private plan: HTMLElement | null = null;
  private shownPlan: PlanShown | null = null;
  private editing: HTMLTextAreaElement | null = null;
  /** The version the edit under way began at, for the save's comparison. */
  private editingVersion = 0;
  private width = 0;

  constructor(
    private readonly words: PlanWords,
    private readonly seams: PageSeams,
  ) {
    this.element = document.createElement("section");
    this.element.className = "pb-page scroller";
    this.element.innerHTML = `<header class="pb-head"><span class="pb-meta"></span><span class="pb-acts"></span></header><div class="pb-sheet"></div>`;
    this.sheet = this.element.querySelector(".pb-sheet")!;
    this.meta = this.element.querySelector(".pb-meta")!;
    this.acts = this.element.querySelector(".pb-acts")!;
    this.acts.addEventListener("click", (event) => {
      const act = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]")?.dataset.act;
      if (act === "edit") this.edit();
      else if (act === "save") this.save();
      else if (act === "cancel") this.drawn();
    });
    new ResizeObserver(() => this.redraw()).observe(this.element);
  }

  show(shown: PlanShown): void {
    // An edit under way is not swept away by a new version.
    this.shownPlan = shown;
    if (this.editing) return;
    this.drawn();
  }

  shown(): void {
    this.width = 0;
    this.redraw();
  }

  /** The plan as it is drawn. */
  private drawn(): void {
    const shown = this.shownPlan;
    this.editing = null;
    this.sheet.innerHTML = "";
    this.meta.innerHTML =
      shown?.source == null ? "" : `<span>${escape(this.words.version(shown.version))}</span><span>${escape(shown.when)}</span>`;
    this.acts.innerHTML = shown?.source == null ? "" : this.act("edit", this.words.edit);
    if (shown?.source == null) {
      this.plan = null;
      const empty = document.createElement("p");
      empty.className = "pb-empty";
      empty.textContent = this.words.empty;
      this.sheet.append(empty);
      return;
    }
    this.plan = planElement(shown.source, { ...this.seams, words: this.words });
    this.sheet.append(this.plan);
    this.width = 0;
    this.redraw();
  }

  /** The plan's own text on the same sheet, to change and save. */
  private edit(): void {
    if (this.shownPlan?.source == null) return;
    this.editingVersion = this.shownPlan.version;
    this.plan = null;
    this.sheet.innerHTML = "";
    const text = document.createElement("textarea");
    text.className = "pb-source";
    text.spellcheck = false;
    text.value = this.shownPlan.source;
    text.addEventListener("input", () => this.fit(text));
    text.addEventListener("keydown", (event) => {
      if (event.key === "Escape") this.drawn();
      else if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) this.save();
    });
    this.sheet.append(text);
    this.editing = text;
    this.acts.innerHTML = `${this.act("cancel", this.words.cancel)}${this.act("save", this.words.save, true)}`;
    this.fit(text);
    text.focus();
  }

  private save(): void {
    const text = this.editing;
    if (!text || !this.shownPlan) return;
    const source = text.value;
    const expected = this.editingVersion;
    if (source === this.shownPlan.source) {
      this.drawn();
      return;
    }
    // The edit stays until the World keeps it; a save that did not go
    // through keeps it to save again.
    text.readOnly = true;
    void this.seams.save(source, expected).then(
      (saved) => {
        if (this.editing !== text) return;
        if (!saved.ok) return this.conflicted(source, saved.current);
        if (this.shownPlan) this.shownPlan = { ...this.shownPlan, source };
        this.drawn();
      },
      (error: unknown) => {
        if (this.editing !== text) return;
        text.readOnly = false;
        this.sheet.querySelector(".plan-unsaved")?.remove();
        const note = noteElement("warn", `<p>${escape(this.words.unsaved)}</p>`, "plan-unsaved");
        note.classList.add("plan-unsaved");
        note.title = error instanceof Error ? error.message : String(error);
        this.sheet.prepend(note);
      },
    );
  }

  /** A save met a newer version. The draft is kept and laid out to edit
   *  again, from that newer version, so saving once more keeps the user's. */
  private conflicted(source: string, current: number | null): void {
    if (current != null && this.shownPlan) this.shownPlan = { ...this.shownPlan, version: current };
    this.edit();
    if (this.editing) this.editing.value = source;
    if (current != null) this.editingVersion = current;
    this.sheet.prepend(noteElement("warn", `<p>${escape(this.words.conflict)}</p>`, "plan-conflict"));
  }

  /** The text area as tall as its text, so the page, not it, scrolls. */
  private fit(text: HTMLTextAreaElement): void {
    text.style.height = "auto";
    text.style.height = `${text.scrollHeight}px`;
  }

  private act(name: string, label: string, main = false): string {
    return `<button class="pb-act${main ? " pb-act-main" : ""}" type="button" data-act="${name}">${escape(label)}</button>`;
  }

  /** Draws again when the page's width changed. */
  private redraw(): void {
    if (this.editing) this.fit(this.editing);
    const width = this.sheet.clientWidth;
    if (!this.plan || width === this.width || width === 0) return;
    this.width = width;
    drawPlan(this.plan);
  }
}
