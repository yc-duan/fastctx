// Taking a machine off the network, or several, three ways
// (`world-network.md`, 移出网络与卸载): uninstalling Fastctx there and keeping
// its data, the way chosen first; erasing everything Fastctx wrote there; or
// only taking it off. The two that uninstall say why when they cannot be
// taken, and of several how many only leave; the red button acts. Once they
// are off, the machines told to uninstall are followed for a while: one that
// could not do all of it says what and why, with the line to run there.

import { Sheet } from "./confirm";
import { copyButton, copyFrom } from "./copy-button";
import { escape } from "../conversation/prose";
import { detailOf } from "./problem";
import type { ManageWords } from "./machine-manage-words";

export type RemoveWay = "uninstall" | "everything" | "leave";

/** How long the machines told to uninstall are followed, and how often
 *  the hub's log is read meanwhile. */
const FOLLOW_MS = 60_000;
const LOOK_MS = 2_000;

/** A machine told to uninstall Fastctx as it was taken off. */
export interface Uninstalling {
  written: string;
  /** Offline when taken off: it uninstalls once it is back. */
  away: boolean;
}

/** The machines told to uninstall, and what each has said it could not do
 *  since, by its written name, as its hub logged it. */
export interface Following {
  machines: Uninstalling[];
  left(): Promise<Map<string, string>>;
  /** Those of the rest that could not be taken off, as said, and why in
   *  the kernel's words, for the pointer. */
  failed?: { said: string; detail: string };
}

/** Machines that could not be taken off, with why in the kernel's words. */
export class RemoveFailed extends Error {
  constructor(readonly machines: string[], detail: string) {
    super(detail);
  }
}

export interface Removing {
  /** The sheet's title, naming the machine or saying how many. */
  title: string;
  /** Why Fastctx is uninstalled from here on none of them, when it is not. */
  cannot: string | null;
  /** Of several, those that only leave and why, when some do. */
  some: string | null;
  offline: boolean;
  /** Said when the one taking them off is not their owner. */
  others: string | null;
  /** Takes them off; resolves to those told to uninstall, to follow. */
  remove(way: RemoveWay): Promise<Following | null>;
}

export class RemoveMachineSheet {
  private sheet: Sheet | null = null;
  private removing: Removing | null = null;
  private way: RemoveWay = "uninstall";
  private said = "";
  /** What the kernel said of it, for the pointer. */
  private detail = "";
  private following: Following | null = null;
  private left = new Map<string, string>();
  /** The following ended without word from those still quiet. */
  private over = false;
  private timer: number | null = null;

  constructor(private readonly words: ManageWords) {}

  open(removing: Removing): void {
    this.stop();
    this.removing = removing;
    this.way = removing.cannot ? "leave" : "uninstall";
    this.said = "";
    this.detail = "";
    const sheet = this.sheet ?? this.make();
    this.draw();
    sheet.show();
  }

  private make(): Sheet {
    const sheet = new Sheet("device-sheet remove-machine-sheet");
    this.sheet = sheet;
    sheet.body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const way = target.closest<HTMLButtonElement>("[data-mr-way]");
      if (way && !way.disabled) {
        this.way = way.dataset.mrWay as RemoveWay;
        this.draw();
        return;
      }
      const copy = target.closest<HTMLElement>(".copy-button");
      if (copy) {
        void copyFrom(copy, this.words.copied);
        return;
      }
      if (target.closest("[data-mr-go]")) this.go();
      if (target.closest("[data-mr-done]")) sheet.close();
    });
    sheet.dialog.addEventListener("close", () => this.stop());
    return sheet;
  }

  private go(): void {
    const { sheet, removing } = this;
    if (!sheet || !removing || sheet.busy) return;
    sheet.busy = true;
    this.draw();
    removing.remove(this.way).then(
      (following) => {
        sheet.busy = false;
        if (following?.machines.length) this.follow(following);
        else sheet.close();
      },
      (error: unknown) => {
        sheet.busy = false;
        this.said = this.words.removeFailed(error instanceof RemoveFailed ? error.machines : []);
        this.detail = detailOf(error);
        this.draw();
      },
    );
  }

  /** Reads what the machines told to uninstall say, until each said it or
   *  is away, or the time is up. */
  private follow(following: Following): void {
    this.following = following;
    this.left = new Map();
    this.over = false;
    const began = Date.now();
    const look = (): void => {
      void following
        .left()
        .then((left) => {
          if (this.following === following) this.left = left;
        })
        .catch((error: unknown) => console.error("reading what the machines taken off said", error))
        .finally(() => {
          if (this.following !== following) return;
          const quiet = following.machines.filter((one) => !one.away && !this.left.has(one.written));
          if (!quiet.length || Date.now() - began >= FOLLOW_MS) this.over = true;
          else this.timer = window.setTimeout(look, LOOK_MS);
          this.draw();
        });
    };
    this.draw();
    this.sheet?.anew();
    look();
  }

  private stop(): void {
    this.following = null;
    if (this.timer !== null) window.clearTimeout(this.timer);
    this.timer = null;
  }

  private draw(): void {
    const { sheet, removing, words } = this;
    if (!sheet || !removing) return;
    if (this.following) {
      const failed = this.following.failed ? `<p class="sheet-note" role="alert" title="${escape(this.following.failed.detail)}">${escape(this.following.failed.said)}</p>` : "";
      sheet.body.innerHTML = `${sheet.head(words.removedTitle, words.close)}
        <div class="mr-follow">${this.following.machines.map((one) => this.row(one)).join("")}</div>
        ${failed}
        <div class="sheet-foot mr-go"><button class="sheet-go" type="button" data-mr-done>${escape(words.done)}</button></div>`;
      return;
    }
    const way = (kind: RemoveWay) => {
      const { title, what } = words.ways[kind];
      const blocked = kind !== "leave" && removing.cannot !== null;
      const chosen = this.way === kind;
      const some = kind !== "leave" && !blocked && removing.some ? `<span class="mr-way-some">${escape(removing.some)}</span>` : "";
      return `<button type="button" class="mr-way${chosen ? " chosen" : ""}${kind === "everything" ? " erase" : ""}" role="radio" aria-checked="${chosen}" data-mr-way="${kind}"${blocked ? " disabled" : ""}>
        <span class="mr-way-title">${escape(title)}</span>
        <span class="mr-way-what">${escape(blocked ? removing.cannot! : what)}</span>${some}
      </button>`;
    };
    const note =
      (removing.others ? `<p class="mr-note">${escape(removing.others)}</p>` : "") +
      (removing.offline && this.way !== "leave" ? `<p class="mr-note">${escape(words.removeOffline)}</p>` : "");
    sheet.body.innerHTML = `${sheet.head(removing.title, words.close)}
      <div class="mr-ways" role="radiogroup" aria-label="${escape(removing.title)}">${way("uninstall")}${way("everything")}${way("leave")}</div>
      ${note}
      <p class="sheet-note" role="alert"${this.said ? "" : " hidden"} title="${escape(this.detail)}">${escape(this.said)}</p>
      <div class="sheet-foot mr-go"><button class="sheet-go danger" type="button" data-mr-go${sheet.busy ? " disabled" : ""}>${escape(words.removeGo)}</button></div>`;
  }

  /** One machine taken off to uninstall, and how that goes as it said. */
  private row(one: Uninstalling): string {
    const { words } = this;
    const left = this.left.get(one.written);
    if (left !== undefined) {
      // The line to run there is set apart; the machine's words read on
      // without its marks.
      const command = /`([^`]+)`/.exec(left)?.[1];
      return `<div class="mr-follow-row left">
        <span class="mr-follow-name">${escape(one.written)}</span>
        <span class="mr-follow-state">${escape(words.uninstallLeft)}</span>
        <span class="mr-follow-why">${escape(left.replace(/`([^`]+)`/g, "$1"))}</span>${
          command
            ? `<span class="mr-follow-line"><span class="mr-follow-lead">${escape(words.runThere)}</span><code class="mono">${escape(command)}</code>${copyButton(command, words.copy)}</span>`
            : ""
        }
      </div>`;
    }
    const state = one.away ? words.uninstallAway : this.over ? words.uninstallQuiet : words.uninstalling;
    return `<div class="mr-follow-row${!one.away && !this.over ? " going" : ""}">
      <span class="mr-follow-name">${escape(one.written)}</span>
      <span class="mr-follow-state">${escape(state)}</span>
    </div>`;
  }
}
