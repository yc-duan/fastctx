// Where the conversation in focus works, read only (`world-gui.md`, 对话的
// 侧面板): the computer it lives on and the folder it works in, in one line
// in the desk's top right corner, which stays there as the side opens and
// folds away; on a phone, in the side's own line. Pressed, it pulls out the
// whole of it: each computer it works on, the way to its page, and each
// folder by its full path, one on this computer shown in the system's own
// window.

import { layerOpened } from "../app/back";
import { Sheet } from "../app/confirm";
import { leave, leaving, stay } from "../app/leave";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { folderName } from "../world/focus";
import type { WhereMachine, WhereShown } from "./types";
import type { WhereWords } from "./words";
import "./where.css";

export class SideWhere {
  /** The line in the desk's corner, its pull-out opening down from it. */
  readonly corner: HTMLElement;
  /** The line in the side's own line, where the side lies over the desk;
   *  pulled out as a sheet. */
  readonly line: HTMLButtonElement;
  private readonly button: HTMLButtonElement;
  private readonly pop: HTMLElement;
  private sheet: Sheet | null = null;
  private shown: WhereShown | null = null;

  constructor(
    private readonly words: WhereWords,
    private readonly close: string,
    /** Shows a folder of this computer in the system's own window. */
    private readonly reveal?: (path: string) => void,
  ) {
    this.corner = document.createElement("div");
    this.corner.className = "sw";
    this.corner.hidden = true;
    this.corner.innerHTML = `<button class="sw-line" type="button" aria-haspopup="dialog" aria-expanded="false"></button><div class="sw-pop" role="dialog" aria-label="${escape(words.title)}" hidden></div>`;
    this.button = this.corner.querySelector(".sw-line")!;
    this.pop = this.corner.querySelector(".sw-pop")!;
    this.button.addEventListener("click", () => (this.pop.hidden || leaving(this.pop) ? this.pull() : this.putBack()));
    this.pop.addEventListener("click", (event) => this.pressed(event));
    this.pop.addEventListener("keydown", (event) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      this.putBack();
      this.button.focus({ preventScroll: true });
    });
    document.addEventListener("pointerdown", (event) => {
      if (this.pop.hidden || !this.corner.isConnected) return;
      if (!this.corner.contains(event.target as Node)) this.putBack();
    });
    this.line = document.createElement("button");
    this.line.type = "button";
    this.line.className = "sw-line";
    this.line.hidden = true;
    this.line.setAttribute("aria-haspopup", "dialog");
    this.line.addEventListener("click", () => this.pullSheet());
  }

  show(where: WhereShown | null): void {
    this.shown = where?.machines.length ? where : null;
    this.corner.hidden = this.line.hidden = !this.shown;
    if (!this.shown) {
      this.putBack();
      this.sheet?.close();
      return;
    }
    const html = this.drawLine(this.shown);
    if (this.button.innerHTML !== html) {
      this.button.innerHTML = html;
      this.line.innerHTML = html;
    }
    const title = this.said(this.shown);
    this.button.title = this.line.title = title;
    this.button.setAttribute("aria-label", title);
    this.line.setAttribute("aria-label", title);
    if (!this.pop.hidden) this.pop.innerHTML = this.drawPlaces(this.shown);
    if (this.sheet?.dialog.open) this.fillSheet(this.sheet, this.shown);
  }

  /** The line: the computer it lives on, the first folder it works in
   *  there, and how many more places the pull-out holds. */
  private drawLine(where: WhereShown): string {
    const host = where.machines[0]!;
    const folder = host.folders[0] ?? null;
    const places = where.machines.reduce((sum, machine, at) => sum + (at === 0 ? machine.folders.length : Math.max(1, machine.folders.length)), 0);
    const more = places - (folder ? 1 : 0);
    const machine = `<span class="sw-at sw-host${host.online === false ? " away" : ""}">${glyph(host.form ?? "machine", 16)}<span class="sw-text">${escape(host.name)}</span></span>`;
    const at = folder ? `<span class="sw-at sw-folder-at">${glyph("folder", 16)}<span class="sw-text">${escape(folderName(folder))}</span></span>` : "";
    return `${machine}${at}${more > 0 ? `<span class="sw-more">${escape(this.words.more(more))}</span>` : ""}`;
  }

  /** On hover: its name, then each computer and its folders in full. */
  private said(where: WhereShown): string {
    const lines = where.machines.map((machine) => {
      const name = machine.online === false ? `${machine.name}（${this.words.away}）` : machine.name;
      return [name, ...machine.folders.map((folder) => `  ${folder}`)].join("\n");
    });
    return [this.words.title, ...(where.from ? [this.words.from(where.from)] : []), ...lines].join("\n");
  }

  /** Every computer it works on, each with the way to its page and its
   *  folders by their full paths. */
  private drawPlaces(where: WhereShown): string {
    return where.machines.map((machine, at) => this.drawMachine(machine, at === 0 ? where.from : null)).join("");
  }

  private drawMachine(machine: WhereMachine, from: string | null): string {
    const away = machine.online === false ? " away" : "";
    const inner = `${glyph(machine.form ?? "machine", 16)}<span class="sw-name">${escape(machine.name)}</span>`;
    const head = machine.link
      ? `<a class="sw-machine${away}" href="${escape(machine.link)}"${away ? ` title="${escape(this.words.away)}"` : ""}>${inner}</a>`
      : `<span class="sw-machine${away}"${away ? ` title="${escape(this.words.away)}"` : ""}>${inner}</span>`;
    const folders = machine.folders
      .map((path) => {
        const inside = `${glyph("folder", 16)}<span class="sw-path">${escape(path)}</span>`;
        return machine.here && this.reveal
          ? `<button class="sw-folder" type="button" data-path="${escape(path)}" title="${escape(this.words.reveal)}">${inside}</button>`
          : `<span class="sw-folder">${inside}</span>`;
      })
      .join("");
    return `<section class="sw-place">${head}${from ? `<p class="sw-from">${escape(this.words.from(from))}</p>` : ""}${folders}</section>`;
  }

  private pressed(event: Event): void {
    const target = event.target as HTMLElement;
    const folder = target.closest<HTMLElement>("button.sw-folder");
    if (folder?.dataset.path) {
      this.reveal?.(folder.dataset.path);
      return;
    }
    // Going to a computer's page, the pull-out goes too.
    if (!target.closest("a.sw-machine")) return;
    this.putBack();
    this.sheet?.close();
  }

  /** Pulled out under the line. */
  private pull(): void {
    if (!this.shown) return;
    this.pop.innerHTML = this.drawPlaces(this.shown);
    stay(this.pop);
    this.pop.hidden = false;
    this.button.setAttribute("aria-expanded", "true");
    layerOpened({ isOpen: () => !this.pop.hidden && !leaving(this.pop) && this.corner.isConnected, close: () => this.putBack() });
  }

  private putBack(): void {
    if (this.pop.hidden || leaving(this.pop)) return;
    this.button.setAttribute("aria-expanded", "false");
    leave(this.pop, () => (this.pop.hidden = true));
  }

  /** On a phone, a sheet from the bottom. */
  private pullSheet(): void {
    if (!this.shown) return;
    const sheet = this.sheet ?? new Sheet("sw-sheet");
    if (!this.sheet) {
      this.sheet = sheet;
      sheet.body.addEventListener("click", (event) => this.pressed(event));
      sheet.dialog.addEventListener("close", () => {
        if (this.sheet === sheet) this.sheet = null;
      });
    }
    this.fillSheet(sheet, this.shown);
    sheet.show();
  }

  private fillSheet(sheet: Sheet, where: WhereShown): void {
    sheet.body.innerHTML = `${sheet.head(this.words.title, this.close)}<div class="sw-places">${this.drawPlaces(where)}</div>`;
    sheet.anew();
  }
}
