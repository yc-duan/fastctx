// A conversation moved by dragging it in the sidebar (`world-gui.md`, 侧栏):
// dropped on a project's folder it moves into that project, on the
// conversations outside any project it leaves its own; while it is dragged
// two places rise at the sidebar's foot, archive and delete, and dropped on
// one it is archived at once, or asked about once as its "⋯" asks. A
// project of another account takes nothing. The row still drags into the
// panel beside a conversation, as before (`side/mount.ts`).

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { ResidentNow } from "../world/world";
import type { TalkActs } from "./talk-acts";
import type { TalkMenuWords } from "./talk-menu-words";

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
  let dragged: { r: ResidentNow; row: HTMLElement } | null = null;
  let over: HTMLElement | null = null;

  const mark = (target: HTMLElement | null) => {
    if (over === target) return;
    over?.classList.remove("sb-over");
    over = target;
    over?.classList.add("sb-over");
  };
  const end = () => {
    mark(null);
    dragged = null;
    sidebar.classList.remove("sb-dragging");
  };
  /** Where a drop would land, when it would do anything. */
  const targetOf = (event: DragEvent): HTMLElement | null => {
    if (!dragged) return null;
    const target = (event.target as Element | null)?.closest<HTMLElement>("[data-drop]") ?? null;
    if (!target) return null;
    const { r, row } = dragged;
    const sameAccount = target.closest(".sidebar-account") === row.closest(".sidebar-account");
    switch (target.dataset.drop) {
      case "archive":
        return r.removed ? null : target;
      case "delete":
        return target;
      case "town":
        return sameAccount && target.dataset.town !== (r.town ?? undefined) ? target : null;
      case "loose":
        return sameAccount && r.town ? target : null;
      default:
        return null;
    }
  };

  sidebar.addEventListener("dragstart", (event) => {
    const item = (event.target as HTMLElement).closest<HTMLElement>(".talk-item[data-resident]");
    if (!item?.dataset.resident || !item.dataset.machine) return;
    const r = options.find(item.dataset.resident, item.dataset.machine);
    if (!r) return;
    dragged = { r, row: item };
    // The foot rises once the drag has begun, so the row is still where
    // the pointer took it when the browser draws its image.
    requestAnimationFrame(() => {
      if (dragged) sidebar.classList.add("sb-dragging");
    });
  });
  sidebar.addEventListener("dragend", end);
  sidebar.addEventListener("dragover", (event) => {
    const target = targetOf(event);
    mark(target);
    if (!target) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
  });
  sidebar.addEventListener("dragleave", (event) => {
    if (!sidebar.contains(event.relatedTarget as Node | null)) mark(null);
  });
  sidebar.addEventListener("drop", (event) => {
    const target = targetOf(event);
    const r = dragged?.r;
    end();
    if (!target || !r) return;
    event.preventDefault();
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
  });
}
