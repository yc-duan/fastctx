// The desk (`world-gui.md`, 分屏): the main area while conversations are
// shown, up to ten of them side by side, each whole with its head, its words
// and its input, set apart by lines ruled by hand; and the side of the one in
// focus. They stand in one row while each keeps room enough, else in two, the
// top row one more where they are odd; where even two rows leave too little,
// as on a phone, only the one in focus is drawn, the others waiting for the
// room to come back or for the list to open them. The one in focus is the one last pressed or typed in:
// the address and the sidebar follow it. A conversation opened plainly takes
// the place of the one in focus; one opened side by side, from its menu, by a
// press with Ctrl or the middle button, or dragged in from the list to an
// edge of one shown, takes a place of its own, and with ten shown the one in
// focus longest ago gives way. Each pane's head has its cross; those shown
// need no list of their own. A fork opens beside the conversation it is
// made from, a card of where it came from carried over to its head. Which
// are shown, in their order, is kept on this device. A phone is not a desk:
// there one conversation is the page, another opened or a fork made is gone
// to, and back comes back; a window as narrow keeps those shown aside until
// it widens again.

import type { Page } from "../app/shell";
import { placeForDrops, type Carried } from "../app/talk-drag";
import { forkCard, type ForkFrom } from "../conversation/fork-card";
import { glyph } from "../conversation/glyphs";
import type { KeepablePage } from "../conversation/kept";
import type { ConversationPage } from "../conversation/page";
import { escape } from "../conversation/prose";
import { msOf, reducedMotion } from "../motion";
import { OVER, SideFrame } from "../side/frame";
import type { SideWords } from "../side/words";
import type { Address } from "../world/wire";
import { fly } from "./flight";
import type { DeskWords } from "./words";
import "./desk.css";

/** The most conversations the desk shows at once. */
export const MOST = 10;
/** The narrowest a conversation is drawn in one row, and in two. */
const ONE_ROW = 320;
const TWO_ROWS = 240;
/** Which are shown and which is in focus, kept on this device. */
const KEPT = "desk";

/** A conversation's page as the desk holds it (`main.ts`). */
export interface DeskTalk extends KeepablePage {
  readonly page: ConversationPage;
}

export interface DeskSeams {
  words: DeskWords;
  side: SideWords;
  /** The page of a conversation, kept alive or made (`kept.ts`). */
  take(address: Address): DeskTalk;
  /** A page no longer shown: kept a while, then let go. */
  release(address: Address): void;
  /** The conversation in focus changed by the user's hand on the desk. */
  focused(address: Address): void;
  /** Goes to a conversation as a page of its own, as a phone does. */
  go(address: Address): void;
  /** How the list names a conversation, and its family. */
  named(address: Address): { name: string; agent: "claude" | "codex" };
}

interface Pane {
  key: string;
  address: Address | null;
  talk: DeskTalk | null;
  readonly element: HTMLElement;
  /** A fork on its way: the key of the conversation it is made from, where
   *  in it, and the line that says how it goes. */
  fork: { source: string; origin: ForkFrom; status: HTMLElement } | null;
}

/** Where a conversation carried over the desk would go. */
interface Spot {
  pane: Pane;
  side: "before" | "after" | "in";
}

const keyOf = (address: Address) => `${address.resident}@${address.machine}`;

/** What this device kept of the desk, as it was written. */
function keptDesk(): { panes?: unknown; focused?: unknown } | null {
  try {
    const kept: unknown = JSON.parse(localStorage.getItem(KEPT) ?? "null");
    return kept && typeof kept === "object" ? (kept as { panes?: unknown; focused?: unknown }) : null;
  } catch {
    return null;
  }
}

function addressOf(key: string): Address | null {
  const at = key.lastIndexOf("@");
  return at > 0 ? { resident: key.slice(0, at), machine: key.slice(at + 1) } : null;
}

export class Desk implements Page {
  readonly element: HTMLElement;
  private readonly box: HTMLElement;
  private readonly drop: HTMLElement;
  private readonly frame: SideFrame;
  private panes: Pane[] = [];
  private focusKey: string | null = null;
  /** Keys in the order they were last in focus, the latest at the end. */
  private used: string[] = [];
  /** Another page is shown meanwhile. */
  private away = false;
  /** What this device kept is put back the first time the desk is shown. */
  private restored = false;
  private forks = 0;
  /** Only the one in focus is drawn. */
  private single = false;

  constructor(private readonly seams: DeskSeams) {
    this.element = document.createElement("div");
    this.element.className = "desk";
    this.box = document.createElement("div");
    this.box.className = "desk-panes";
    this.frame = new SideFrame(this.element, this.box, seams.side, { beside: (address) => this.open(address) });
    this.drop = document.createElement("div");
    this.drop.className = "desk-drop";
    this.drop.setAttribute("aria-hidden", "true");
    this.drop.innerHTML = `<span class="desk-drop-words"><span class="desk-drop-mark"></span><span class="desk-drop-name"></span></span>`;
    this.element.append(this.drop);

    // The one pressed or typed in is the one in focus.
    const toFocus = (event: Event) => {
      const pane = this.paneOf(event.target as Element | null);
      if (pane && pane.key !== this.focusKey && !pane.fork) this.focus(pane, true);
    };
    this.box.addEventListener("pointerdown", toFocus, true);
    this.box.addEventListener("focusin", toFocus);
    this.box.addEventListener("click", (event) => {
      const target = event.target as Element;
      const pane = this.paneOf(target);
      if (!pane) return;
      if (target.closest(".dk-close")) this.close(pane);
      else if (target.closest(".dk-forking .fk-card") && pane.fork) {
        const source = this.panes.find((one) => one.key === pane.fork!.source);
        if (source) this.focus(source, true);
      }
    });
    new ResizeObserver(() => this.lay()).observe(this.box);
    OVER.addEventListener("change", () => this.lay());
    this.dropping();
  }

  get title(): string {
    return this.current()?.talk?.title ?? "";
  }

  /** Shows `address`: in focus where it is on the desk, else in the place
   *  of the one in focus. */
  show(address: Address): void {
    this.wake();
    let pane = this.find(keyOf(address));
    if (!pane) {
      const focused = this.current();
      if (focused && !focused.fork) {
        this.replace(focused, address);
        pane = focused;
      } else pane = this.insert(address, focused ? this.panes.indexOf(focused) + 1 : this.panes.length, false);
    }
    this.focus(pane, false);
  }

  /** Opens `address` in a place of its own: before or after one shown, or
   *  after the one in focus. Where it is shown already it is put there, or
   *  only comes into focus. With ten shown, the one in focus longest ago
   *  gives way. */
  open(address: Address, where?: { key: string; side: "before" | "after" }): void {
    if (OVER.matches) {
      this.seams.go(address);
      return;
    }
    this.wake();
    const key = keyOf(address);
    const there = this.find(key);
    if (there && where) this.move(there, where);
    if (there) {
      this.focus(there, true);
      return;
    }
    if (this.panes.length >= MOST) {
      const oldest = this.oldest();
      if (!oldest) return;
      this.replace(oldest, address);
      this.focus(oldest, true);
      return;
    }
    const anchor = where ? this.find(where.key) : this.current();
    const index = anchor ? this.panes.indexOf(anchor) + (where?.side === "before" ? 0 : 1) : this.panes.length;
    this.focus(this.insert(address, index, true), true);
  }

  /** Brings a turn of a conversation shown into view, once it is drawn. */
  reach(address: Address, turn: string): void {
    const talk = this.find(keyOf(address))?.talk;
    if (talk?.reach) queueMicrotask(() => talk.reach!(turn));
  }

  /** The page of `address` was let go to be made anew, as when the World
   *  first answered for it: where it is shown, the new one takes its place. */
  renew(address: Address): void {
    const pane = this.find(keyOf(address));
    if (!pane) return;
    this.fill(pane, address);
    if (pane.key === this.focusKey) this.focus(pane, false);
  }

  /** Goes from a fork to the conversation it came from, at the turn it was
   *  made from: in focus where it is shown, else opened just before the
   *  fork. */
  toSource(fork: Address, source: Address, turn: string | null): void {
    const there = this.find(keyOf(source));
    if (there) this.focus(there, true);
    else this.open(source, this.find(keyOf(fork)) ? { key: keyOf(fork), side: "before" } : undefined);
    if (turn) this.reach(source, turn);
  }

  /** A fork is being made of `source` from `turn`, or from where it is now:
   *  its place opens beside the conversation, the card of where it came from
   *  lifting off that turn and carried over to the place's head. */
  forking(source: Address, turn: string | null): void {
    // On a phone the fork is gone to once it is made.
    if (OVER.matches) return;
    this.wake();
    const sourceKey = keyOf(source);
    const from = this.find(sourceKey);
    if (!from) return;
    if (this.panes.length >= MOST) {
      const oldest = this.oldest(from);
      if (oldest) this.remove(oldest, false);
    }
    const words = this.seams.words;
    const named = this.seams.named(source);
    const at = from.talk?.page.forkFrom(turn) ?? null;
    const origin: ForkFrom = { name: named.name, agent: named.agent, said: at?.said ?? null, answer: at?.answer ?? null };
    const element = document.createElement("div");
    element.className = "desk-pane dk-forking";
    element.dataset.key = `fork:${(this.forks += 1)}`;
    element.innerHTML = `<div class="dk-fork-page"><div class="page-head dk-fork-head"><div class="who"><h1 class="who-name">${escape(named.name)}</h1></div><div class="page-head-actions">${this.closer().outerHTML}</div></div><div class="dk-fork-body">${forkCard(origin, words.backTo(named.name))}<p class="dk-fork-status" role="status">${escape(words.forking)}</p></div></div>`;
    const pane: Pane = { key: element.dataset.key, address: null, talk: null, element, fork: { source: sourceKey, origin, status: element.querySelector(".dk-fork-status")! } };
    this.panes.splice(this.panes.indexOf(from) + 1, 0, pane);
    this.box.append(element);
    this.grow(pane);
    this.lay();
    fly(at?.element ?? null, element.querySelector<HTMLElement>(".fk-card")!);
  }

  /** The fork of `source` is made: its page takes the place opened for it,
   *  in focus. */
  forked(source: Address, made: Address): void {
    const sourceKey = keyOf(source);
    const pane = this.panes.find((one) => one.fork?.source === sourceKey);
    if (!pane || OVER.matches) {
      if (pane) this.remove(pane, false);
      this.open(made, this.find(sourceKey) ? { key: sourceKey, side: "after" } : undefined);
      return;
    }
    const origin = pane.fork!.origin;
    this.fill(pane, made);
    // Where it came from, as the desk saw it, for a fork whose own record
    // does not carry the words it was made from.
    pane.talk?.page.forkSeed(origin);
    this.focus(pane, true);
  }

  /** The fork of `source` waits for the turn under way to end. */
  forkQueued(source: Address): void {
    const pane = this.panes.find((one) => one.fork?.source === keyOf(source));
    if (pane?.fork) pane.fork.status.textContent = this.seams.words.forkWaits;
  }

  /** The fork of `source` was not made: its place closes, and the line
   *  beside the conversation's input says why. */
  forkFailed(source: Address): void {
    const pane = this.panes.find((one) => one.fork?.source === keyOf(source));
    if (pane) this.close(pane);
  }

  /** Lets go of every conversation on it, as when an account is logged out
   *  of this device: none is put back next time. */
  forget(): void {
    for (const pane of this.panes.splice(0)) {
      pane.element.remove();
      if (pane.address && pane.talk) this.seams.release(pane.address);
    }
    this.used = [];
    this.focusKey = null;
    this.frame.use(null);
    this.lay();
    this.keep();
  }

  /** Another page is shown: those on the desk wait, still alive. */
  leave(): void {
    this.away = true;
    for (const pane of this.panes) pane.talk?.suspend();
  }

  /** The desk is shown again, and the first time with what was kept. */
  private wake(): void {
    if (!this.restored) {
      this.restored = true;
      this.restore();
    }
    if (!this.away) return;
    this.away = false;
    for (const pane of this.panes) pane.talk?.resume();
  }

  private current(): Pane | null {
    return this.panes.find((one) => one.key === this.focusKey) ?? null;
  }

  private find(key: string): Pane | null {
    return this.panes.find((one) => one.key === key) ?? null;
  }

  private paneOf(target: Element | null): Pane | null {
    const element = target?.closest<HTMLElement>(".desk-pane");
    return this.panes.find((one) => one.element === element) ?? null;
  }

  /** The one in focus longest ago, but `keep` and the one in focus now. */
  private oldest(keep?: Pane): Pane | null {
    const candidates = this.panes.filter((one) => one !== keep && one.key !== this.focusKey && !one.fork);
    candidates.sort((a, b) => this.used.indexOf(a.key) - this.used.indexOf(b.key));
    return candidates[0] ?? null;
  }

  private insert(address: Address, index: number, grow: boolean): Pane {
    const element = document.createElement("div");
    element.className = "desk-pane";
    const pane: Pane = { key: keyOf(address), address, talk: null, element, fork: null };
    this.fill(pane, address);
    this.panes.splice(index, 0, pane);
    this.box.append(element);
    if (grow) this.grow(pane);
    this.lay();
    return pane;
  }

  /** Puts the page of `address` in `pane`, with the desk's own buttons in
   *  its head. */
  private fill(pane: Pane, address: Address): void {
    const talk = this.seams.take(address);
    pane.talk = talk;
    pane.address = address;
    pane.key = keyOf(address);
    pane.fork = null;
    pane.element.classList.remove("dk-forking");
    pane.element.dataset.key = pane.key;
    pane.element.replaceChildren(talk.element);
    talk.page.paneTools(this.closer());
    if (this.away) talk.suspend();
  }

  /** `pane` shows `address` instead; the one it showed is let go to be kept
   *  a while. */
  private replace(pane: Pane, address: Address): void {
    const old = pane.address;
    this.used = this.used.filter((key) => key !== pane.key);
    if (old) this.seams.release(old);
    this.fill(pane, address);
    this.keep();
  }

  /** Puts a pane before or after another, by the order it is drawn in: the
   *  page stays where it is in the document, keeping its place in the words. */
  private move(pane: Pane, where: { key: string; side: "before" | "after" }): void {
    const target = this.find(where.key);
    if (!target || target === pane) return;
    this.panes.splice(this.panes.indexOf(pane), 1);
    this.panes.splice(this.panes.indexOf(target) + (where.side === "after" ? 1 : 0), 0, pane);
    this.lay();
    this.keep();
  }

  /** Closes a pane, as its cross asks: the one in focus before it comes
   *  back into focus. The last one shown stays. */
  private close(pane: Pane): void {
    if (this.panes.length <= 1) return;
    this.remove(pane, true);
  }

  private remove(pane: Pane, gently: boolean): void {
    const index = this.panes.indexOf(pane);
    if (index < 0) return;
    this.panes.splice(index, 1);
    this.used = this.used.filter((key) => key !== pane.key);
    const gone = () => {
      pane.element.remove();
      if (pane.address && pane.talk) this.seams.release(pane.address);
    };
    if (gently) this.shrink(pane, gone);
    else gone();
    if (this.focusKey === pane.key) {
      const back = [...this.used].reverse().map((key) => this.find(key)).find((one): one is Pane => !!one && !one.fork) ?? this.panes[Math.min(index, this.panes.length - 1)] ?? null;
      this.focusKey = null;
      if (back) this.focus(back, true);
    }
    this.lay();
    this.keep();
  }

  /** `pane` in focus: its words are the ones the keys go to, the side shows
   *  its parts, and, where the user moved it, the address follows. */
  private focus(pane: Pane, byHand: boolean): void {
    const changed = this.focusKey !== pane.key;
    this.focusKey = pane.key;
    this.used = [...this.used.filter((key) => key !== pane.key), pane.key];
    for (const one of this.panes) {
      const focused = one === pane;
      one.element.classList.toggle("focused", focused);
      one.talk?.page.setActive(focused);
    }
    this.frame.use(pane.talk?.page.sideParts ?? null);
    if (changed) this.lay();
    this.keep();
    if (byHand && changed && pane.address) this.seams.focused(pane.address);
  }

  /** The panes laid out for the room the desk has: one row, two, or only
   *  the one in focus; each in its order and with its share of its row, a
   *  line ruled before each but the first of its row and across the top of
   *  the second, and the top row's last keeping clear of the corner where
   *  the window's own buttons stand. */
  private lay(): void {
    const n = this.panes.length;
    const width = this.box.clientWidth;
    let top = n;
    let single = n > 1 && OVER.matches;
    if (n > 1 && !single && width > 0 && width / n < ONE_ROW) {
      top = Math.ceil(n / 2);
      if (width / top < TWO_ROWS) {
        single = true;
        top = n;
      }
    }
    const two = !single && top < n;
    this.single = single;
    this.element.classList.toggle("desk-single", single);
    this.element.classList.toggle("desk-two", two);
    this.element.classList.toggle("desk-many", n > 1 && !OVER.matches);
    this.panes.forEach((pane, index) => {
      const below = two && index >= top;
      pane.element.style.order = String(index);
      pane.element.style.setProperty("--dk-row", String(single ? 1 : below ? n - top : top));
      pane.element.classList.toggle("dk-after", !single && index !== 0 && index !== top);
      pane.element.classList.toggle("dk-below", below);
      pane.element.classList.toggle("dk-corner", single ? pane.key === this.focusKey : index === top - 1);
    });
  }

  /** A pane coming in grows from nothing, the others giving way to it. */
  private grow(pane: Pane): void {
    if (reducedMotion()) return;
    pane.element.style.setProperty("--dk-grow", "0");
    pane.element.classList.add("dk-moving");
    void pane.element.offsetWidth;
    requestAnimationFrame(() => {
      pane.element.style.removeProperty("--dk-grow");
      this.settled(pane, () => pane.element.classList.remove("dk-moving"));
    });
  }

  /** A pane going shrinks to nothing, then `gone`. */
  private shrink(pane: Pane, gone: () => void): void {
    if (reducedMotion()) return gone();
    pane.element.classList.add("dk-moving", "dk-leaving");
    pane.element.style.setProperty("--dk-grow", "0");
    this.settled(pane, gone);
  }

  /** Calls `done` once the pane's size has stopped moving. */
  private settled(pane: Pane, done: () => void): void {
    let finished = false;
    const finish = () => {
      if (finished) return;
      finished = true;
      pane.element.removeEventListener("transitionend", ended);
      done();
    };
    const ended = (event: TransitionEvent) => {
      if (event.target === pane.element && event.propertyName === "--dk-grow") finish();
    };
    pane.element.addEventListener("transitionend", ended);
    window.setTimeout(finish, msOf(getComputedStyle(pane.element).getPropertyValue("--glide")) + 120);
  }

  /** The cross that closes a pane. */
  private closer(): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "icon-button dk-close";
    button.setAttribute("aria-label", this.seams.words.close);
    button.title = this.seams.words.close;
    button.innerHTML = glyph("close", 18);
    return button;
  }

  /** A conversation carried in from the list: to an edge of one shown it
   *  takes a place of its own on that side, onto the middle it takes that
   *  one's place; with ten shown, or only one drawn, it takes the place of
   *  the one it is let go over. Carried onto itself, or beside itself where
   *  it already stands, nothing takes it. */
  private dropping(): void {
    placeForDrops({
      over: (carried, x, y, under) => {
        const spot = this.spotAt(carried, x, y, under);
        if (!spot) {
          this.hideDrop();
          return false;
        }
        this.showDrop(carried, spot);
        return true;
      },
      out: () => this.hideDrop(),
      take: (carried, x, y, under) => {
        const spot = this.spotAt(carried, x, y, under);
        this.hideDrop();
        if (!spot) return;
        const address = { resident: carried.resident, machine: carried.machine };
        if (spot.side === "in") {
          const there = this.find(keyOf(address));
          if (there) this.remove(there, false);
          this.replace(spot.pane, address);
          this.focus(spot.pane, true);
        } else this.open(address, { key: spot.pane.key, side: spot.side });
      },
    });
  }

  private spotAt(carried: Carried, x: number, _y: number, under: Element | null): Spot | null {
    if (this.away || !this.element.isConnected || OVER.matches || !under || !this.box.contains(under)) return null;
    const pane = this.paneOf(under);
    if (!pane || pane.fork) return null;
    const key = keyOf(carried);
    const there = this.find(key);
    const box = pane.element.getBoundingClientRect();
    const across = (x - box.left) / Math.max(1, box.width);
    const crowded = this.single || (this.panes.length >= MOST && !there);
    const side: Spot["side"] = crowded ? "in" : across < 0.3 ? "before" : across > 0.7 ? "after" : "in";
    if (there === pane) return null;
    if (there) {
      const at = this.panes.indexOf(pane);
      const from = this.panes.indexOf(there);
      if ((side === "before" && from === at - 1) || (side === "after" && from === at + 1)) return null;
    }
    return { pane, side };
  }

  /** Where a conversation let go would stand: the half of the pane it would
   *  open in, or the whole pane it would take, washed in clay with its name. */
  private showDrop(carried: Carried, spot: Spot): void {
    const desk = this.element.getBoundingClientRect();
    const box = spot.pane.element.getBoundingClientRect();
    const inset = 8;
    const half = spot.side === "in" ? box.width : box.width / 2;
    const left = spot.side === "after" ? box.right - half : box.left;
    this.drop.style.left = `${left - desk.left + inset}px`;
    this.drop.style.top = `${box.top - desk.top + inset}px`;
    this.drop.style.width = `${Math.max(0, half - 2 * inset)}px`;
    this.drop.style.height = `${Math.max(0, box.height - 2 * inset)}px`;
    const named = this.seams.named({ resident: carried.resident, machine: carried.machine });
    this.drop.querySelector(".desk-drop-mark")!.innerHTML = glyph(named.agent, 16);
    this.drop.querySelector(".desk-drop-name")!.textContent = named.name;
    this.element.classList.add("dk-dropping");
  }

  private hideDrop(): void {
    this.element.classList.remove("dk-dropping");
  }

  private keep(): void {
    const panes = this.panes.filter((one) => one.address).map((one) => one.key);
    try {
      localStorage.setItem(KEPT, JSON.stringify({ panes, focused: this.focusKey }));
    } catch {
      // Not kept: the desk opens with one conversation next time.
    }
  }

  /** The conversations this device last had side by side, put back. */
  private restore(): void {
    const kept = keptDesk();
    const keys = Array.isArray(kept?.panes) ? kept.panes.filter((key): key is string => typeof key === "string").slice(0, MOST) : [];
    for (const key of keys) {
      const address = addressOf(key);
      if (address && !this.find(key)) this.insert(address, this.panes.length, false);
    }
    const focused = typeof kept?.focused === "string" ? this.find(kept.focused) : null;
    if (focused) this.focus(focused, false);
  }
}
