// The desk's conversations moving from where they stood to where they stand
// now (`desk.ts`), over one glide, as those beside the side give way to it
// (`side.css`): each by its place and size. One coming in opens from nothing
// where its room comes from, and one going folds to nothing where its room
// goes; their pages keep the size they have where they stand, riding in or
// out on the edge that moves and fading as they go, so nothing in them is
// set again on the way. A head that takes the corner over, or gives it up,
// makes the room or takes it back as they go. Only where every conversation
// keeps its row: where one changes rows they would pass over one another,
// and they take their places at once. By layout, not by a transform, so the
// words stay sharp on Windows.

import { bezier } from "../conversation/flight";
import { msOf, reducedMotion } from "../motion";

/** Where a pane stands in the desk's box, and whether its head keeps clear
 *  of the corner. */
export interface Place {
  left: number;
  top: number;
  width: number;
  height: number;
  corner: boolean;
}

export type Places = Map<HTMLElement, Place>;

/** The desk's rows, each pane in its order; none where only the one in
 *  focus is drawn. */
export type Rows = HTMLElement[][] | null;

export interface Change {
  /** Where the panes stood, and in what rows. */
  from: Places;
  before: Rows;
  /** The rows they stand in now. */
  after: Rows;
  /** The one opened, standing now. */
  coming?: HTMLElement;
  /** The one closed, no longer among those standing; put away by `gone`
   *  once it has folded. */
  going?: HTMLElement;
  gone?: () => void;
}

/** The edges of its pane a page coming or going rides on. */
interface Hold {
  x: "left" | "right";
  y: "top" | "bottom";
}

export class Morph {
  /** Ends the move under way. */
  private ending: (() => void) | null = null;

  constructor(private readonly box: HTMLElement) {}

  /** Where each of `panes` stands now. */
  places(panes: Iterable<HTMLElement>): Places {
    const box = this.box.getBoundingClientRect();
    const places: Places = new Map();
    for (const pane of panes) {
      const rect = pane.getBoundingClientRect();
      places.set(pane, { left: rect.left - box.left, top: rect.top - box.top, width: rect.width, height: rect.height, corner: pane.classList.contains("dk-corner") });
    }
    return places;
  }

  /** Puts every pane of the move under way where it ends, at once. */
  finish(): void {
    const ending = this.ending;
    this.ending = null;
    ending?.();
  }

  /** Moves the panes from where they stood to where they stand now. */
  run(change: Change): void {
    this.finish();
    const { from, coming, going } = change;
    const gone = () => change.gone?.();
    const style = getComputedStyle(this.box);
    const ms = msOf(style.getPropertyValue("--glide"));
    const leftFrom = going ? from.get(going) : undefined;
    if (reducedMotion() || ms <= 0 || this.box.clientWidth === 0 || !kept(change) || (going && !leftFrom)) {
      gone();
      return;
    }
    const before = change.before!;
    const after = change.after!;
    // The one going leaves the flow where it stood, so the others' places
    // now can be read.
    if (going && leftFrom) {
      going.classList.add("dk-going");
      put(going, leftFrom);
    }
    const to = this.places(after.flat());
    const staying = after.flat().filter((one) => one !== coming);
    const turning = staying.filter((one) => from.get(one)!.corner !== to.get(one)!.corner);
    const out = going && leftFrom ? { pane: going, at: leftFrom, ...fold(going, before, leftFrom, to) } : null;
    const comingTo = coming ? to.get(coming) : undefined;
    const into = coming && comingTo ? { pane: coming, at: comingTo, ...fold(coming, after, comingTo, from) } : null;

    this.box.classList.add("dk-morphing");
    for (const one of turning) one.classList.add("dk-turning");
    // The very heads and pages held, let go of at the end though a pane
    // shows another page by then: a page is kept a while to be shown again.
    const heads = turning.map((one) => ({ head: head(one), corner: to.get(one)!.corner }));
    const held: HTMLElement[] = [];
    if (out) held.push(...pin(out.pane, out.at, out.hold));
    if (into) {
      into.pane.classList.add("dk-coming");
      held.push(...pin(into.pane, into.at, into.hold));
    }
    const draw = (at: number) => {
      for (const one of staying) put(one, between(from.get(one)!, to.get(one)!, at));
      for (const one of heads) one.head?.style.setProperty("--dk-corner", String(one.corner ? at : 1 - at));
      if (out) {
        put(out.pane, between(out.at, out.place, at));
        out.pane.style.setProperty("--dk-shown", String(1 - at));
      }
      if (into) {
        put(into.pane, between(into.place, into.at, at));
        into.pane.style.setProperty("--dk-shown", String(at));
      }
    };
    draw(0);

    const ease = bezier(style.getPropertyValue("--glide-easing"));
    const began = performance.now();
    let frame = 0;
    this.ending = () => {
      cancelAnimationFrame(frame);
      this.box.classList.remove("dk-morphing");
      for (const one of heads) one.head?.style.removeProperty("--dk-corner");
      for (const one of held) unpin(one);
      for (const one of staying) settle(one);
      if (into) settle(into.pane);
      if (out) {
        settle(out.pane);
        gone();
      }
    };
    const step = (now: number) => {
      const t = Math.min(1, (now - began) / ms);
      draw(ease(t));
      if (t < 1) frame = requestAnimationFrame(step);
      else this.finish();
    };
    frame = requestAnimationFrame(step);
  }
}

/** Whether every pane but the one coming or going keeps its row, and one
 *  at least stays. */
function kept({ before, after, coming, going }: Change): boolean {
  if (!before || !after) return false;
  const others = (rows: HTMLElement[][]) => rows.map((row) => row.filter((one) => one !== coming && one !== going)).filter((row) => row.length > 0);
  const was = others(before);
  const now = others(after);
  return was.length > 0 && was.length === now.length && was.every((row, i) => row.length === now[i].length && row.every((one, j) => one === now[i][j]));
}

/** Where `pane`, standing `at` in `rows`, has no room in the other places,
 *  `others`: at the edge between its neighbours in its row, or where its
 *  row is its alone, at that row's outer edge; and the edges its page rides
 *  on, the ones that move. */
function fold(pane: HTMLElement, rows: HTMLElement[][], at: Place, others: Places): { place: Place; hold: Hold } {
  const r = rows.findIndex((row) => row.includes(pane));
  const row = rows[r] ?? [pane];
  const i = row.indexOf(pane);
  const before = row[i - 1];
  const after = row[i + 1];
  const mate = others.get(before ?? after);
  if (mate) {
    const left = before ? mate.left + mate.width : mate.left;
    return { place: { left, top: mate.top, width: 0, height: mate.height, corner: at.corner }, hold: { x: i === 0 ? "right" : "left", y: "top" } };
  }
  const top = r === 0;
  return { place: { left: at.left, top: top ? at.top : at.top + at.height, width: at.width, height: 0, corner: at.corner }, hold: { x: "left", y: top ? "bottom" : "top" } };
}

function between(from: Place, to: Place, at: number): Place {
  const mix = (a: number, b: number) => a + (b - a) * at;
  return { left: mix(from.left, to.left), top: mix(from.top, to.top), width: mix(from.width, to.width), height: mix(from.height, to.height), corner: to.corner };
}

function put(pane: HTMLElement, place: Place): void {
  pane.style.left = `${place.left}px`;
  pane.style.top = `${place.top}px`;
  pane.style.width = `${place.width}px`;
  pane.style.height = `${place.height}px`;
}

function page(pane: HTMLElement): HTMLElement | null {
  return pane.firstElementChild instanceof HTMLElement ? pane.firstElementChild : null;
}

function head(pane: HTMLElement): HTMLElement | null {
  return page(pane)?.querySelector<HTMLElement>(":scope > .page-head") ?? null;
}

/** Holds a pane's page at the size it has `at`, against the edges it rides
 *  on; the page held, if any. */
function pin(pane: HTMLElement, at: Place, hold: Hold): HTMLElement[] {
  const held = page(pane);
  if (!held) return [];
  held.classList.add("dk-pinned");
  held.style.width = `${at.width}px`;
  held.style.height = `${at.height}px`;
  held.style[hold.x] = "0";
  held.style[hold.y] = "0";
  return [held];
}

function unpin(held: HTMLElement): void {
  held.classList.remove("dk-pinned");
  for (const property of ["width", "height", "left", "right", "top", "bottom"]) held.style.removeProperty(property);
}

/** Gives a pane back to the desk's own layout. */
function settle(pane: HTMLElement): void {
  pane.classList.remove("dk-going", "dk-coming", "dk-turning");
  for (const property of ["left", "top", "width", "height", "--dk-shown"]) pane.style.removeProperty(property);
}
