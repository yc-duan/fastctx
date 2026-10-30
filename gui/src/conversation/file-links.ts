// Links an agent writes to files (`[the report](C:/Users/…/report.html)`):
// pressed, the file opens with this device's program for its kind, brought
// here first when it is on another machine; right-clicked or held, a small
// menu shows it in its folder, where it is on this computer, or copies its
// path as that machine writes it. A file that takes longer than a glance
// to come says how it comes beside the composer; one whose way broke goes
// on once the way is back, while the page is open.

import { layerOpened } from "../app/back";
import { onHeld } from "../app/press";
import type { MachineName } from "../world/events";
import type { AttachmentSource } from "../world/files";
import { HeldReads, LetGo, shareOf, type HeldRead, type ReadProgress } from "../world/reads";
import { glyph } from "./glyphs";
import { escape, naturalPath } from "./prose";

export interface FileLinkWords {
  reveal: string;
  copyPath: string;
  copied: string;
  /** Where a file on another machine is, for a pointer resting on it. */
  on(machine: string, path: string): string;
  openFailed(why: string): string;
  /** A file the user sent that is nowhere to be had any more. */
  gone: string;
  /** A file coming here that takes a while: "正在取 report.html 45%". */
  fetching(name: string, share: number | null): string;
  /** Its way broke; it comes on once the way is back. */
  fetchInterrupted(name: string): string;
}

export interface FileLinkOptions {
  words: FileLinkWords;
  /** This computer. */
  here: MachineName;
  /** The conversation's own machine, whose files the agent's links name
   *  where nothing else says. */
  machine: MachineName;
  /** The machine the file `path` that `link` names is on, as the turn its
   *  words are in says; absent where it says nothing. */
  machineFor?: ((link: HTMLElement, path: string) => MachineName | null) | undefined;
  /** Opens a file on `machine` here, bringing it over first when needed,
   *  as the read `held`. */
  open(machine: MachineName, path: string, held: HeldRead): Promise<void>;
  /** Opens a file the user sent here: the World's copy, or where that is
   *  gone, its host's own record of it. */
  openAttachment?: ((attachment: AttachmentSource, held: HeldRead) => Promise<void>) | undefined;
  /** Shows a file on this computer in its folder; absent where there is no
   *  folder to show (a phone). */
  reveal?: ((path: string) => Promise<void>) | undefined;
  /** A line beside the composer: why a file did not open, or that its
   *  path was copied; `detail`, the original of a failure, on hover. */
  say(text: string, detail?: string): void;
  /** The same line saying how a file comes, with `detail` on hover worked
   *  out when the pointer comes to it; `unsay` takes it back unless
   *  something else was said since. */
  note(text: string, detail: (() => string) | null): void;
  unsay(text: string): void;
  /** For how long the way to a file has been broken, since `since`
   *  (milliseconds since the epoch), and why. */
  broken(since: number, why: string): string;
}

/** How long a file may take to come before the line says it is coming: a
 *  glance, so one that opens at once does so without a word. */
const SAY_AFTER = 400;

export class FileLinks {
  private menu: HTMLElement | null = null;
  /** The files coming here for the links pressed. */
  private readonly reads = new HeldReads();

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
      this.open(link);
    });
    onHeld(root, "a[data-file]", (link, event, input) => {
      event.stopPropagation();
      this.openMenu(link, event.clientX, event.clientY, input === "touch");
    });
    // Where the file is, said on hover only once asked for.
    root.addEventListener("pointerover", (event) => {
      const link = linkOf(event.target);
      if (!link || link.title) return;
      const path = naturalPath(link.dataset.file!);
      const machine = this.machineOf(link);
      link.title = machine === options.here ? path : options.words.on(machine, path);
    });
  }

  /** The machine holding the file `link` names. */
  private machineOf(link: HTMLElement): MachineName {
    const { options } = this;
    return link.dataset.fileMachine ?? options.machineFor?.(link, link.dataset.file!) ?? options.machine;
  }

  /** The page went: the files still coming are not wanted. */
  release(): void {
    this.reads.release();
    this.closeMenu();
  }

  private open(link: HTMLElement): void {
    const { options } = this;
    const { words } = options;
    const sent = link.dataset.attachment && options.openAttachment ? (JSON.parse(link.dataset.attachment) as AttachmentSource) : null;
    const path = link.dataset.file!;
    const name = (sent ? link.textContent?.trim() : null) || path.split(/[\\/]/).pop() || path;
    let progress: ReadProgress | null = null;
    let said: string | null = null;
    const tell = () => {
      if (said === null) return;
      const broken = progress?.interrupted ?? null;
      said = broken ? words.fetchInterrupted(name) : words.fetching(name, shareOf(progress));
      options.note(said, broken ? () => options.broken(broken.since, broken.why) : null);
    };
    const saying = setTimeout(() => {
      said = "";
      tell();
    }, SAY_AFTER);
    const opening = (held: HeldRead) =>
      sent && options.openAttachment ? options.openAttachment(sent, held) : options.open(this.machineOf(link), path, held);
    void this.reads
      .hold(opening, (now) => {
        progress = now;
        tell();
      })
      .then(
        () => {
          clearTimeout(saying);
          if (said) options.unsay(said);
        },
        (error: unknown) => {
          clearTimeout(saying);
          if (error instanceof LetGo) return;
          const gone = !!error && typeof error === "object" && "gone" in error && error.gone === true;
          const why = error instanceof Error ? error.message : String(error);
          options.say(gone ? words.gone : words.openFailed(why), gone ? undefined : why);
        },
      );
  }

  private openMenu(link: HTMLElement, x: number, y: number, sheet: boolean): void {
    this.closeMenu();
    const { words, here, reveal } = this.options;
    const machine = this.machineOf(link);
    const path = link.dataset.file!;
    const menu = document.createElement("div");
    menu.className = `file-menu${sheet ? " as-sheet" : ""}`;
    menu.setAttribute("role", "menu");
    menu.innerHTML = [
      // Where the file is, said where a pointer cannot rest on the link.
      `<p class="file-menu-where mono">${escape(machine === here ? naturalPath(path) : words.on(machine, naturalPath(path)))}</p>`,
      machine === here && reveal ? `<button class="file-menu-item" type="button" role="menuitem" data-file-act="reveal">${glyph("folder", 16)}<span>${escape(words.reveal)}</span></button>` : "",
      `<button class="file-menu-item" type="button" role="menuitem" data-file-act="copy">${glyph("link", 16)}<span>${escape(words.copyPath)}</span></button>`,
    ].join("");
    document.body.append(menu);
    if (!sheet) {
      const left = Math.min(x, innerWidth - menu.offsetWidth - 8);
      const top = Math.min(y, innerHeight - menu.offsetHeight - 8);
      menu.style.transform = `translate(${Math.max(8, left)}px, ${Math.max(8, top)}px)`;
    }
    menu.addEventListener("click", (event) => {
      const act = (event.target as HTMLElement).closest<HTMLElement>("[data-file-act]")?.dataset.fileAct;
      if (act === "reveal")
        void reveal!(path).catch((error: unknown) => {
          const why = error instanceof Error ? error.message : String(error);
          this.options.say(this.options.words.openFailed(why), why);
        });
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
    layerOpened({ isOpen: () => menu.isConnected, close: () => this.closeMenu(), holds: (target) => menu.contains(target) });
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
