// Taking a machine off the network, or several, three ways
// (`world-network.md`, 移出网络与卸载): uninstalling Fastctx there and keeping
// its data, the way chosen first; erasing everything Fastctx wrote there; or
// only taking it off. The two that uninstall say why when they cannot be
// taken, and of several how many only leave; the red button acts.

import { Sheet } from "./confirm";
import { escape } from "../conversation/prose";
import type { ManageWords } from "./machine-manage-words";

export type RemoveWay = "uninstall" | "everything" | "leave";

export interface Removing {
  /** The sheet's title, naming the machine or saying how many. */
  title: string;
  /** Why Fastctx is uninstalled from here on none of them, when it is not. */
  cannot: string | null;
  /** Of several, those that only leave and why, when some do. */
  some: string | null;
  offline: boolean;
  remove(way: RemoveWay): Promise<void>;
}

export class RemoveMachineSheet {
  private sheet: Sheet | null = null;
  private removing: Removing | null = null;
  private way: RemoveWay = "uninstall";
  private said = "";

  constructor(private readonly words: ManageWords) {}

  open(removing: Removing): void {
    this.removing = removing;
    this.way = removing.cannot ? "leave" : "uninstall";
    this.said = "";
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
      if (target.closest("[data-mr-go]")) this.go();
    });
    return sheet;
  }

  private go(): void {
    const { sheet, removing } = this;
    if (!sheet || !removing || sheet.busy) return;
    sheet.busy = true;
    this.draw();
    removing.remove(this.way).then(
      () => {
        sheet.busy = false;
        sheet.close();
      },
      (error: unknown) => {
        sheet.busy = false;
        this.said = this.words.removeFailed(error instanceof Error ? error.message : String(error));
        this.draw();
      },
    );
  }

  private draw(): void {
    const { sheet, removing, words } = this;
    if (!sheet || !removing) return;
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
    const note = removing.offline && this.way !== "leave" ? `<p class="mr-note">${escape(words.removeOffline)}</p>` : "";
    sheet.body.innerHTML = `${sheet.head(removing.title, words.close)}
      <div class="mr-ways" role="radiogroup" aria-label="${escape(removing.title)}">${way("uninstall")}${way("everything")}${way("leave")}</div>
      ${note}
      <p class="sheet-note" role="alert"${this.said ? "" : " hidden"}>${escape(this.said)}</p>
      <div class="mr-go"><button class="sheet-go danger" type="button" data-mr-go${sheet.busy ? " disabled" : ""}>${escape(words.removeGo)}</button></div>`;
  }
}
