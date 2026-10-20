// What a list of conversations does with its rows (`talk-row.ts`) beyond
// following them: a row's "⋯" opens its menu, the right button opens it where
// it was pressed, a finger held on a row raises it from below, and a row is
// renamed in place (`world-gui.md`, 界面的结构). The sidebar and the page of
// every conversation both list them so.

import { titleOf } from "../world/names";
import type { ResidentNow } from "../world/world";
import { addressOf } from "./store";
import type { TalkActs } from "./talk-acts";
import type { TalkMenu } from "./talk-menu";
import type { TalkMenuWords } from "./talk-menu-words";
import type { RowActs } from "./talk-row";
import { composing } from "../typing";
import { onHeld } from "./press";

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

  constructor(
    private readonly list: HTMLElement,
    private readonly options: TalkListOptions,
  ) {
    list.addEventListener(
      "click",
      (event) => {
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
    // Held by a finger, the menu rises from below; opened with the right
    // button, it stands where it was pressed.
    onHeld(list, ".talk-item[data-resident]", (item, event, input) => {
      const r = this.rowOf(item);
      if (!r) return false;
      const rename = () => this.rename(r);
      options.menu.open(r, input === "touch" ? { rename } : { at: { x: event.clientX, y: event.clientY }, rename });
    });
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
