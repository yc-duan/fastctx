// A conversation carried from a list by the pointer, and the places that take
// it (`world-gui.md`, 侧栏 and 侧面板): pressed on its row and moved a few
// pixels, a small card with its name follows the pointer; each place says
// whether it would take it where the pointer is and shows so, and takes it
// when it is let go there. Escape, or letting go where nothing takes it, puts
// it back. It is driven by pointer events, not the browser's drag and drop:
// on Windows the window's own handling of dropped files takes every drag over
// the page, and anything but a file shows as not allowed.

import "./talk-drag.css";

/** A conversation carried, by its address, from its row. */
export interface Carried {
  resident: string;
  machine: string;
  row: HTMLElement;
}

/** A place that may take a conversation carried over it. */
export interface DropPlace {
  /** Whether it would take `carried` let go at (x, y), over `under`: when
   *  it would, it shows where, and when not, nothing. */
  over(carried: Carried, x: number, y: number, under: Element | null): boolean;
  /** Shows nothing any more. */
  out(): void;
  take(carried: Carried, x: number, y: number, under: Element | null): void;
}

const places = new Set<DropPlace>();

/** Lets `place` take what is carried, until the returned function is called. */
export function placeForDrops(place: DropPlace): () => void {
  places.add(place);
  return () => {
    place.out();
    places.delete(place);
  };
}

/** How far the pointer moves, in pixels, before a press becomes carrying. */
const BEGIN = 5;

export interface CarryOptions {
  /** The conversation a pressed row stands for, when it can be carried. */
  carried(row: HTMLElement): Carried | null;
  /** Its name, for the card that follows the pointer. */
  name(carried: Carried): string;
  began(carried: Carried): void;
  ended(): void;
}

/** Lets the rows of `list` that match `rows` be carried. */
export function carryRows(list: HTMLElement, rows: string, options: CarryOptions): void {
  // The browser's own dragging of a link is not used.
  list.addEventListener("dragstart", (event) => {
    if ((event.target as Element | null)?.closest(rows)) event.preventDefault();
  });
  list.addEventListener("pointerdown", (event) => {
    if (event.button !== 0 || event.pointerType === "touch") return;
    const row = (event.target as Element | null)?.closest<HTMLElement>(rows);
    // Pressing a row's own controls, or its name being edited, carries nothing.
    if (!row || (event.target as Element).closest("button, input, textarea")) return;
    const carried = options.carried(row);
    if (carried) follow(event, list, carried, options);
  });
}

function follow(start: PointerEvent, list: HTMLElement, carried: Carried, options: CarryOptions): void {
  const stop = new AbortController();
  const listening = { signal: stop.signal };
  let card: HTMLElement | null = null;
  let at: DropPlace | null = null;

  const look = (x: number, y: number) => {
    const under = document.elementFromPoint(x, y);
    at = null;
    for (const place of places) {
      if (at) place.out();
      else if (place.over(carried, x, y, under)) at = place;
    }
    // Where nothing would take it, the card says so by stepping back.
    card?.classList.toggle("td-nowhere", !at);
  };
  const end = (taken: { x: number; y: number } | null) => {
    stop.abort();
    if (!card) return;
    if (taken && at) at.take(carried, taken.x, taken.y, document.elementFromPoint(taken.x, taken.y));
    for (const place of places) place.out();
    card.remove();
    document.documentElement.classList.remove("td-carrying");
    if (list.hasPointerCapture(start.pointerId)) list.releasePointerCapture(start.pointerId);
    options.ended();
    // The click that letting go makes opens nothing.
    const swallow = (event: MouseEvent) => {
      event.preventDefault();
      event.stopPropagation();
    };
    window.addEventListener("click", swallow, { capture: true, once: true });
    setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
  };

  window.addEventListener(
    "pointermove",
    (event) => {
      if (event.pointerId !== start.pointerId) return;
      if (!card) {
        if (Math.hypot(event.clientX - start.clientX, event.clientY - start.clientY) < BEGIN) return;
        card = document.createElement("div");
        card.className = "td-card";
        card.textContent = options.name(carried);
        document.body.append(card);
        document.documentElement.classList.add("td-carrying");
        try {
          list.setPointerCapture(start.pointerId);
        } catch {
          // A pointer already gone is followed on the window as it is.
        }
        options.began(carried);
      }
      card.style.left = `${event.clientX}px`;
      card.style.top = `${event.clientY}px`;
      look(event.clientX, event.clientY);
    },
    listening,
  );
  window.addEventListener(
    "pointerup",
    (event) => {
      if (event.pointerId === start.pointerId) end({ x: event.clientX, y: event.clientY });
    },
    listening,
  );
  window.addEventListener(
    "pointercancel",
    (event) => {
      if (event.pointerId === start.pointerId) end(null);
    },
    listening,
  );
  window.addEventListener(
    "keydown",
    (event) => {
      if (event.key !== "Escape" || !card) return;
      event.preventDefault();
      end(null);
    },
    listening,
  );
}
