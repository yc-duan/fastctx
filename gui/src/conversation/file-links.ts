// Links an agent writes to files (`[the report](C:/Users/…/report.html)`):
// pressed, the file opens with this device's program for its kind, brought
// here first when it is on another machine; right-clicked or held, a small
// menu shows it in its folder, where it is on this computer, or copies its
// path as that machine writes it.

import type { MachineName } from "../world/events";
import { glyph } from "./glyphs";
import { escape, naturalPath } from "./prose";

export interface FileLinkWords {
  reveal: string;
  copyPath: string;
  copied: string;
  /** Where a file on another machine is, for a pointer resting on it. */
  on(machine: string, path: string): string;
  openFailed(why: string): string;
}

export interface FileLinkOptions {
  words: FileLinkWords;
  /** This computer. */
  here: MachineName;
  /** The machine whose files the agent's links name. */
  machine: MachineName;
  /** Opens a file on `machine` here, bringing it over first when needed. */
  open(machine: MachineName, path: string): Promise<void>;
  /** Shows a file on this computer in its folder; absent where there is no
   *  folder to show (a phone). */
  reveal?: ((path: string) => Promise<void>) | undefined;
  /** A line beside the composer: why a file did not open, or that its
   *  path was copied. */
  say(text: string): void;
}

export class FileLinks {
  private menu: HTMLElement | null = null;

  constructor(
    root: HTMLElement,
    private readonly options: FileLinkOptions,
  ) {
    root.addEventListener("click", (event) => {
      const link = linkOf(event.target);
      if (!link) return;
      event.preventDefault();
      event.stopPropagation();
      this.closeMenu();
      void options.open(link.dataset.fileMachine ?? options.machine, link.dataset.file!).catch((error: unknown) => options.say(options.words.openFailed(error instanceof Error ? error.message : String(error))));
    });
    root.addEventListener("contextmenu", (event) => {
      const link = linkOf(event.target);
      if (!link) return;
      event.preventDefault();
      event.stopPropagation();
      this.openMenu(link, event.clientX, event.clientY);
    });
    // Where the file is, said on hover only once asked for.
    root.addEventListener("pointerover", (event) => {
      const link = linkOf(event.target);
      if (!link || link.title) return;
      const path = naturalPath(link.dataset.file!);
      const machine = link.dataset.fileMachine ?? options.machine;
      link.title = machine === options.here ? path : options.words.on(machine, path);
    });
  }

  private openMenu(link: HTMLElement, x: number, y: number): void {
    this.closeMenu();
    const { words, here, reveal } = this.options;
    const machine = link.dataset.fileMachine ?? this.options.machine;
    const path = link.dataset.file!;
    const sheet = matchMedia("(hover: none)").matches;
    const menu = document.createElement("div");
    menu.className = `file-menu${sheet ? " as-sheet" : ""}`;
    menu.setAttribute("role", "menu");
    menu.innerHTML = [
      machine === here && reveal ? `<button class="file-menu-item" type="button" role="menuitem" data-file-act="reveal">${glyph("folder", 16)}<span>${escape(words.reveal)}</span></button>` : "",
      `<button class="file-menu-item" type="button" role="menuitem" data-file-act="copy">${glyph("copy", 16)}<span>${escape(words.copyPath)}</span></button>`,
    ].join("");
    document.body.append(menu);
    if (!sheet) {
      const left = Math.min(x, innerWidth - menu.offsetWidth - 8);
      const top = Math.min(y, innerHeight - menu.offsetHeight - 8);
      menu.style.transform = `translate(${Math.max(8, left)}px, ${Math.max(8, top)}px)`;
    }
    menu.addEventListener("click", (event) => {
      const act = (event.target as HTMLElement).closest<HTMLElement>("[data-file-act]")?.dataset.fileAct;
      if (act === "reveal") void reveal!(path).catch((error: unknown) => this.options.say(this.options.words.openFailed(error instanceof Error ? error.message : String(error))));
      if (act === "copy") void navigator.clipboard?.writeText(naturalPath(path)).then(() => this.options.say(words.copied));
      this.closeMenu();
    });
    const away = (event: Event) => {
      if (!menu.contains(event.target as Node)) this.closeMenu();
    };
    const escape_ = (event: KeyboardEvent) => {
      if (event.key === "Escape") this.closeMenu();
    };
    setTimeout(() => {
      document.addEventListener("pointerdown", away, true);
      document.addEventListener("keydown", escape_, true);
    });
    this.menu = menu;
    this.forget = () => {
      document.removeEventListener("pointerdown", away, true);
      document.removeEventListener("keydown", escape_, true);
    };
    menu.querySelector<HTMLElement>("button")?.focus({ preventScroll: true });
  }

  private forget: () => void = () => {};

  private closeMenu(): void {
    this.menu?.remove();
    this.menu = null;
    this.forget();
    this.forget = () => {};
  }
}

const linkOf = (target: EventTarget | null): HTMLElement | null => (target instanceof Element ? target.closest<HTMLElement>("a[data-file]") : null);
