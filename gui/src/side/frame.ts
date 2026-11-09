// The side of the desk (`world-gui.md`, 对话的侧面板): one for all the
// conversations shown, holding the parts of the one in focus. It stands at
// the conversations' right as tall as the window by default, or at their
// left, or along their foot as wide as the desk; as wide as its cards read
// well at first, or half the desk's height along its foot, and dragged
// larger or smaller by the line between them. Where it stands, how
// much it takes and whether it is open are kept on this device; its own menu
// moves it. Where the desk has no room for it there, it stands along the
// other edge, or lies over the whole desk as on a phone, until the room comes
// back. The window's top right corner holds one line, before the window's
// own buttons where they are drawn: where the conversation in focus works,
// the side's "⋯" and its switch. It stays there as the side opens and folds
// away, and is all that is left of the side while it is folded.

import { makeGrip } from "../app/grip";
import { glyph } from "../conversation/glyphs";
import type { Address } from "../world/wire";
import { SidePane, type Dock, type Peek } from "./pane";
import type { PartsFrame, SideParts } from "./parts";
import type { SideWords } from "./words";

/** Narrower than this, the side lies over the desk. */
export const OVER = matchMedia("(max-width: 760px)");

/** The share of the desk the side was dragged to, kept on this device. */
const SHARE_KEPT = "side-share";
/** Its size at first: across, a share of the desk kept between a reading
 *  width's bounds; down, half the desk. */
const FIRST = { share: 0.3, least: 360, most: 440, down: 0.5 };
/** The least the side may be, and the least it leaves the conversations,
 *  across and down. */
const LEAST = { across: 320, down: 180 };
const ROOM = { across: 360, down: 220 };

/** Where the side stands and whether it is open, kept on this device. */
const DOCK_KEPT = "side-dock";
const OPEN_KEPT = "side-open";
const DOCKS: readonly Dock[] = ["right", "bottom", "left"];
type DeskSize = { width: number; height: number };

function kept(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function keep(key: string, value: string | null): void {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // Not kept: it holds while the window is open.
  }
}

function keptDock(): Dock {
  const dock = kept(DOCK_KEPT);
  return dock === "bottom" || dock === "left" ? dock : "right";
}

function keptShare(): number | null {
  const share = Number(kept(SHARE_KEPT));
  return Number.isFinite(share) && share > 0 && share < 1 ? share : null;
}

export class SideFrame implements PartsFrame {
  readonly pane: SidePane;
  readonly opener: HTMLButtonElement;
  /** Opens the side at the history of the conversation in focus. */
  private readonly historyMark: HTMLButtonElement;
  /** The corner's line, and where in it the conversation's place goes. */
  private readonly corner: HTMLElement;
  private readonly whereSlot: HTMLElement;
  /** The conversations and the side, the two columns of the desk. */
  readonly split: HTMLElement;
  private parts: SideParts | null = null;
  /** Whether it was put back open as this device left it. */
  private reopened = false;
  private dock: Dock = keptDock();
  /** Where the side stands for the room the desk has now. */
  private placed: Dock | "over" = this.dock;
  private readonly fitting: ResizeObserver;

  constructor(
    /** The desk, whose size the side's own follows. */
    private readonly host: HTMLElement,
    main: HTMLElement,
    words: SideWords,
    private readonly seams: {
      /** Another conversation, opened beside the one in focus. */
      beside(address: Address): void;
    },
  ) {
    this.pane = new SidePane(words, {
      changed: () => {
        this.drawSwitch();
        this.parts?.seen(this.pane.visible);
      },
      dock: (dock) => this.setDock(dock),
    });
    this.split = document.createElement("div");
    this.split.className = "sp-split";
    this.split.append(main, this.pane.element);
    host.append(this.split);
    host.classList.add("sp-host");
    const frame = this;
    this.pane.element.prepend(
      makeGrip("sp-grip", {
        get part() {
          return frame.placed === "over" ? frame.dock : frame.placed;
        },
        label: words.resize,
        width: () => this.width(),
        least: () => this.least(),
        most: () => this.most(this.room()),
        set: (width) => this.resize(width),
        moving: (moving) => host.classList.toggle("sp-resizing", moving),
      }),
    );
    this.opener = document.createElement("button");
    this.opener.type = "button";
    this.opener.className = "icon-button sp-toggle";
    this.opener.setAttribute("aria-label", words.panel);
    this.opener.title = words.panel;
    this.opener.addEventListener("click", () => this.pane.toggle(this.parts?.firstPart()));
    this.corner = document.createElement("div");
    this.corner.className = "sp-corner-bar";
    this.whereSlot = document.createElement("div");
    this.whereSlot.className = "sp-corner-where";
    this.historyMark = document.createElement("button");
    this.historyMark.type = "button";
    this.historyMark.className = "icon-button sp-history-mark";
    this.historyMark.hidden = true;
    this.historyMark.innerHTML = glyph("journal", 18);
    // Pressed again on its history, the side goes back to the overview.
    this.historyMark.addEventListener("click", () => this.pane.turnToHistory());
    this.corner.append(this.whereSlot, this.historyMark, this.pane.more, this.opener);
    // It stands over a head that drags the window: the window is told it is
    // pressed, not dragged from.
    this.corner.dataset.worldTitlebar = "client";
    host.append(this.corner);
    // The heads under the corner keep clear of it, as wide as it is now.
    new ResizeObserver(() => {
      const width = `${Math.ceil(this.corner.getBoundingClientRect().width)}px`;
      if (host.style.getPropertyValue("--sp-corner-width") !== width) host.style.setProperty("--sp-corner-width", width);
    }).observe(this.corner);
    this.place(true);
    this.fitting = new ResizeObserver(([entry]) => {
      if (!entry) return;
      // The split has no border or padding. Match its integer client size
      // without forcing another layout after another observer has written.
      this.place(false, { width: Math.round(entry.contentRect.width), height: Math.round(entry.contentRect.height) });
    });
    this.fitting.observe(this.split);
    OVER.addEventListener("change", this.replace);
    this.drawSwitch();
  }

  /** Turns the side to the parts of the conversation now in focus. */
  use(parts: SideParts | null): void {
    if (parts === this.parts) return;
    if (this.parts) {
      this.parts.frame = null;
      this.parts.seen(null);
    }
    this.parts = parts;
    if (parts) parts.frame = this;
    this.corner.hidden = !parts;
    this.whereSlot.replaceChildren(...(parts ? [parts.where.corner] : []));
    const mark = parts?.history?.label ?? null;
    this.historyMark.hidden = mark === null;
    this.historyMark.title = mark ?? "";
    this.historyMark.setAttribute("aria-label", mark ?? "");
    this.pane.use(parts);
    this.drawSwitch();
    parts?.seen(this.pane.visible);
    if (parts && !this.reopened) {
      this.reopened = true;
      // Open as it was left, once the desk is laid out on the screen: no
      // glide, as if it had never shut.
      requestAnimationFrame(() => {
        this.place();
        if (!kept(OPEN_KEPT) || this.placed === "over" || this.pane.open) return;
        this.pane.show(parts.part ?? parts.firstPart());
        this.host.classList.add("sp-shown");
        this.drawSwitch();
      });
    }
  }

  show(part: string): void {
    this.pane.show(part);
  }

  peek(peek: Peek): void {
    if (!this.pane.open) this.pane.show();
    this.pane.peek(peek);
  }

  restate(): void {
    this.pane.restate();
    this.drawSwitch();
  }

  get lying(): boolean {
    return this.placed === "over";
  }

  beside(address: Address): void {
    this.seams.beside(address);
  }

  /** Opens the side at the part the conversation in focus would open at,
   *  as a phone's "⋯" asks. */
  open(): void {
    this.pane.show(this.parts?.part ?? this.parts?.firstPart());
  }

  leave(): void {
    OVER.removeEventListener("change", this.replace);
    this.fitting.disconnect();
  }

  private readonly replace = () => this.place();

  /** The side laid out where it stands for the room the desk has now:
   *  drawn anew where that changed, and sized for the room either way. */
  private place(first = false, size: DeskSize = { width: this.split.clientWidth, height: this.split.clientHeight }): void {
    const placed = this.placement(this.dock, size);
    if (first || placed !== this.placed) {
      this.placed = placed;
      this.pane.over(placed === "over");
      this.host.classList.toggle("sp-over", placed === "over");
      const shown = placed === "over" ? this.dock : placed;
      this.split.dataset.dock = shown;
      this.pane.setDock(shown);
      this.drawOpener();
    }
    this.pane.fitting(DOCKS.filter((dock) => this.placement(dock, size) === dock));
    this.fit(this.down ? size.height : size.width);
  }

  /** Whether the desk has room for the side and the conversations both,
   *  with the side at `dock`. */
  private fits(dock: Dock, size: DeskSize): boolean {
    return dock === "bottom" ? size.height >= LEAST.down + ROOM.down : size.width >= LEAST.across + ROOM.across;
  }

  /** Where the side stands when put at `dock`: there where it fits, else
   *  along the other edge where that fits, else over the whole desk. */
  private placement(dock: Dock, size: DeskSize): Dock | "over" {
    if (OVER.matches) return "over";
    if (this.fits(dock, size)) return dock;
    const other: Dock = dock === "bottom" ? "right" : "bottom";
    return this.fits(other, size) ? other : "over";
  }

  /** Whether the side stands along the foot, sized by its height. */
  private get down(): boolean {
    return this.placed === "bottom";
  }

  /** The room the two columns share, across or down. */
  private room(): number {
    return this.down ? this.split.clientHeight : this.split.clientWidth;
  }

  /** The side's size now, across or down. */
  private width(): number {
    return parseFloat(getComputedStyle(this.host).getPropertyValue(this.down ? "--side-height" : "--side-width")) || 0;
  }

  private least(): number {
    return this.down ? LEAST.down : LEAST.across;
  }

  /** The most the side may be in `room`, the conversations keeping theirs. */
  private most(room: number): number {
    return Math.max(this.least(), room - (this.down ? ROOM.down : ROOM.across));
  }

  /** The side's size in `room`: at the share it was dragged to, else its
   *  first. */
  private fitted(room: number): number {
    const share = keptShare();
    const wanted = share !== null ? share * room : this.down ? FIRST.down * room : Math.min(FIRST.most, Math.max(FIRST.least, FIRST.share * room));
    return Math.round(Math.min(this.most(room), Math.max(this.least(), wanted)));
  }

  /** The side is put where it was chosen to: kept on this device, and the
   *  desk laid out anew around it. */
  private setDock(dock: Dock): void {
    if (dock === this.dock) return;
    this.dock = dock;
    keep(DOCK_KEPT, dock);
    this.place();
  }

  /** The switch that opens the side, drawn as the place it opens at, with a
   *  dot while something new waits there. */
  private drawOpener(): void {
    const at = this.placed === "over" ? this.dock : this.placed;
    this.opener.innerHTML = `${glyph(`dock-${at}`, 18)}<span class="sp-toggle-dot" aria-hidden="true"></span>`;
  }

  /** Takes a width the side was dragged to, as its share of the desk, or
   *  none to go back to its first. */
  private resize(width: number | null): void {
    keep(SHARE_KEPT, width === null ? null : String(width / Math.max(1, this.room())));
    this.fit();
  }

  /** The side's size for the room the desk has now. */
  private fit(room = this.room()): void {
    if (room <= 0) return;
    const name = this.down ? "--side-height" : "--side-width";
    const value = `${this.fitted(room)}px`;
    if (this.host.style.getPropertyValue(name) !== value) this.host.style.setProperty(name, value);
  }

  /** The switch, and the desk opening or shutting by the side: opening, the
   *  desk is laid out shut first, so the side comes from nothing and pushes
   *  the conversations aside as it comes. Seen open, what was new about
   *  the conversation in focus is no longer new. */
  private drawSwitch(): void {
    const open = this.pane.open;
    const was = this.host.classList.contains("sp-shown");
    if (open && !was) void this.host.offsetWidth;
    this.host.classList.toggle("sp-shown", open);
    this.host.classList.toggle("sp-visible", !this.pane.element.hidden);
    if (open && this.parts) this.parts.fresh = false;
    this.opener.setAttribute("aria-pressed", String(open));
    this.historyMark.setAttribute("aria-pressed", String(!!this.parts?.history && this.pane.visible === this.parts.history.part));
    this.opener.classList.toggle("news", !open && !!this.parts?.fresh);
    if (this.placed !== "over") keep(OPEN_KEPT, open ? "1" : null);
  }
}
