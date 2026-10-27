// Finding machines on the machines page, as the conversation's find does
// (`conversation/find.ts`): a bar under the head, opened from its glass or
// with Ctrl+F. What is typed is matched against each machine's name and the
// names it had, whose it is and the group it is in; the list then holds the
// matches and what they stand in, and the drawing marks them, Enter and
// Shift+Enter going from one to the next.

import { layerOpened } from "./back";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { composing } from "../typing";
import type { MapNode } from "./map-nodes";

export interface MachineFindWords {
  find: string;
  placeholder: string;
  /** How many match, before one of them is gone to. */
  found(total: number): string;
  count(current: number, total: number): string;
  none: string;
  previous: string;
  next: string;
  close: string;
}

export interface MachineFindHost {
  /** What is looked for changed: the page draws itself with what matches. */
  changed(): void;
  /** Goes to the match `key`. */
  reach(key: string): void;
}

export class MachineFind {
  readonly element: HTMLElement;
  private readonly input: HTMLInputElement;
  private readonly count: HTMLElement;
  /** The matches, in the order the page shows them. */
  private found: readonly string[] = [];
  private current = -1;
  /** What had focus before the bar opened. */
  private before: HTMLElement | null = null;

  constructor(
    private readonly words: MachineFindWords,
    private readonly host: MachineFindHost,
  ) {
    this.element = document.createElement("div");
    this.element.className = "find-bar mf-bar";
    this.element.setAttribute("role", "search");
    this.element.hidden = true;
    this.element.dataset.worldTitlebar = "client";
    this.element.innerHTML = `<span class="find-glyph">${glyph("search", 16)}</span><input class="find-input" type="search" enterkeyhint="search" placeholder="${escape(words.placeholder)}" aria-label="${escape(words.find)}" autocomplete="off" spellcheck="false"><span class="find-count" aria-live="polite"></span><button class="icon-button find-step" type="button" data-step="-1" aria-label="${escape(words.previous)}" title="${escape(words.previous)}">${glyph("chevron", 16)}</button><button class="icon-button find-step find-next" type="button" data-step="1" aria-label="${escape(words.next)}" title="${escape(words.next)}">${glyph("chevron", 16)}</button><button class="icon-button find-close" type="button" aria-label="${escape(words.close)}" title="${escape(words.close)}">${glyph("close", 16)}</button>`;
    this.input = this.element.querySelector<HTMLInputElement>(".find-input")!;
    this.count = this.element.querySelector<HTMLElement>(".find-count")!;
    this.input.addEventListener("input", () => {
      this.current = -1;
      this.host.changed();
    });
    this.input.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !composing(event)) {
        event.preventDefault();
        this.step(event.shiftKey ? -1 : 1);
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
      else this.step(Number(target.dataset.step));
    });
  }

  get open(): boolean {
    return !this.element.hidden;
  }

  /** What is looked for, lower-cased; empty while the bar is closed. */
  get query(): string {
    return this.open ? this.input.value.trim().toLowerCase() : "";
  }

  show(): void {
    if (this.element.hidden && document.activeElement instanceof HTMLElement) this.before = document.activeElement;
    if (this.element.hidden) layerOpened({ isOpen: () => !this.element.hidden && this.element.isConnected, close: () => this.close() });
    this.element.hidden = false;
    this.input.focus();
    this.input.select();
    this.host.changed();
  }

  close(): void {
    if (this.element.hidden) return;
    const inside = this.element.contains(document.activeElement);
    this.element.hidden = true;
    this.current = -1;
    if (inside && this.before?.isConnected) this.before.focus({ preventScroll: true });
    this.before = null;
    this.host.changed();
  }

  /** The matches as the page now shows them, which the count says. */
  showing(found: readonly string[]): void {
    const at = this.current >= 0 ? this.found[this.current] : undefined;
    this.found = found;
    this.current = at === undefined ? -1 : found.indexOf(at);
    this.say();
  }

  private step(direction: number): void {
    if (!this.found.length) return;
    const total = this.found.length;
    this.current = this.current < 0 ? (direction > 0 ? 0 : total - 1) : (this.current + direction + total) % total;
    this.say();
    this.host.reach(this.found[this.current]!);
  }

  private say(): void {
    const { words } = this;
    const total = this.found.length;
    const asked = this.query !== "";
    this.count.textContent = !asked ? "" : !total ? words.none : this.current < 0 ? words.found(total) : words.count(this.current + 1, total);
    this.count.classList.toggle("none", asked && !total);
    for (const button of this.element.querySelectorAll<HTMLButtonElement>(".find-step")) button.disabled = !total;
  }
}

/** The machines that match `query` (lower-cased), by key, in `nodes`'
 *  order: by the name each is written with or had, whose it is and the group
 *  it is in. */
export function foundIn(nodes: readonly MapNode[], query: string): string[] {
  if (!query) return [];
  return nodes.filter((node) => said(node).some((text) => text.toLowerCase().includes(query))).map((node) => node.key);
}

/** What a machine on the page is known by. */
function said(node: MapNode): string[] {
  return [node.name, ...(node.was ?? []).map((key) => key.replace(/^m:/, "")), node.group?.label ?? "", node.group?.within?.label ?? ""].filter(Boolean);
}
