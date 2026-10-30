// A conversation moved by dragging it in the sidebar (`world-gui.md`, 侧栏):
// dropped on a project's folder it moves into that project, on the
// conversations outside any project it leaves its own; while it is dragged
// two places rise at the sidebar's foot, archive and delete, and dropped on
// one it is archived at once, or asked about once as its "⋯" asks. A
// project of another account takes nothing. The row is carried by the
// pointer (`talk-drag.ts`), and the panel beside a conversation takes it too
// (`side/mount.ts`).

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { ResidentNow } from "../world/world";
import type { TalkActs } from "./talk-acts";
import type { TalkMenuWords } from "./talk-menu-words";
import { type Carried, carryRows, placeForDrops } from "./talk-drag";

export interface DragOptions {
  sidebar: HTMLElement;
  words: TalkMenuWords;
  /** The conversation a row stands for. */
  find(resident: string, machine: string): ResidentNow | null;
  acts: TalkActs;
  askDelete(r: ResidentNow): void;
}

/** The two places at the foot, drawn once; shown only while dragging. */
export function dropZones(words: TalkMenuWords): string {
  return `<div class="sb-zones" aria-hidden="true">
      <div class="sb-zone sb-zone-archive" data-drop="archive">${glyph("archive", 18)}<span>${escape(words.archive)}</span></div>
      <div class="sb-zone sb-zone-delete" data-drop="delete">${glyph("trash", 18)}<span>${escape(words.delete)}</span></div>
    </div>`;
}

export function followDrags(options: DragOptions): void {
  const { sidebar } = options;
  let over: HTMLElement | null = null;

  const mark = (target: HTMLElement | null) => {
    if (over === target) return;
    over?.classList.remove("sb-over");
    over = target;
    over?.classList.add("sb-over");
  };
  /** Where a drop would land, when it would do anything. */
  const targetOf = (carried: Carried, under: Element | null): { target: HTMLElement; r: ResidentNow } | null => {
    const target = under?.closest<HTMLElement>("[data-drop]") ?? null;
    if (!target || !sidebar.contains(target)) return null;
    const r = options.find(carried.resident, carried.machine);
    if (!r) return null;
    const sameAccount = target.closest(".sidebar-account") === carried.row.closest(".sidebar-account");
    switch (target.dataset.drop) {
      case "archive":
        return r.removed ? null : { target, r };
      case "delete":
        return { target, r };
      case "town":
        return sameAccount && target.dataset.town !== (r.town ?? undefined) ? { target, r } : null;
      case "loose":
        return sameAccount && r.town ? { target, r } : null;
      default:
        return null;
    }
  };

  carryRows(sidebar, ".talk-item[data-resident]", {
    carried: (row) => (row.dataset.resident && row.dataset.machine ? { resident: row.dataset.resident, machine: row.dataset.machine, row } : null),
    name: (carried) => carried.row.querySelector(".talk-name")?.textContent ?? carried.resident,
    began: () => sidebar.classList.add("sb-dragging"),
    ended: () => sidebar.classList.remove("sb-dragging"),
  });
  placeForDrops({
    over: (carried, _x, _y, under) => {
      const found = targetOf(carried, under);
      mark(found?.target ?? null);
      return !!found;
    },
    out: () => mark(null),
    take: (carried, _x, _y, under) => {
      const found = targetOf(carried, under);
      mark(null);
      if (!found) return;
      const { target, r } = found;
      switch (target.dataset.drop) {
        case "archive":
          options.acts.archive(r);
          break;
        case "delete":
          options.askDelete(r);
          break;
        case "town":
          options.acts.move(r, target.dataset.town ?? null);
          break;
        case "loose":
          options.acts.move(r, null);
          break;
      }
    },
  });
}
