// The panel at the right of a conversation (`world-gui.md`, 对话右边的侧面板):
// shut until asked for, then a sheet beside the conversation, under its head,
// with a margin ruled down its left edge and its pages named at its top, the
// one shown in the semibold face on a field's shade. A page that has something
// new while not shown carries a dot after its name, as the switch does while
// the panel is shut. It opens and shuts by the page's `--sp-open` (`side.css`):
// shutting, it stays drawn until that has gone back to nothing. On a phone it
// lies over the conversation.

import { escape } from "../conversation/prose";
import { reducedMotion } from "../motion";
import { Sketch } from "../pen/sketch";
import type { SideWords } from "./words";
import "./side.css";

export type SidePageName = "plan" | "goal" | "collab";

export interface SidePage {
  readonly element: HTMLElement;
  /** The page came into view: it may draw what needs its size. */
  shown?(): void;
}

export class SidePanel {
  readonly element: HTMLElement;
  private readonly sheet: HTMLElement;
  private readonly body: HTMLElement;
  private readonly margin: HTMLElement;
  private current: SidePageName;
  private readonly fresh = new Set<SidePageName>();
  private height = 0;
  /** Shutting: still drawn while it goes. */
  private shutting = 0;

  constructor(
    words: SideWords,
    private readonly pages: { plan: SidePage; goal?: SidePage; collab: SidePage },
    private readonly seams: {
      /** The panel opened or shut, or a page has news: the switch redraws. */
      changed: () => void;
    },
    first: SidePageName = "plan",
  ) {
    this.current = first;
    this.element = document.createElement("aside");
    this.element.className = "sp-panel";
    this.element.hidden = true;
    this.element.setAttribute("aria-label", words.panel);
    const names = (["plan", "goal", "collab"] as const).filter((name) => this.pages[name]);
    this.element.innerHTML = `<div class="sp-sheet"><div class="sp-margin" aria-hidden="true"></div><header class="sp-head"><nav class="sp-tabs" role="tablist">${names
      .map((name) => `<button class="sp-tab" type="button" role="tab" data-page="${name}"><span data-label="${escape(words.pages[name])}">${escape(words.pages[name])}</span><span class="sp-dot" aria-hidden="true"></span></button>`)
      .join("")}</nav></header><div class="sp-body"></div></div>`;
    this.sheet = this.element.querySelector(".sp-sheet")!;
    this.body = this.element.querySelector(".sp-body")!;
    this.margin = this.element.querySelector(".sp-margin")!;
    for (const name of names) {
      const page = this.pages[name]!;
      page.element.classList.add("sp-page");
      page.element.setAttribute("role", "tabpanel");
      page.element.hidden = true;
      this.body.append(page.element);
    }
    this.element.querySelector(".sp-tabs")!.addEventListener("click", (event) => {
      const tab = (event.target as HTMLElement).closest<HTMLButtonElement>(".sp-tab");
      if (tab?.dataset.page) this.select(tab.dataset.page as SidePageName);
    });
    this.element.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && this.element.classList.contains("sp-over")) this.close();
    });
    new ResizeObserver(() => this.rule()).observe(this.sheet);
    this.drawTabs();
  }

  /** Open, or opening: not while it shuts. */
  get open(): boolean {
    return !this.element.hidden && !this.shutting;
  }

  /** Still drawn as it shuts. */
  get leaving(): boolean {
    return this.shutting !== 0;
  }

  /** Whether something came while the panel was shut, for the switch's dot. */
  get hasNews(): boolean {
    return this.fresh.size > 0 && !this.open;
  }

  /** Opens the panel, at `page` when one is named. */
  show(page?: SidePageName): void {
    if (page) this.current = page;
    if (this.shutting) {
      window.clearTimeout(this.shutting);
      this.shutting = 0;
    }
    this.element.hidden = false;
    this.select(this.current);
    this.seams.changed();
  }

  /** Shuts it: it goes as `--sp-open` goes back, then is no longer drawn. */
  close(): void {
    if (!this.open) return;
    const settle = getComputedStyle(this.element).getPropertyValue("--settle").trim();
    const ms = reducedMotion() ? 0 : parseFloat(settle) * (settle.endsWith("ms") ? 1 : 1000) || 0;
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

  select(page: SidePageName): void {
    if (!this.pages[page]) page = "plan";
    this.current = page;
    this.fresh.delete(page);
    for (const [name, shown] of Object.entries(this.pages) as [SidePageName, SidePage][]) shown.element.hidden = name !== page;
    this.drawTabs();
    if (this.open) this.pages[page]?.shown?.();
    this.seams.changed();
  }

  /** A page has something new; said by a dot until it is looked at. */
  news(page: SidePageName): void {
    if (!this.pages[page] || (this.open && this.current === page)) return;
    this.fresh.add(page);
    this.drawTabs();
    this.seams.changed();
  }

  /** On a phone the panel lies over the conversation. */
  over(over: boolean): void {
    this.element.classList.toggle("sp-over", over);
  }

  private drawTabs(): void {
    for (const tab of this.element.querySelectorAll<HTMLButtonElement>(".sp-tab")) {
      const name = tab.dataset.page as SidePageName;
      const current = name === this.current;
      tab.classList.toggle("current", current);
      tab.setAttribute("aria-selected", String(current));
      tab.classList.toggle("fresh", this.fresh.has(name));
    }
  }

  /** The margin is drawn again when the sheet's height changes. */
  private rule(): void {
    const height = this.sheet.clientHeight;
    if (height === 0 || height === this.height) return;
    this.height = height;
    this.margin.innerHTML = new Sketch("side-margin").rule([2, 6], [2, height - 6], 1.2, "graphite").svg(4, height, "sp-margin-svg");
  }
}
