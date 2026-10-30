// A conversation's "⋯" (`world-gui.md`, 界面的结构): move it to a project,
// rename it, archive it, delete it; an archived one is restored or deleted.
// On a computer it opens by the "⋯" a row or the conversation's head shows,
// or where a row is pressed with the right button; on a narrow screen it
// rises from below, headed by the conversation it is about. With its machine
// away only deleting goes ahead, the rest grey until the machine is back.
// Moving lists in place of the acts the projects spanning its machine.

import { leave, leaving, stay } from "./leave";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { titleOf } from "../world/names";
import type { ResidentNow } from "../world/world";
import { layerOpened, type Layer } from "./back";
import type { TalkActs } from "./talk-acts";
import type { TalkMenuWords } from "./talk-menu-words";
import { stepped } from "./menu-keys";

export interface TalkMenuOptions {
  words: TalkMenuWords;
  acts: TalkActs;
  /** The projects a conversation on `machine` can be in. */
  /** The projects a conversation can be in. */
  towns(r: ResidentNow): { town: string; name: string }[];
  /** Whether the conversation's machine is there to be asked now. */
  reachable(r: ResidentNow): boolean;
  /** Whether its machine is out of reach for good. */
  departed?(r: ResidentNow): boolean;
  /** Who it is with, where it has no name; where it is said to be. Never
   *  the computer an agent assisting the user runs on. */
  someone(r: ResidentNow): string;
  place(r: ResidentNow): string;
  /** Asks before deleting, and deletes; then `then`. */
  askDelete(r: ResidentNow, then?: () => void): void;
  /** Opens it side by side with the conversations shown. */
  beside?(r: ResidentNow): void;
}

export interface MenuOpening {
  /** Opened by this button, which keeps the menu beside it. */
  anchor?: HTMLElement;
  /** Opened where the pointer was. */
  at?: { x: number; y: number };
  /** Renames it where it is shown. */
  rename(): void;
  /** Opened from its own head, where it is shown already. */
  shown?: boolean;
  /** Its head has no room for finding and the choice of view: they are
   *  here, as on a phone. */
  folded?: boolean;
  /** Finds in it, where its page is the one shown. */
  find?: () => void;
  /** Its page's choice between every step and only what was said. */
  steps?: { shown(): boolean; toggle(): void };
  /** Opens the panel beside it, where it holds anything. */
  panel?: (() => void) | null;
  /** Once its deletion is asked for: a page of it goes. */
  deleted?: () => void;
  /** Copies the whole conversation as Markdown, from its own page. */
  copy?: () => void;
  /** Opens it in its agent's own program on this computer, where it was
   *  begun here from Fastctx. */
  attach?: { agent: string; run(): void };
}

const narrow = matchMedia("(max-width: 760px)");

export class TalkMenu {
  private readonly element: HTMLElement;
  private readonly shade: HTMLElement;
  private opened: { r: ResidentNow; how: MenuOpening } | null = null;
  private towns = false;
  private readonly layer: Layer = {
    isOpen: () => !this.element.hidden,
    close: () => this.close(),
    holds: (target) => this.element.contains(target),
  };

  constructor(private readonly options: TalkMenuOptions) {
    this.shade = document.createElement("div");
    this.shade.className = "talk-menu-shade";
    this.shade.hidden = true;
    this.element = document.createElement("div");
    this.element.className = "talk-menu";
    this.element.setAttribute("role", "menu");
    this.element.hidden = true;
    // Opened from a caption, it may lie over one, which drags the window on Windows.
    this.element.dataset.worldTitlebar = "client";
    document.body.append(this.shade, this.element);
    this.shade.addEventListener("click", () => this.close());
    this.element.addEventListener("click", (event) => this.pick(event));
    this.element.addEventListener("keydown", (event) => this.key(event));
    document.addEventListener(
      "pointerdown",
      (event) => {
        if (this.element.hidden) return;
        const path = event.composedPath();
        if (path.includes(this.element) || (this.opened?.how.anchor && path.includes(this.opened.how.anchor))) return;
        this.close();
      },
      true,
    );
    addEventListener("resize", () => this.close());
  }

  /** Whether it is open for `r` now. */
  isOpenFor(r: ResidentNow): boolean {
    return !this.element.hidden && this.opened?.r.about.id === r.about.id && this.opened.r.about.machine === r.about.machine;
  }

  /** Asks before deleting `r`, as its menu's delete does. */
  askDelete(r: ResidentNow): void {
    this.options.askDelete(r);
  }

  open(r: ResidentNow, how: MenuOpening): void {
    if (this.isOpenFor(r) && how.anchor) {
      this.close();
      return;
    }
    stay(this.element);
    stay(this.shade);
    this.opened = { r, how };
    this.towns = false;
    this.draw();
    const sheet = narrow.matches;
    this.element.classList.toggle("as-sheet", sheet);
    this.shade.hidden = !sheet;
    this.element.hidden = false;
    this.fit();
    how.anchor?.setAttribute("aria-expanded", "true");
    how.anchor?.closest(".talk-item")?.classList.add("menu-open");
    if (!sheet) this.place();
    layerOpened(this.layer);
    // Opened from the keyboard, the first act takes the focus.
    if (document.activeElement === how.anchor || !how.anchor) this.items()[0]?.focus({ preventScroll: true });
  }

  /** The menu's size while it is open: as tall and as wide as the larger
   *  of its two faces, its acts and the projects to move to, so turning
   *  from one to the other moves nothing. Both are laid out at once, before
   *  it is painted. */
  private fit(): void {
    this.element.style.height = this.element.style.width = "";
    if (!this.opened || this.opened.r.removed) return;
    let height = 0;
    let width = 0;
    for (const towns of [true, false]) {
      this.towns = towns;
      this.draw();
      height = Math.max(height, this.element.offsetHeight);
      width = Math.max(width, this.element.offsetWidth);
    }
    this.element.style.height = `${height}px`;
    if (!this.element.classList.contains("as-sheet")) this.element.style.width = `${width}px`;
  }

  close(): void {
    if (this.element.hidden || leaving(this.element)) return;
    leave(this.element, () => {
      this.element.hidden = true;
      this.element.style.height = this.element.style.width = "";
    });
    if (!this.shade.hidden) leave(this.shade, () => (this.shade.hidden = true));
    const anchor = this.opened?.how.anchor;
    anchor?.setAttribute("aria-expanded", "false");
    anchor?.closest(".talk-item")?.classList.remove("menu-open");
    if (this.element.contains(document.activeElement)) anchor?.focus({ preventScroll: true });
    this.opened = null;
  }

  private items(): HTMLButtonElement[] {
    return [...this.element.querySelectorAll<HTMLButtonElement>("button:not([disabled])")];
  }

  private draw(): void {
    if (!this.opened) return;
    const { words, acts } = this.options;
    const r = acts.view(this.opened.r);
    const away = !this.options.reachable(r);
    const head = narrow.matches
      ? `<div class="talk-menu-head"><span class="talk-menu-host">${glyph(r.about.host.agent === "claude" || r.about.host.agent === "codex" ? r.about.host.agent : "agents", 18)}</span><span class="talk-menu-name">${escape(titleOf(r) ?? this.options.someone(r))}</span></div>`
      : "";
    const note = away ? `<p class="talk-menu-note">${escape(this.options.departed?.(r) ? words.departed : words.offline(this.options.place(r)))}</p>` : "";
    const item = (act: string, icon: Parameters<typeof glyph>[0], label: string, disabled: boolean, end = "") =>
      `<button class="talk-menu-item${act === "delete" ? " danger" : ""}" type="button" role="menuitem" data-act="${act}"${disabled ? " disabled" : ""}>${glyph(icon, 16)}<span class="talk-menu-label">${escape(label)}</span>${end}</button>`;
    const rule = `<hr class="talk-menu-rule">`;
    if (this.towns) {
      const towns = this.options.towns(r);
      const pick = (town: string | null, name: string) =>
        `<button class="talk-menu-item talk-menu-town" type="button" role="menuitemradio" aria-checked="${(r.town ?? null) === town}" data-town="${escape(town ?? "")}"><span class="talk-menu-check">${(r.town ?? null) === town ? glyph("check", 16) : ""}</span><span class="talk-menu-label">${escape(name)}</span></button>`;
      this.element.innerHTML = `${head}<button class="talk-menu-back" type="button" data-act="back">${glyph("back", 16)}<span>${escape(words.move)}</span></button>
        ${towns.length ? towns.map((town) => pick(town.town, town.name)).join("") : `<p class="talk-menu-note">${escape(words.noProjects)}</p>`}
        ${towns.length || r.town ? `${rule}${pick(null, words.noProject)}` : ""}`;
      return;
    }
    const body = r.removed
      ? [item("restore", "unarchive", words.restore, away), rule, item("delete", "trash", words.delete, false)]
      : [
          // On a phone the head has no room for finding or for the choice
          // of view: they are here.
          // A phone shows one conversation at a time.
          ...(this.options.beside && !this.opened.how.shown && !narrow.matches ? [item("beside", "split", words.beside, false), rule] : []),
          ...((narrow.matches || this.opened.how.folded) && (this.opened.how.find || this.opened.how.steps || this.opened.how.panel)
            ? [
                ...(this.opened.how.panel ? [item("panel", "panel", words.panel, false)] : []),
                ...(this.opened.how.steps ? [item("steps", "steps", this.opened.how.steps.shown() ? words.viewSaid : words.viewSteps, false)] : []),
                ...(this.opened.how.find ? [item("find", "search", words.find, false)] : []),
                rule,
              ]
            : []),
          item("move", "move", words.move, away, `<span class="talk-menu-more">${glyph("chevron", 14)}</span>`),
          item("rename", "edit", words.rename, away),
          item("archive", "archive", words.archive, away),
          ...(this.opened.how.copy || this.opened.how.attach
            ? [
                rule,
                ...(this.opened.how.copy ? [item("copy", "copy", words.copyMarkdown, false)] : []),
                ...(this.opened.how.attach ? [item("attach", "terminal", words.openOwn(this.opened.how.attach.agent), false)] : []),
              ]
            : []),
          rule,
          item("delete", "trash", words.delete, false),
        ];
    this.element.innerHTML = `${head}${note}${body.join("")}`;
  }

  /** Beside what opened it, in the window. */
  private place(): void {
    const how = this.opened?.how;
    const box = how?.anchor?.getBoundingClientRect();
    const width = this.element.offsetWidth;
    const height = this.element.offsetHeight;
    let left = box ? box.right - width : (how?.at?.x ?? 0);
    let top = box ? box.bottom + 4 : (how?.at?.y ?? 0);
    // A row's "⋯" is at its end: the menu opens into the page beside it.
    if (box && box.left < innerWidth / 2) left = box.left - 6;
    left = Math.min(Math.max(8, left), innerWidth - width - 8);
    if (top + height > innerHeight - 8) top = Math.max(8, (box ? box.top - 4 : top) - height);
    this.element.style.left = `${Math.round(left)}px`;
    this.element.style.top = `${Math.round(top)}px`;
  }

  private pick(event: MouseEvent): void {
    const target = event.target as HTMLElement;
    const opened = this.opened;
    if (!opened) return;
    const town = target.closest<HTMLElement>("[data-town]");
    if (town) {
      const chosen = town.dataset.town || null;
      const r = opened.r;
      this.close();
      if ((this.options.acts.view(r).town ?? null) !== chosen) this.options.acts.move(r, chosen);
      return;
    }
    const button = target.closest<HTMLButtonElement>("[data-act]");
    if (!button || button.disabled) return;
    const r = opened.r;
    switch (button.dataset.act) {
      case "move":
        this.towns = true;
        this.draw();
        if (!narrow.matches) this.place();
        this.items()[1]?.focus({ preventScroll: true });
        return;
      case "back":
        this.towns = false;
        this.draw();
        if (!narrow.matches) this.place();
        this.items()[0]?.focus({ preventScroll: true });
        return;
      case "rename": {
        const rename = opened.how.rename;
        this.close();
        rename();
        return;
      }
      case "find": {
        const find = opened.how.find;
        this.close();
        find?.();
        return;
      }
      case "steps": {
        const steps = opened.how.steps;
        this.close();
        steps?.toggle();
        return;
      }
      case "beside": {
        this.close();
        this.options.beside?.(r);
        return;
      }
      case "panel": {
        const panel = opened.how.panel;
        this.close();
        panel?.();
        return;
      }
      case "archive":
        this.close();
        this.options.acts.archive(r);
        return;
      case "copy": {
        const copy = opened.how.copy;
        this.close();
        copy?.();
        return;
      }
      case "attach": {
        const attach = opened.how.attach;
        this.close();
        attach?.run();
        return;
      }
      case "restore":
        this.close();
        this.options.acts.restore(r);
        return;
      case "delete": {
        const deleted = opened.how.deleted;
        this.close();
        this.options.askDelete(r, deleted);
        return;
      }
    }
  }

  private key(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      this.close();
      return;
    }
    const items = this.items();
    const next = stepped(items, document.activeElement, event.key);
    if (next === null) return;
    event.preventDefault();
    items[next]?.focus({ preventScroll: true });
  }
}
