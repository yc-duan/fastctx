// The frame every page sits in: the sidebar on the left and the place shown
// on the right, one screen high, nothing beside them (`world-gui.md`, 界面的
// 结构). On a narrow screen the sidebar becomes a drawer pulled from the
// left. The page itself never scrolls; what is long scrolls in its own
// region.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Place } from "./places";
import { Sidebar } from "./sidebar";
import type { Store } from "./store";
import type { ShellWords } from "./words";

export interface Page {
  readonly element: HTMLElement;
  readonly title: string;
  /** Stops whatever the page keeps running: a refresh, a watch. */
  leave?(): void;
}

export interface ShellOptions {
  store: Store;
  words: ShellWords;
  newTalk(): void;
  newProject(): void;
}

const narrow = matchMedia("(max-width: 760px)");

function keptFolded(): boolean {
  try {
    return localStorage.getItem("sidebar") === "folded";
  } catch {
    return false;
  }
}

export class Shell {
  readonly element: HTMLElement;
  private readonly main: HTMLElement;
  private readonly sidebar: Sidebar;
  private page: Page | null = null;
  private place: Place = { kind: "home" };
  /** What had focus when the drawer opened, given it back on closing. */
  private beforeDrawer: HTMLElement | null = null;

  constructor(private readonly options: ShellOptions) {
    this.element = document.createElement("div");
    this.element.className = "shell";
    this.element.classList.toggle("folded", keptFolded());
    this.element.innerHTML = `<div class="sidebar-frame"></div><div class="scrim"></div><main class="main"></main>`;
    this.main = this.element.querySelector<HTMLElement>(".main")!;
    this.sidebar = new Sidebar({
      store: options.store,
      words: options.words,
      place: () => this.place,
      newTalk: () => {
        this.closeDrawer();
        options.newTalk();
      },
      newProject: () => {
        this.closeDrawer();
        options.newProject();
      },
      fold: () => this.fold(true),
    });
    this.element.querySelector(".sidebar-frame")!.append(this.sidebar.element);
    this.element.querySelector(".scrim")!.addEventListener("click", () => this.closeDrawer());
    // Any page's menu button brings the sidebar back: as a drawer on a
    // narrow screen, unfolded on a wide one.
    this.main.addEventListener("click", (event) => {
      if (!(event.target as HTMLElement).closest(".menu-button")) return;
      if (narrow.matches) this.drawer(true);
      else this.fold(false);
    });
    // Following a link in the drawer closes it.
    this.sidebar.element.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("a")) this.closeDrawer();
    });
    addEventListener("keydown", (event) => {
      const mod = event.ctrlKey || event.metaKey;
      if (mod && !event.shiftKey && !event.altKey && event.key.toLowerCase() === "b") {
        event.preventDefault();
        this.toggleSidebar();
      } else if (mod && !event.shiftKey && !event.altKey && event.key.toLowerCase() === "n") {
        event.preventDefault();
        options.newTalk();
      } else if (event.key === "Escape" && this.element.classList.contains("drawer-open")) {
        this.closeDrawer();
      }
    });
    narrow.addEventListener("change", () => {
      this.closeDrawer();
      this.reach();
    });
    this.reach();
  }

  show(place: Place, page: Page): void {
    this.page?.leave?.();
    this.page?.element.remove();
    this.page = page;
    this.place = place;
    this.main.append(page.element);
    document.title = page.title ? `${page.title} · ${this.options.words.app}` : this.options.words.app;
    this.sidebar.draw();
  }

  /** Folds the sidebar away or brings it back; a drawer on a narrow
   *  screen. */
  toggleSidebar(): void {
    if (narrow.matches) this.drawer(!this.element.classList.contains("drawer-open"));
    else this.fold(!this.element.classList.contains("folded"));
  }

  /** Names the page shown, once it knows its own name. */
  retitle(title: string): void {
    document.title = `${title} · ${this.options.words.app}`;
  }

  private fold(shut: boolean): void {
    this.element.classList.toggle("folded", shut);
    try {
      localStorage.setItem("sidebar", shut ? "folded" : "open");
    } catch {
      // Not kept: the sidebar opens again next time.
    }
    this.reach();
  }

  private drawer(open: boolean): void {
    const was = this.element.classList.contains("drawer-open");
    if (open === was) return;
    if (open) this.beforeDrawer = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    this.element.classList.toggle("drawer-open", open);
    this.reach();
    // The drawer takes focus while it is open, and gives it back.
    if (open) this.sidebar.element.querySelector<HTMLElement>("a.current, a, button")?.focus({ preventScroll: true });
    else this.beforeDrawer?.focus({ preventScroll: true });
  }

  private closeDrawer(): void {
    this.drawer(false);
  }

  /** What cannot be seen cannot be reached: a folded sidebar or a closed
   *  drawer is left out of Tab and of a screen reader, and so is the page
   *  under an open drawer. */
  private reach(): void {
    const drawerOpen = this.element.classList.contains("drawer-open");
    const shown = narrow.matches ? drawerOpen : !this.element.classList.contains("folded");
    this.element.querySelector<HTMLElement>(".sidebar-frame")!.inert = !shown;
    this.main.inert = narrow.matches && drawerOpen;
  }
}

/** The head of a page: the button that brings the sidebar back where it is
 *  hidden, the page's name, and what can be done here. */
export function pageHead(words: ShellWords, title: string, actions = ""): string {
  return `<header class="page-head">
    <button class="icon-button menu-button" type="button" aria-label="${escape(words.openSidebar)}">${glyph("menu", 20)}</button>
    <div class="page-head-title">${title}</div>
    ${actions ? `<div class="page-head-actions">${actions}</div>` : ""}
  </header>`;
}
