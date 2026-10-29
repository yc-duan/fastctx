// The side of a conversation's page (`world-gui.md`, 对话的侧面板): shut
// until asked for, then the page's second part, set apart from the
// conversation by the margin ruled along the edge they share alone. It
// holds other conversations, a tab each at its top: a colleague the
// conversation asked, one of the host's agents, or any conversation dragged
// in from the list, each closed by its cross. Before them all stands the
// conversation's own overview, its first tab, which does not close. A
// tab's content is made when first chosen and kept while the tab is open.
// The side opens and shuts by the page's `--sp-open` (`side.css`):
// shutting, it stays drawn until that has gone back to nothing. Where
// there is no room beside the conversation, as on a phone, it lies over the
// whole page and holds the overview alone.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { msOf, reducedMotion } from "../motion";
import { Sketch } from "../pen/sketch";
import { goalMark } from "./marks";
import type { GoalStatus } from "./types";
import type { SideWords } from "./words";
import "./side.css";

/** A page drawn in the side or laid on the conversation's whole. */
export interface SidePage {
  readonly element: HTMLElement;
  /** It came into view: it may draw what needs its size. */
  shown?(): void;
}

/** What a tab shows: kept while the tab is open, let go when it closes. */
export interface PaneContent extends SidePage {
  leave?(): void;
  /** The room it has across is about to change by `by` pixels. */
  roomWillChange?(by: number): void;
}

/** Where the side stands beside the conversation. */
export type Dock = "right" | "bottom" | "left";

export interface PaneTab {
  /** The conversation or agent the tab shows. */
  id: string;
  name: string;
  /** The family's mark before the name. */
  agent: "claude" | "codex";
  /** Opens the tab's conversation as the page's own, where it is one. */
  main?: () => void;
  /** How the goal of the tab's conversation stands, where it has one. */
  goal?: GoalStatus | null;
}

interface Held {
  tab: PaneTab;
  make: () => PaneContent;
  content: PaneContent | null;
}

export class SidePane {
  readonly element: HTMLElement;
  private readonly sheet: HTMLElement;
  private readonly strip: HTMLElement;
  private readonly body: HTMLElement;
  private readonly home: HTMLElement;
  private readonly margin: HTMLElement;
  private readonly main: HTMLButtonElement;
  private readonly held = new Map<string, Held>();
  private order: string[] = [];
  private current: string | null = null;
  private readonly fresh = new Set<string>();
  private height = 0;
  private dock: Dock = "right";
  /** Over the whole page, where there is no room beside the conversation. */
  private lying = false;
  private readonly docks: HTMLElement;
  /** Shutting: still drawn while it goes. */
  private shutting = 0;

  constructor(
    private readonly words: SideWords,
    private readonly seams: {
      /** The side opened or shut, its tabs changed, or one has news. */
      changed: () => void;
      /** The user chose where the side stands. */
      dock: (dock: Dock) => void;
      /** The user asked for a side conversation branched off this one. */
      fork?: () => void;
    },
  ) {
    this.element = document.createElement("aside");
    this.element.className = "sp-panel";
    this.element.hidden = true;
    this.element.setAttribute("aria-label", words.panel);
    this.element.innerHTML = `<div class="sp-sheet"><div class="sp-margin" aria-hidden="true"></div><header class="sp-head"><nav class="sp-tabs" role="tablist"></nav><button class="icon-button sp-new" type="button" hidden aria-label="${escape(words.fork)}" title="${escape(words.fork)}">${glyph("plus", 16)}</button><div class="sp-acts"><button class="icon-button sp-main" type="button" hidden aria-label="${escape(words.openMain)}" title="${escape(words.openMain)}">${glyph("expand", 16)}</button><button class="icon-button sp-dock" type="button" aria-haspopup="menu" aria-expanded="false" aria-label="${escape(words.dock)}" title="${escape(words.dock)}">${glyph("more", 18)}</button><button class="icon-button sp-shut" type="button" aria-label="${escape(words.shut)}" title="${escape(words.shut)}">${glyph("dock-right", 18)}</button><div class="sp-dock-menu" role="menu" hidden>${(["right", "bottom", "left"] as const)
      .map((dock) => `<button class="sp-dock-row" type="button" role="menuitemradio" aria-checked="false" data-dock="${dock}">${glyph(`dock-${dock}`, 16)}<span>${escape(words.docks[dock])}</span></button>`)
      .join("")}</div></div></header><div class="sp-body"><div class="sp-home"></div></div></div>`;
    this.sheet = this.element.querySelector(".sp-sheet")!;
    this.strip = this.element.querySelector(".sp-tabs")!;
    this.body = this.element.querySelector(".sp-body")!;
    this.home = this.element.querySelector(".sp-home")!;
    this.margin = this.element.querySelector(".sp-margin")!;
    this.main = this.element.querySelector(".sp-main")!;
    this.docks = this.element.querySelector(".sp-dock-menu")!;
    const dockButton = this.element.querySelector<HTMLButtonElement>(".sp-dock")!;
    dockButton.addEventListener("click", () => this.dockMenu(this.docks.hidden));
    this.docks.addEventListener("click", (event) => {
      const dock = (event.target as HTMLElement).closest<HTMLElement>("[data-dock]")?.dataset.dock as Dock | undefined;
      if (!dock) return;
      this.dockMenu(false);
      this.seams.dock(dock);
    });
    document.addEventListener("pointerdown", (event) => {
      const target = event.target as HTMLElement;
      if (!this.docks.hidden && !this.docks.contains(target) && !dockButton.contains(target)) this.dockMenu(false);
    });
    this.main.addEventListener("click", () => (this.current ? this.held.get(this.current)?.tab.main?.() : undefined));
    this.element.querySelector(".sp-shut")!.addEventListener("click", () => this.close());
    this.element.querySelector(".sp-new")!.addEventListener("click", () => this.seams.fork?.());
    this.strip.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target.closest(".sp-tab-home")) {
        this.goHome();
        return;
      }
      const id = target.closest<HTMLElement>(".sp-tab")?.dataset.tab;
      if (!id) return;
      if (target.closest(".sp-tab-x")) this.remove(id);
      else this.select(id);
    });
    // The middle button closes a tab, as in a browser.
    this.strip.addEventListener("auxclick", (event) => {
      const id = (event.target as HTMLElement).closest<HTMLElement>(".sp-tab")?.dataset.tab;
      if (event.button === 1 && id) this.remove(id);
    });
    this.strip.addEventListener("keydown", (event) => {
      const id = (event.target as HTMLElement).closest<HTMLElement>(".sp-tab")?.dataset.tab;
      if (!id) return;
      const at = this.order.indexOf(id);
      const step = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
      if (step) {
        event.preventDefault();
        const next = this.order[(at + step + this.order.length) % this.order.length]!;
        this.select(next);
        this.strip.querySelector<HTMLElement>(`.sp-tab[data-tab="${CSS.escape(next)}"]`)?.focus();
      } else if (event.key === "Delete" || event.key === "Backspace") {
        event.preventDefault();
        this.remove(id);
      }
    });
    this.element.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && this.element.classList.contains("sp-over")) this.close();
    });
    new ResizeObserver(() => this.rule()).observe(this.sheet);
  }

  /** Open, or opening: not while it shuts. */
  get open(): boolean {
    return !this.element.hidden && !this.shutting;
  }

  /** Still drawn as it shuts. */
  get leaving(): boolean {
    return this.shutting !== 0;
  }

  /** Whether something came while it could not be seen, for the switch's dot. */
  get hasNews(): boolean {
    return this.fresh.size > 0 && !this.open;
  }

  /** The tab shown, if any is open. */
  get shownTab(): string | null {
    return this.current;
  }

  /** The tabs, in their order. */
  get tabs(): PaneTab[] {
    return this.order.map((id) => this.held.get(id)!.tab);
  }

  /** Whether a side conversation can be branched off here now. */
  offerFork(on: boolean): void {
    this.element.querySelector<HTMLButtonElement>(".sp-new")!.hidden = !on;
  }

  /** A side conversation is being branched off: the "+" waits. */
  forking(on: boolean): void {
    const button = this.element.querySelector<HTMLButtonElement>(".sp-new")!;
    button.disabled = on;
    button.setAttribute("aria-busy", String(on));
  }

  /** How the goal of a tab's conversation stands now. */
  markGoal(id: string, goal: GoalStatus | null): void {
    const held = this.held.get(id);
    if (!held || held.tab.goal === goal) return;
    held.tab = { ...held.tab, goal };
    this.drawTabs();
  }

  /** What the tab shows, once it was made. */
  content(id: string | null): PaneContent | null {
    return id ? (this.held.get(id)?.content ?? null) : null;
  }

  /** Puts a tab after the others, or names one already there anew; its
   *  content is made when it is first shown. */
  add(tab: PaneTab, make: () => PaneContent): void {
    const held = this.held.get(tab.id);
    if (held) held.tab = tab;
    else {
      this.held.set(tab.id, { tab, make, content: null });
      this.order.push(tab.id);
    }
    this.drawTabs();
    this.seams.changed();
  }

  /** Closes a tab, its content let go of. */
  remove(id: string): void {
    const held = this.held.get(id);
    if (!held) return;
    const at = this.order.indexOf(id);
    this.order.splice(at, 1);
    this.held.delete(id);
    this.fresh.delete(id);
    held.content?.leave?.();
    held.content?.element.remove();
    if (this.current === id) {
      this.current = null;
      const next = this.order[Math.max(0, at - 1)];
      if (next) this.select(next);
      else this.drawShown();
    } else this.drawTabs();
    this.seams.changed();
  }

  /** Opens the side, at `id` when one is named. */
  show(id?: string | null): void {
    if (this.shutting) {
      window.clearTimeout(this.shutting);
      this.shutting = 0;
    }
    this.element.hidden = false;
    const at = id && this.held.has(id) ? id : (this.current ?? this.order[0] ?? null);
    if (at) this.select(at);
    else {
      this.drawShown();
      this.seams.changed();
    }
  }

  /** Shuts it: it goes as `--sp-open` goes back, then is no longer drawn. */
  close(): void {
    if (!this.open) return;
    const ms = reducedMotion() ? 0 : msOf(getComputedStyle(this.element).getPropertyValue("--glide"));
    this.shutting = window.setTimeout(() => {
      this.shutting = 0;
      this.element.hidden = true;
      this.seams.changed();
    }, ms);
    this.seams.changed();
  }

  toggle(): void {
    if (this.open) this.close();
    else this.show();
  }

  select(id: string): void {
    const held = this.held.get(id);
    if (!held) return;
    this.current = id;
    this.fresh.delete(id);
    if (!held.content) {
      held.content = held.make();
      held.content.element.classList.add("sp-page");
      held.content.element.setAttribute("role", "tabpanel");
      this.body.append(held.content.element);
    }
    this.drawShown();
    if (this.open) held.content.shown?.();
    this.seams.changed();
  }

  /** What the side shows with no other tab chosen: the overview. */
  setHome(element: HTMLElement): void {
    this.home.replaceChildren(element);
  }

  /** Back to the side's home, its tabs kept. */
  goHome(): void {
    this.current = null;
    this.drawShown();
    this.seams.changed();
  }

  /** A tab has something new; said by a dot until it is looked at. */
  news(id: string): void {
    if (!this.held.has(id) || (this.open && this.current === id)) return;
    this.fresh.add(id);
    this.drawTabs();
    this.seams.changed();
  }

  /** Where there is no room beside the conversation, as on a phone, the
   *  side lies over the whole page and holds the overview alone: another
   *  conversation opens as the page's own instead, and the tabs wait for
   *  the room to come back. */
  over(over: boolean): void {
    this.lying = over;
    this.element.classList.toggle("sp-over", over);
    this.drawShut();
    this.drawShown();
  }

  /** Where the side stands: its margin is ruled along the edge it shares
   *  with the conversation, the button that shuts it drawn as the place,
   *  and its menu marks the place. */
  setDock(dock: Dock): void {
    this.dock = dock;
    this.height = 0;
    this.rule();
    this.drawShut();
    for (const row of this.docks.querySelectorAll<HTMLElement>("[data-dock]")) row.setAttribute("aria-checked", String(row.dataset.dock === dock));
  }

  /** The places the page has room for the side at now; the others are
   *  not offered. */
  fitting(docks: readonly Dock[]): void {
    for (const row of this.docks.querySelectorAll<HTMLButtonElement>("[data-dock]")) row.disabled = !docks.includes(row.dataset.dock as Dock);
  }

  /** The button that shuts the side, drawn as where it stands; a cross
   *  where it lies over the page. */
  private drawShut(): void {
    this.element.querySelector(".sp-shut")!.innerHTML = glyph(this.lying ? "close" : `dock-${this.dock}`, 18);
  }

  private dockMenu(open: boolean): void {
    this.docks.hidden = !open;
    this.element.querySelector(".sp-dock")!.setAttribute("aria-expanded", String(open));
  }

  /** The tab shown, or the home where none is chosen. */
  private drawShown(): void {
    const current = this.lying ? null : this.current;
    for (const [key, one] of this.held) if (one.content) one.content.element.hidden = key !== current;
    this.home.hidden = current !== null;
    this.main.hidden = !(current && this.held.get(current)?.tab.main);
    this.drawTabs();
  }

  /** The row of tabs, drawn again as they change: the overview first, then
   *  each conversation's family's mark, its name in the reading face and its
   *  cross; the one shown in the semibold face on a field's shade. */
  private drawTabs(): void {
    const home = this.lying || this.current === null;
    const overview = `<button class="sp-tab sp-tab-home${home ? " current" : ""}" type="button" role="tab" aria-selected="${home}" tabindex="${home ? 0 : -1}"><span class="sp-tab-name" data-label="${escape(this.words.home)}"><span>${escape(this.words.home)}</span></span></button>`;
    this.strip.innerHTML = overview + (this.lying ? [] : this.order)
      .map((id) => {
        const { tab } = this.held.get(id)!;
        const current = id === this.current;
        return `<button class="sp-tab${current ? " current" : ""}${this.fresh.has(id) ? " fresh" : ""}" type="button" role="tab" data-tab="${escape(id)}" aria-selected="${current}" tabindex="${current ? 0 : -1}" title="${escape(tab.name)}"><span class="sp-tab-seal">${glyph(tab.agent, 14)}</span><span class="sp-tab-name" data-label="${escape(tab.name)}"><span>${escape(tab.name)}</span></span>${this.goalMark(tab.goal)}<span class="sp-dot" aria-hidden="true"></span><span class="sp-tab-x" aria-label="${escape(this.words.close)}" title="${escape(this.words.close)}">${glyph("close", 12)}</span></button>`;
      })
      .join("");
    this.strip.querySelector<HTMLElement>(".sp-tab.current")?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }

  /** A goal at work is the goal's flag on its tab; one paused or stopped
   *  its own small mark; one done leaves the tab, as it leaves the bar. */
  private goalMark(goal: GoalStatus | null | undefined): string {
    if (!goal || goal === "complete") return "";
    const mark = goal === "active" ? glyph("goal", 12) : goalMark(goal, `tab-${goal}`, 12, "sp-tab-goal-svg");
    return `<span class="sp-tab-goal" aria-label="${escape(this.words.goal.states[goal])}">${mark}</span>`;
  }

  /** The margin is drawn again when the sheet's length along it changes,
   *  with the same stroke in ink over it for while it is held: down the
   *  sheet's edge beside the conversation, or across its top at the foot. */
  private rule(): void {
    const across = this.dock === "bottom";
    const length = across ? this.sheet.clientWidth : this.sheet.clientHeight;
    if (length === 0 || length === this.height) return;
    this.height = length;
    const stroke = (ink: "graphite" | "current") =>
      across
        ? new Sketch("side-margin-across").rule([4, 2], [length - 6, 2], 1.2, ink).svg(length, 4, "sp-margin-svg")
        : new Sketch("side-margin").rule([2, 4], [2, length - 6], 1.2, ink).svg(4, length, "sp-margin-svg");
    this.margin.innerHTML = `${stroke("graphite")}<div class="sp-margin-ink">${stroke("current")}</div>`;
  }
}
