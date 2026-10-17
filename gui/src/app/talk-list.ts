// What a list of conversations does with its rows (`talk-row.ts`) beyond
// following them: a row's "⋯" opens its menu, the right button opens it where
// it was pressed, a finger resting on a row raises it from below, and a row
// is renamed in place (`world-gui.md`, 界面的结构). The sidebar and the page of
// every conversation both list them so.

import { titleOf } from "../world/names";
import type { ResidentNow } from "../world/world";
import { addressOf } from "./store";
import type { TalkActs } from "./talk-acts";
import type { TalkMenu } from "./talk-menu";
import type { TalkMenuWords } from "./talk-menu-words";
import type { RowActs } from "./talk-row";
import { composing } from "../typing";

/** How long a finger rests on a row before its menu rises, as Android
 *  counts a long press. */
const LONG_PRESS_MS = 500;
/** How far a finger may drift and still be resting, in CSS pixels. */
const SLOP = 10;

export interface TalkListOptions {
  acts: TalkActs;
  menu: TalkMenu;
  menuWords: TalkMenuWords;
  /** Whether a conversation's machine is there to be asked now. */
  reachable(r: ResidentNow): boolean;
  /** Draws the list again now, whatever it held. */
  redraw(): void;
}

const keyOf = (r: ResidentNow) => `${r.about.machine}\n${r.about.id}`;

export class TalkList {
  /** The conversations drawn now, by machine and id. */
  private rows = new Map<string, ResidentNow>();
  /** The conversation whose name is a field now, and what it holds. */
  private renaming: { key: string; name: string } | null = null;
  /** A press that became a long press: the click it ends in is swallowed. */
  private pressed = false;

  constructor(
    private readonly list: HTMLElement,
    private readonly options: TalkListOptions,
  ) {
    list.addEventListener(
      "click",
      (event) => {
        if (this.pressed) {
          this.pressed = false;
          event.preventDefault();
          event.stopPropagation();
          return;
        }
        const target = event.target as HTMLElement;
        // Typing a name into a row is not following it.
        if (target.closest(".talk-rename")) {
          event.preventDefault();
          event.stopPropagation();
          return;
        }
        const now = target.closest<HTMLButtonElement>(".talk-quick-act");
        if (now) {
          event.preventDefault();
          event.stopPropagation();
          const r = this.rowOf(now);
          if (!r) return;
          if (now.dataset.quick === "restore") options.acts.restore(r);
          else options.acts.archive(r);
          return;
        }
        const more = target.closest<HTMLButtonElement>(".talk-more");
        if (!more) return;
        event.preventDefault();
        event.stopPropagation();
        const r = this.rowOf(more);
        if (r) options.menu.open(r, { anchor: more, rename: () => this.rename(r) });
      },
      true,
    );
    list.addEventListener("contextmenu", (event) => {
      const r = this.rowOf(event.target as HTMLElement);
      if (!r) return;
      event.preventDefault();
      if (this.pressed || (event as PointerEvent).pointerType === "touch") return;
      options.menu.open(r, { at: { x: event.clientX, y: event.clientY }, rename: () => this.rename(r) });
    });
    this.longPresses();
  }

  /** Takes the conversations about to be drawn. */
  drawing(rows: ResidentNow[]): void {
    this.rows = new Map(rows.map((r) => [keyOf(r), r]));
  }

  /** Whether a name is being typed into a row: the list is kept as it is
   *  until that is done, so nothing typed is lost to a redraw. */
  get held(): boolean {
    return !!this.renaming && !!this.list.querySelector(".talk-rename");
  }

  /** What a row says of what the user did to it. */
  rowActs(r: ResidentNow): RowActs {
    const { acts, menuWords } = this.options;
    return {
      more: menuWords.more,
      archive: menuWords.archive,
      restore: menuWords.restore,
      reachable: this.options.reachable(r),
      deleting: menuWords.deleting,
      isDeleting: acts.deleting(addressOf(r)),
      renaming: this.renaming?.key === keyOf(r) ? this.renaming.name : null,
    };
  }

  /** The conversation a row is of, as drawn. */
  private rowOf(element: HTMLElement): ResidentNow | null {
    const item = element.closest<HTMLElement>(".talk-item[data-resident]");
    return item ? (this.rows.get(`${item.dataset.machine}\n${item.dataset.resident}`) ?? null) : null;
  }

  /** A finger resting on a row raises its menu from below. */
  private longPresses(): void {
    let timer = 0;
    let start: { x: number; y: number } | null = null;
    const cancel = () => {
      clearTimeout(timer);
      start = null;
    };
    this.list.addEventListener(
      "pointerdown",
      (event) => {
        this.pressed = false;
        if (event.pointerType === "mouse") return;
        const r = this.rowOf(event.target as HTMLElement);
        if (!r || (event.target as HTMLElement).closest(".talk-rename")) return;
        start = { x: event.clientX, y: event.clientY };
        timer = window.setTimeout(() => {
          start = null;
          this.pressed = true;
          navigator.vibrate?.(8);
          this.options.menu.open(r, { rename: () => this.rename(r) });
        }, LONG_PRESS_MS);
      },
      { passive: true },
    );
    this.list.addEventListener(
      "pointermove",
      (event) => {
        if (start && Math.hypot(event.clientX - start.x, event.clientY - start.y) > SLOP) cancel();
      },
      { passive: true },
    );
    this.list.addEventListener("pointerup", cancel, { passive: true });
    this.list.addEventListener("pointercancel", cancel, { passive: true });
  }

  /** The row's name becomes a field: Enter or leaving it keeps what is
   *  typed, Escape the name it had. */
  private rename(r: ResidentNow): void {
    const { acts } = this.options;
    const shown = acts.view(r);
    const before = titleOf(shown) ?? "";
    this.renaming = { key: keyOf(shown), name: before };
    this.options.redraw();
    const field = this.list.querySelector<HTMLInputElement>(".talk-rename");
    if (!field) {
      this.renaming = null;
      return;
    }
    field.focus({ preventScroll: true });
    field.select();
    let done = false;
    const finish = (keep: boolean) => {
      if (done) return;
      done = true;
      const name = field.value.trim();
      this.renaming = null;
      if (keep && name && name !== before) acts.rename(r, name);
      this.options.redraw();
    };
    field.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !composing(event)) {
        event.preventDefault();
        finish(true);
      } else if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        finish(false);
      }
    });
    field.addEventListener("blur", () => finish(true));
  }
}
