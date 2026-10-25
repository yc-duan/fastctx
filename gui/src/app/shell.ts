// The frame every page sits in: the sidebar on the left and the place shown
// on the right, one screen high, nothing beside them (`world-gui.md`, 界面的
// 结构). On a narrow screen the sidebar becomes a drawer pulled from the
// left. The page itself never scrolls; what is long scrolls in its own
// region.

import type { Login } from "../world/accounts";
import { layerOpened, leavePage, type Layer } from "./back";
import { followDrawer } from "./drawer-swipe";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Place } from "./places";
import { Sidebar, type ProjectMenuOptions } from "./sidebar";
import type { ProfileWords } from "./profile-words";
import type { Store } from "./store";
import type { TalkActs } from "./talk-acts";
import type { TalkMenu } from "./talk-menu";
import type { TalkMenuWords } from "./talk-menu-words";
import type { ShellWords } from "./words";
import { effect, type Signal } from "./signals";
import type { Offered } from "./updates";
import type { UpdateWords } from "./update-words";

export interface Page {
  readonly element: HTMLElement;
  readonly title: string;
  /** Stops whatever the page keeps running: a refresh, a watch. */
  leave?(): void;
}

export interface ShellOptions {
  store: Store;
  words: ShellWords;
  /** The agents' sections, named in the sidebar. */
  sections: ProfileWords;
  newTalk(): void;
  /** A new project, in `login`'s account where the sidebar names one. */
  newProject(login: Login | null): void;
  /** What the user does to conversations, and their "⋯". */
  acts: TalkActs;
  menu: TalkMenu;
  menuWords: TalkMenuWords;
  /** A new version of the application: offered in the sidebar, and marked
   *  on the way to the drawer on a phone. */
  updates?: { offered: Signal<Offered | null>; open(): void; words: UpdateWords } | undefined;
  /** What a project's "⋯" in the sidebar does. */
  projects?: ProjectMenuOptions | undefined;
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
  /** Where the sidebar's foot is on the screen, while the sidebar shows:
   *  what passes rests above it rather than over the accounts. */
  readonly footBox = (): DOMRect | null => this.sidebar.footBox();
  private readonly main: HTMLElement;
  private readonly sidebar: Sidebar;
  private page: Page | null = null;
  private place: Place = { kind: "home" };
  /** What had focus when the drawer opened, given it back on closing. */
  private beforeDrawer: HTMLElement | null = null;
  /** The open drawer, as the system back closes it. */
  private readonly drawerLayer: Layer = {
    isOpen: () => this.element.classList.contains("drawer-open"),
    close: () => this.closeDrawer(),
  };

  constructor(private readonly options: ShellOptions) {
    this.element = document.createElement("div");
    this.element.className = "shell";
    this.element.classList.toggle("folded", keptFolded());
    this.element.innerHTML = `<div class="sidebar-frame"></div><div class="scrim"></div><main class="main"></main>`;
    this.main = this.element.querySelector<HTMLElement>(".main")!;
    this.sidebar = new Sidebar({
      store: options.store,
      words: options.words,
      sections: options.sections,
      place: () => this.place,
      newTalk: () => {
        this.closeDrawer();
        options.newTalk();
      },
      newProject: (login) => {
        this.closeDrawer();
        options.newProject(login);
      },
      fold: () => this.fold(true),
      acts: options.acts,
      menu: options.menu,
      menuWords: options.menuWords,
      updates: options.updates
        ? {
            ...options.updates,
            open: () => {
              this.closeDrawer();
              options.updates!.open();
            },
          }
        : undefined,
      projects: options.projects
        ? {
            ...options.projects,
            // Where it goes elsewhere, the drawer gives way on a phone.
            newTalk: (town) => {
              this.closeDrawer();
              options.projects!.newTalk(town);
            },
          }
        : undefined,
    });
    const updates = options.updates;
    if (updates) {
      effect(() => {
        this.element.classList.toggle("update-offered", !!updates.offered.get());
      });
    }
    this.element.querySelector(".sidebar-frame")!.append(this.sidebar.element);
    this.element.querySelector(".scrim")!.addEventListener("click", () => this.closeDrawer());
    // On a phone the drawer follows the finger.
    followDrawer({
      frame: this.element.querySelector<HTMLElement>(".sidebar-frame")!,
      scrim: this.element.querySelector<HTMLElement>(".scrim")!,
      narrow: () => narrow.matches && !this.element.classList.contains("alone"),
      isOpen: () => this.element.classList.contains("drawer-open"),
      open: () => this.drawer(true),
      close: () => this.closeDrawer(),
    });
    // Any page's menu button brings the sidebar back: as a drawer on a
    // narrow screen, unfolded on a wide one.
    this.main.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target.closest(".page-back")) {
        leavePage();
        return;
      }
      if (!target.closest(".menu-button")) return;
      if (narrow.matches) this.drawer(true);
      else this.fold(false);
    });
    // Following a link in the drawer closes it.
    this.sidebar.element.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("a")) this.closeDrawer();
    });
    addEventListener("keydown", (event) => {
      if (this.element.classList.contains("alone")) return;
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

  /** The page shown is now at `place`, as when it changed what it shows:
   *  the sidebar marks it, the page stays. */
  mark(place: Place): void {
    this.place = place;
    this.sidebar.draw();
  }

  show(place: Place, page: Page): void {
    const before = this.page;
    this.page?.leave?.();
    this.page?.element.remove();
    this.page = page;
    this.place = place;
    // Gone to another place, the page there comes in over a moment rather
    // than at a blink; the first page, and a page shown again where it was,
    // are simply there.
    if (before && before !== page) {
      page.element.classList.remove("page-arriving");
      void page.element.offsetWidth;
      page.element.classList.add("page-arriving");
      page.element.addEventListener("animationend", (event) => {
        if (event.target === page.element) page.element.classList.remove("page-arriving");
      });
    }
    this.main.append(page.element);
    document.title = page.title ? `${page.title} - ${this.options.words.app}` : this.options.words.app;
    this.sidebar.draw();
  }

  /** Folds the sidebar away or brings it back; a drawer on a narrow
   *  screen. */
  toggleSidebar(): void {
    if (narrow.matches) this.drawer(!this.element.classList.contains("drawer-open"));
    else this.fold(!this.element.classList.contains("folded"));
  }

  /** Shows the page alone, with no sidebar and no way to it: a managed
   *  machine's one page (`world-gui.md`, 受管机器上的界面). */
  alone(on: boolean): void {
    if (this.element.classList.contains("alone") === on) return;
    this.element.classList.toggle("alone", on);
    if (on) this.closeDrawer();
    this.reach();
  }

  /** Names the page shown, once it knows its own name. */
  retitle(title: string): void {
    document.title = `${title} - ${this.options.words.app}`;
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
    if (open) layerOpened(this.drawerLayer);
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
    const shown = !this.element.classList.contains("alone") && (narrow.matches ? drawerOpen : !this.element.classList.contains("folded"));
    this.element.querySelector<HTMLElement>(".sidebar-frame")!.inert = !shown;
    this.main.inert = narrow.matches && drawerOpen;
  }
}

/** The head of a page: the button that brings the sidebar back where it is
 *  hidden, the way back on a page the user came into, named by the page it
 *  goes to, the page's name, and what can be done here. */
export function pageHead(words: ShellWords, title: string, actions = "", back: string | null = null): string {
  const way = back === null
    ? ""
    : `<button class="text-button page-back" type="button" aria-label="${escape(words.backTo(back))}">${glyph("back", 18)}<span>${escape(back)}</span></button>`;
  return `<header class="page-head">
    <button class="icon-button menu-button" type="button" aria-label="${escape(words.openSidebar)}">${glyph("menu", 20)}</button>
    ${way}
    <div class="page-head-title">${title}</div>
    ${actions ? `<div class="page-head-actions">${actions}</div>` : ""}
  </header>`;
}
