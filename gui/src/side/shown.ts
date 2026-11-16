// Which of the overview's cards are shown (the user, 2026-11-16: the side
// is not to be fixed at plan, files, helpers in a row; "用户这边直接右键"):
// pressed with the other button, the overview opens a small menu of its
// cards, a tick after each one shown, and a card pressed there comes or
// goes. The side's "⋯" holds the same rows. What is chosen is this
// device's, the same for every conversation.

import { glyph, type GlyphName } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { SideWords } from "./words";

export type Card = "goal" | "plan" | "files" | "colleagues" | "agents";

/** The cards chosen among, in the order they stand on the overview. The
 *  goal's is not one: beside the conversation its bar says it. */
export const CHOSEN: readonly Card[] = ["plan", "files", "colleagues", "agents"];

const KEPT = "side-home-hidden";
const MARK: Record<Card, GlyphName> = { goal: "goal", plan: "steps", files: "file", colleagues: "colleague", agents: "agents" };

let hidden: Set<Card> | null = null;
const heard = new Set<() => void>();

function read(): Set<Card> {
  if (hidden) return hidden;
  try {
    const kept = JSON.parse(localStorage.getItem(KEPT) ?? "[]") as Card[];
    hidden = new Set(kept.filter((card) => CHOSEN.includes(card)));
  } catch {
    hidden = new Set();
  }
  return hidden;
}

export function cardHidden(card: Card): boolean {
  return read().has(card);
}

/** Shows a card or puts it away, and tells every overview. */
export function showCard(card: Card, shown: boolean): void {
  const now = read();
  if (shown === !now.has(card)) return;
  if (shown) now.delete(card);
  else now.add(card);
  try {
    localStorage.setItem(KEPT, JSON.stringify([...now]));
  } catch {
    // Chosen for this visit only.
  }
  for (const told of [...heard]) told();
}

/** Hears the choice change; the answer stops hearing. */
export function onCardsChosen(told: () => void): () => void {
  heard.add(told);
  return () => heard.delete(told);
}

export function cardNames(words: SideWords): Record<Card, string> {
  return { goal: words.pages.goal, plan: words.pages.plan, files: words.pages.files, colleagues: words.collab.groups.colleagues, agents: words.collab.groups.agents };
}

/** The menu's rows: a card's mark and name, a tick where it is shown. */
export function cardRows(names: Record<Card, string>): string {
  return CHOSEN.map(
    (card) => `<button class="sp-menu-row sp-menu-card" type="button" role="menuitemcheckbox" aria-checked="${!cardHidden(card)}" data-card="${card}">${glyph(MARK[card], 16)}<span>${escape(names[card])}</span>${glyph("check", 14)}</button>`,
  ).join("");
}

/** A row pressed: its card comes or goes; true where a row was pressed. */
export function cardPressed(target: HTMLElement): boolean {
  const card = target.closest<HTMLElement>(".sp-menu-card")?.dataset.card as Card | undefined;
  if (!card) return false;
  showCard(card, cardHidden(card));
  return true;
}

let floating: { element: HTMLElement; shut(): void } | null = null;

/** Opens the menu at a point of the window, as a press with the other
 *  button asks; it stays while cards are chosen and goes at a press
 *  outside it, Escape, or the window being left. `pull`, where the press
 *  was on a card that can be taken out: the menu's first row takes it out
 *  into a window of its own. */
export function openCardsMenu(x: number, y: number, words: SideWords, pull?: () => void): void {
  floating?.shut();
  const names = cardNames(words);
  const menu = document.createElement("div");
  menu.className = "sp-menu sp-menu-float";
  menu.setAttribute("role", "menu");
  menu.setAttribute("aria-label", words.home.shows);
  const draw = () => {
    menu.innerHTML = `${pull ? `<button class="sp-menu-row" type="button" role="menuitem" data-pull>${glyph("dock-out", 16)}<span>${escape(words.board.out)}</span></button><div class="sp-menu-rule" aria-hidden="true"></div>` : ""}${cardRows(names)}`;
  };
  draw();
  document.body.append(menu);
  // Kept inside the window, opening up or to the left where it must.
  const box = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(8, Math.min(x, innerWidth - box.width - 8))}px`;
  menu.style.top = `${Math.max(8, Math.min(y, innerHeight - box.height - 8))}px`;
  const stop = onCardsChosen(draw);
  const outside = (event: Event) => {
    if (!menu.contains(event.target as Node)) shut();
  };
  const key = (event: KeyboardEvent) => {
    if (event.key === "Escape") shut();
  };
  function shut(): void {
    stop();
    document.removeEventListener("pointerdown", outside, true);
    document.removeEventListener("keydown", key, true);
    removeEventListener("blur", shut);
    removeEventListener("resize", shut);
    menu.remove();
    if (floating?.element === menu) floating = null;
  }
  menu.addEventListener("click", (event) => {
    if (pull && (event.target as HTMLElement).closest("[data-pull]")) {
      shut();
      pull();
    } else cardPressed(event.target as HTMLElement);
  });
  menu.addEventListener("contextmenu", (event) => event.preventDefault());
  document.addEventListener("pointerdown", outside, true);
  document.addEventListener("keydown", key, true);
  addEventListener("blur", shut);
  addEventListener("resize", shut);
  floating = { element: menu, shut };
  menu.querySelector<HTMLElement>(".sp-menu-row")?.focus({ preventScroll: true });
}
