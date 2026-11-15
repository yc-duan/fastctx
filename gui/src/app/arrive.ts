// How what comes in says where the reader came from (用户 2026-11-15,
// `gui-aesthetics-and-design-system`): going down into a place, the page
// comes from the right, and back up, from the left; between the views under
// one head, the body comes from the side its switch moved to, the head
// standing still; a page opened fresh comes up from a little below; what
// changes in place only fades. The marks are `data-arriving` on what comes
// (`design/arrive.css`); nothing moves more than 20px, over the app's own two
// durations, and the window clips what moves (`.main`), so no scrollbar
// flickers in.

import type { Place } from "./places";

export type Arrival = "page" | "forward" | "back" | "right" | "left" | "quiet";

/** Marks `element` as coming in `how`, from the start again if it already
 *  was, and clears the mark once its own animation ends. */
export function arrive(element: HTMLElement, how: Arrival): void {
  delete element.dataset.arriving;
  void element.offsetWidth;
  element.dataset.arriving = how;
  const ended = (event: AnimationEvent) => {
    if (event.target !== element) return;
    element.removeEventListener("animationend", ended);
    if (element.dataset.arriving === how) delete element.dataset.arriving;
  };
  element.addEventListener("animationend", ended);
}

/** Where a place stands: the places above it, then itself. One under
 *  another is gone down into; places on no line of each other are opened
 *  fresh. */
export function lineage(place: Place): string[] {
  switch (place.kind) {
    case "machine":
      return ["machines", `machine:${place.name}`];
    case "file":
      return ["machines", `machine:${place.machine}`, `file:${place.path}`];
    case "project":
      return ["projects", `project:${place.town}`];
    case "history":
      return ["projects", `project:${place.town}`, `history:${place.name}`];
    case "page":
      return ["projects", `project:${place.town}`, `page:${place.page}`];
    case "talk":
      return [`talk:${place.address.resident}@${place.address.machine}`];
    case "talk-history":
      return [`talk:${place.address.resident}@${place.address.machine}`, `history:${place.name}`];
    case "usage":
      return ["agents", `login:${place.machine}/${place.agent}/${place.dir}`];
    case "prompt":
      return ["agents", `prompt:${place.agent}`];
    case "subagents":
      return ["agents", "subagents"];
    case "feature":
      return ["features", `feature:${place.feature}`];
    case "data":
      return ["settings", "data"];
    case "archived":
      return ["talks", "archived"];
    case "sentinel":
      return ["sentinels", `sentinel:${place.home}/${place.id}`];
    default:
      return [place.kind];
  }
}

/** How the page at `to` comes, from the one at `from`. */
export function wayBetween(from: Place | null, to: Place): Arrival {
  if (!from) return "page";
  const before = lineage(from);
  const after = lineage(to);
  const under = (low: string[], high: string[]) => low.length > high.length && high.every((part, at) => low[at] === part);
  if (under(after, before)) return "forward";
  if (under(before, after)) return "back";
  return "page";
}

const chosen = (tab: Element) =>
  tab.getAttribute("aria-selected") === "true" || tab.getAttribute("aria-current") === "page" || tab.classList.contains("chosen") || tab.classList.contains("current");

/** The side a row of tabs moved to, going from what was chosen in it to
 *  `tab`; none when it stayed. */
export function sideOf(tabs: Element[], before: number, after: number): Arrival | null {
  if (before < 0 || after < 0 || before === after || !tabs.length) return null;
  return after > before ? "right" : "left";
}

/** Brings each row of tabs' panel in from the side the row moved to: the
 *  panel a tab names (`aria-controls`), or the one shown in its page. */
export function followTabs(): void {
  document.addEventListener(
    "click",
    (event) => {
      const tab = (event.target as Element | null)?.closest?.("[role='tab']");
      const list = tab?.closest("[role='tablist']");
      if (!tab || !list) return;
      const tabs = [...list.querySelectorAll("[role='tab']")].filter((one) => one.closest("[role='tablist']") === list);
      // A side dragged to is chosen before its press (`segment-slide.ts`).
      const was = (list as HTMLElement).dataset.dragFrom;
      const side = sideOf(tabs, was === undefined ? tabs.findIndex(chosen) : Number(was), tabs.indexOf(tab));
      if (!side) return;
      const scope = list.closest<HTMLElement>(".main > *, dialog") ?? document.body;
      const named = tab.getAttribute("aria-controls");
      // The page answers the press first; what it shows then comes in.
      requestAnimationFrame(() => {
        const panel =
          (named ? document.getElementById(named) : null) ??
          [...scope.querySelectorAll<HTMLElement>("[role='tabpanel']")].find((one) => !one.hidden && one.getClientRects().length > 0);
        if (panel) arrive(panel, side);
      });
    },
    true,
  );
}

/** Between two pages of one row of tabs (`data-tab-group`), the side the
 *  row moved to; none when they are not. */
export function tabSide(before: HTMLElement, after: HTMLElement): Arrival | null {
  const row = (page: HTMLElement) => page.querySelector<HTMLElement>("[data-tab-group]");
  const was = row(before);
  const is = row(after);
  if (!was || !is || was.dataset.tabGroup !== is.dataset.tabGroup) return null;
  const at = (one: HTMLElement) => [...one.children].findIndex(chosen);
  return sideOf([...is.children], at(was), at(is));
}
