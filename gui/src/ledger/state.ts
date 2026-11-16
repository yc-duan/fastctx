// What the agents' pages are showing, and the few things they share: where
// the reader is, what each view was last set to, the deeds a press may ask
// for, and how the page is drawn again.

import { deeds, type Outcome, type Source, type Split } from "./data";

export type Tab = "usage" | "calls" | "assist" | "settings";
export type Place = { kind: "quota" } | { kind: "attach"; machine: string } | { kind: "login"; id: string; tab: Tab };
/** How a drawing comes, which says where the reader came from: the first
 *  page; into a login, or back out of one; to the view on the right or on
 *  the left of the one shown, under the same head; or the same thing drawn
 *  again in place. */
export type How = "page" | "forward" | "back" | "right" | "left" | "quiet";

export const state = {
  place: { kind: "quota" } as Place,
  machine: "desktop",
  chart: "days" as "days" | "weeks" | "year" | "hours",
  split: "none" as Split | "none",
  unit: "quota" as "quota" | "tokens",
  page: 1,
  /** Rows to a page of requests. */
  size: 50,
  time: "最近 7 天",
  /** A range of days picked by hand, where `time` names one. */
  range: null as { from: Date; until: Date } | null,
  who: null as string | null,
  model: null as string | null,
  source: null as Source | null,
  outcome: null as Outcome | null,
  /** One turn of a conversation whose requests alone are shown, as a link
   *  to them asked. */
  turn: null as { conversation: string; turn: string } | null,
  opened: null as number | null,
  editing: null as string | null,
  scanning: false,
  /** The login or person a deed just touched, shown lit for a moment. */
  fresh: null as string | null,
  /** The cards of logins shown whole, each a machine and an agent: one of
   *  many logins shows its first few until asked for the rest. */
  whole: new Set<string>(),
};

const tabs: Tab[] = ["usage", "calls", "assist", "settings"];

/** Where a place is reached: the specimen's own address, or the app's
 *  (`page.ts`). */
let linker = (place: Place): string => (place.kind === "quota" ? "#/quota" : place.kind === "attach" ? `#/attach/${place.machine}` : `#/login/${place.id}/${place.tab}`);
export const linkOf = (place: Place): string => linker(place);
export function setLinker(how: (place: Place) => string): void {
  linker = how;
}
/** Goes to `place`, as a press of a link to it does. */
export function goTo(place: Place): void {
  location.hash = linkOf(place);
}

export function placeOf(hash: string): Place {
  const [, kind, first, second] = hash.split("/");
  if (kind === "attach") return { kind: "attach", machine: first || state.machine };
  if (kind === "login" && first) return { kind: "login", id: first, tab: tabs.includes(second as Tab) ? (second as Tab) : "usage" };
  return { kind: "quota" };
}

type Act = (element: HTMLElement, event: MouseEvent) => void;
const acts = new Map<string, Act>();
/** Names what a press on `data-act="name"` does. */
export function on(name: string, act: Act): void {
  acts.set(name, act);
}
export const actOf = (name: string): Act | undefined => acts.get(name);

let drawer: (how: How) => void = () => {};
export function drawnBy(how: (how: How) => void): void {
  drawer = how;
}
export function draw(how: How = "quiet"): void {
  drawer(how);
}

/** A word said at the page's foot for a moment: what a deed the specimen
 *  does not carry out would have done. */
export function say(text: string): void {
  if (deeds) deeds.say(text);
  else toast(text);
}

/** The word itself, at the foot of the window. */
export function toast(text: string): void {
  document.querySelector(".lg-said")?.remove();
  const said = document.createElement("div");
  said.className = "lg-said";
  said.textContent = text;
  document.body.append(said);
  window.setTimeout(() => {
    said.classList.add("leaving");
    window.setTimeout(() => said.remove(), 300);
  }, 2200);
}
