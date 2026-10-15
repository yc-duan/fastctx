// Finding words in a conversation, as a browser's find does: a bar under
// the head, every match washed, the current one stronger, Enter and
// Shift+Enter between them. Matches are marked with the CSS Custom
// Highlight API, so the conversation's own elements are never touched and
// a stream of words keeps flowing while it is open.

import { layerOpened } from "../app/back";
import { glyph } from "./glyphs";
import { escape } from "./prose";
import { FindIndex, type FarHits, type FarMatch, type FindChanges, type FindMatch } from "./find-index";
import { FindHighlights } from "./find-highlights";

interface FindView {
  findChanges(): FindChanges;
  revealFind(match: FindMatch): Promise<void>;
  findRange(match: FindMatch): Range | null;
  /** What the page does not hold (`find-far.ts`): counted by the shell,
   *  read in where a match in it is gone to. */
  far?: {
    search(query: string): Promise<FarHits[]>;
    /** Whether more may be found, as more is indexed. */
    readonly more: boolean;
    /** Moves as more is indexed. */
    readonly grown: number;
    /** What the page holds, as one word: what is counted far changes with it. */
    held(): string;
    /** Reads in the turns around event `at`, and draws them. */
    reach(at: number): Promise<void>;
    close(): void;
  };
}

export interface FindWords {
  find: string;
  placeholder: string;
  count(current: number, total: number): string;
  none: string;
  previous: string;
  next: string;
  close: string;
}

export class Finder {
  readonly element: HTMLElement;
  private readonly input: HTMLInputElement;
  private readonly count: HTMLElement;
  private readonly index = new FindIndex();
  private selected: FindMatch | FarMatch | null = null;
  /** What the shell was last asked for: the words, what the page held,
   *  how much was indexed. */
  private farAsked = "";
  private current = -1;
  private fresh = false;
  private pending = false;
  private running = false;
  private disposed = false;
  private revision = 0;
  private readonly highlights: FindHighlights;
  private task: Promise<void> = Promise.resolve();

  /** Resolves once queued changes and the requested reveal have settled. */
  get ready(): Promise<void> {
    return (async () => {
      for (;;) {
        const task = this.task;
        await task;
        if (task === this.task) return;
      }
    })();
  }

  constructor(
    transcript: HTMLElement,
    private readonly scroller: HTMLElement,
    private readonly words: FindWords,
    /** The reader moved through the conversation by finding. */
    private readonly moved: () => void,
    private readonly view: FindView,
  ) {
    this.element = document.createElement("div");
    this.element.className = "find-bar";
    this.element.hidden = true;
    this.element.setAttribute("role", "search");
    // It floats over the page's caption, which drags the window on Windows.
    this.element.dataset.worldTitlebar = "client";
    this.element.innerHTML = `<span class="find-glyph">${glyph("search", 16)}</span><input class="find-input" type="search" enterkeyhint="search" placeholder="${escape(words.placeholder)}" aria-label="${escape(words.find)}"><span class="find-count" aria-live="polite"></span><button class="icon-button find-step" type="button" data-step="-1" aria-label="${escape(words.previous)}" title="${escape(words.previous)}">${glyph("chevron", 16)}</button><button class="icon-button find-step find-next" type="button" data-step="1" aria-label="${escape(words.next)}" title="${escape(words.next)}">${glyph("chevron", 16)}</button><button class="icon-button find-close" type="button" aria-label="${escape(words.close)}" title="${escape(words.close)}">${glyph("close", 16)}</button>`;
    this.input = this.element.querySelector<HTMLInputElement>(".find-input")!;
    this.count = this.element.querySelector<HTMLElement>(".find-count")!;
    this.input.addEventListener("input", () => this.search(true));
    this.input.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.isComposing) {
        event.preventDefault();
        void this.step(event.shiftKey ? -1 : 1);
      }
    });
    this.element.addEventListener("keydown", (event) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      this.close();
    });
    this.element.addEventListener("click", (event) => {
      const target = (event.target as HTMLElement).closest<HTMLElement>("[data-step], .find-close");
      if (!target) return;
      if (target.classList.contains("find-close")) this.close();
      else void this.step(Number(target.dataset.step));
    });
    this.highlights = new FindHighlights(transcript, () => {
      if (!this.open) return;
      this.refresh();
      this.paint();
    });
  }

  /** What had focus before the bar opened. */
  private before: HTMLElement | null = null;

  get open(): boolean {
    return !this.element.hidden;
  }

  show(): void {
    if (this.element.hidden && document.activeElement instanceof HTMLElement) this.before = document.activeElement;
    if (this.element.hidden) layerOpened({ isOpen: () => !this.element.hidden && this.element.isConnected, close: () => this.close() });
    this.element.hidden = false;
    // What is selected is what is looked for, as browsers do.
    const selected = window.getSelection()?.toString().trim();
    if (selected && !selected.includes("\n")) this.input.value = selected;
    this.input.focus();
    this.input.select();
    this.search(true);
  }

  close(): void {
    const inside = this.element.contains(document.activeElement);
    this.element.hidden = true;
    this.view.far?.close();
    this.farAsked = "";
    if (inside && this.before?.isConnected) this.before.focus({ preventScroll: true });
    this.before = null;
    this.revision += 1;
    this.selected = null;
    this.current = -1;
    this.paint();
  }

  /** New or changed display text is searched; historical text and the
   * selected match keep their identities across drawing and streaming. */
  refresh(): void {
    if (this.open && this.input.value) this.search(false);
  }

  private search(fresh: boolean): void {
    this.fresh ||= fresh;
    if (fresh) this.revision += 1;
    if (!this.input.value) {
      this.index.search("");
      this.selected = null;
      this.current = -1;
      this.paint();
      return;
    }
    this.pending = true;
    if (!this.running) this.task = this.synchronize();
  }

  private async synchronize(): Promise<void> {
    this.running = true;
    try {
      while (this.pending && this.open && this.input.value) {
        this.pending = false;
        await this.index.update(this.view.findChanges());
        this.index.search(this.input.value.toLowerCase());
        const far = this.view.far;
        const asked = far ? `${this.input.value}\u0001${far.held()}\u0001${far.grown}` : "";
        if (far && asked !== this.farAsked) {
          this.farAsked = asked;
          this.index.setFar(await far.search(this.input.value));
        }
        const same = this.selected ? this.index.position(this.selected) : -1;
        this.current = this.fresh || !this.selected ? this.index.total - 1
          : same >= 0 ? same : Math.min(this.current, this.index.total - 1);
        this.selected = this.index.at(this.current);
      }
      const reveal = this.fresh;
      this.fresh = false;
      this.paint();
      if (reveal && this.open) await this.reveal();
    } catch (error) {
      if (!this.disposed || !(error instanceof DOMException) || error.name !== "AbortError") throw error;
    } finally {
      this.running = false;
      if (this.pending && this.open && this.input.value) this.task = this.synchronize();
    }
  }

  private async step(by: number): Promise<void> {
    await this.ready;
    if (!this.open || !this.index.total) return;
    this.revision += 1;
    this.current = (this.current + by + this.index.total) % this.index.total;
    this.selected = this.index.at(this.current);
    this.paint();
    await this.reveal();
  }

  /** A match in what the page does not hold, once its turns are read in:
   *  the same match, now where the page holds it. */
  private async resolve(match: FarMatch): Promise<FindMatch | null> {
    const far = this.view.far;
    if (!far) return null;
    await far.reach(match.far.at);
    await this.index.update(this.view.findChanges());
    this.index.search(this.input.value.toLowerCase());
    this.farAsked = "";
    return this.index.match(match.far.block, match.far.part, match.far.ordinal);
  }

  private paint(): void {
    const selected = this.open ? this.selected : null;
    const identity = !selected ? null : "far" in selected ? `far|${selected.far.part}|${selected.far.ordinal}` : `${selected.document.key}|${selected.document.stamp}|${selected.start}|${selected.end}`;
    this.highlights.paint(this.open ? this.input.value.toLowerCase() : "", identity, () => (selected && !("far" in selected) ? this.view.findRange(selected) : null));
    const query = this.input.value;
    const more = this.view.far?.more ? "+" : "";
    const said = !query ? "" : this.index.total ? `${this.words.count(this.current + 1, this.index.total)}${more}` : more ? "…" : this.words.none;
    if (this.count.textContent !== said) this.count.textContent = said;
    this.count.classList.toggle("none", !!query && !this.index.total);
  }

  /** Brings the current match into view, opening what folds it away; one
   *  where the page holds nothing is read in first. */
  private async reveal(): Promise<void> {
    let match = this.selected;
    if (!match) return;
    const revision = this.revision;
    this.moved();
    if ("far" in match) {
      const found = await this.resolve(match);
      if (revision !== this.revision || !this.open || !found) return;
      this.selected = match = found;
      const position = this.index.position(found);
      if (position >= 0) this.current = position;
      this.paint();
    }
    await this.view.revealFind(match);
    if (revision !== this.revision || !this.open) return;
    this.paint();
    const range = this.view.findRange(match);
    if (!range) return;
    const box = range.getBoundingClientRect();
    const view = this.scroller.getBoundingClientRect();
    if (box.top < view.top + 48 || box.bottom > view.bottom - 48) {
      this.scroller.scrollTop += box.top - view.top - view.height / 3;
    }
    // A command can be wider than its line box on a phone.
    for (let node = range.startContainer.parentElement; node && node !== this.scroller; node = node.parentElement) {
      if (node.scrollWidth <= node.clientWidth || !/^(auto|scroll|hidden)$/.test(getComputedStyle(node).overflowX)) continue;
      const area = node.getBoundingClientRect();
      const match = range.getBoundingClientRect();
      if (match.left < area.left || match.right > area.right) node.scrollLeft += match.left - area.left;
    }
  }

  destroy(): void {
    this.disposed = true;
    this.close();
    this.index.destroy();
    this.highlights.destroy();
  }
}
