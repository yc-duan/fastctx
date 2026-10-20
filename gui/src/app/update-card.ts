// The card a new version opens in, from the sidebar's button (`world-upgrade.md`,
// 界面上的样子): its version and what it says of itself, how large it is, a
// bar as it comes, and the one button that brings it, then installs it; why
// it did not go through in the interface's words, the update's own a hover
// away; and, where other machines this device's accounts may update can
// take a newer version too, a word on how many, opening their update view.

import { Sheet } from "./confirm";
import { escape } from "../conversation/prose";
import { effect } from "./signals";
import type { Offered, Updates } from "./updates";
import type { UpdateViewWords } from "./update-view-words";
import type { UpdateWords } from "./update-words";

export interface CardOthers {
  view: UpdateViewWords;
  /** How many other machines this device's accounts may update can take
   *  a newer version, as the hub says. */
  count(): Promise<number>;
  /** Opens the machines on their update view. */
  open(): void;
}

export class UpdateCard {
  private sheet: Sheet | null = null;
  private stop: (() => void) | null = null;
  private drawn = "";
  /** The other machines that can update, as last counted. */
  private others = 0;

  constructor(
    private readonly updates: Updates,
    private readonly words: UpdateWords,
    /** Whether this is a phone, whose system asks before it installs. */
    private readonly phone: boolean,
    private readonly network: CardOthers,
  ) {}

  open(): void {
    if (!this.sheet) {
      const sheet = new Sheet("update-sheet");
      sheet.body.addEventListener("click", (event) => {
        const target = event.target as HTMLElement;
        if (target.closest("[data-update-go]")) this.updates.go();
        if (target.closest("[data-update-others]")) {
          sheet.close();
          this.network.open();
        }
      });
      sheet.dialog.addEventListener("close", () => {
        this.stop?.();
        this.stop = null;
      });
      this.sheet = sheet;
    }
    const sheet = this.sheet;
    this.stop?.();
    this.drawn = "";
    this.stop = effect(() => this.draw(sheet, this.updates.offered.get()));
    sheet.show();
    // Asked as it opens: the hub knows which of them run what.
    void this.network.count().then(
      (count) => {
        if (count === this.others) return;
        this.others = count;
        this.drawn = "";
        this.draw(sheet, this.updates.offered.peek());
      },
      () => {},
    );
  }

  private draw(sheet: Sheet, offered: Offered | null): void {
    const { words } = this;
    if (!offered) {
      sheet.close();
      return;
    }
    const { manifest, stage } = offered;
    const beta = manifest.version.includes("-");
    const megabytes = (bytes: number) => (bytes / 1_000_000).toFixed(1);
    const facts = [offered.size ? words.size(megabytes(offered.size)) : "", stage.kind === "fetching" && stage.network ? words.viaNetwork : ""]
      .filter(Boolean)
      .join(" ");
    const percent =
      stage.kind === "fetching" && stage.total ? Math.min(100, Math.floor((stage.received / stage.total) * 100)) : null;
    const go = (label: string) => `<button class="sheet-go" type="button" data-update-go>${escape(label)}</button>`;
    let foot: string;
    switch (stage.kind) {
      case "offered":
        foot = go(words.go);
        break;
      case "fetching":
        foot = `<div class="update-bar" role="progressbar" aria-valuemin="0" aria-valuemax="100"${percent === null ? "" : ` aria-valuenow="${percent}"`}><span style="width:${percent ?? 4}%"></span></div><span class="update-said">${escape(percent === null ? words.fetching : `${percent}%`)}</span>`;
        break;
      case "ready":
        foot = `${this.phone ? `<span class="update-said">${escape(words.phoneAsks)}</span>` : ""}${go(this.phone ? words.go : words.restart)}`;
        break;
      case "waiting":
        foot = `<span class="update-said">${escape(words.waiting)}</span>${go(this.phone ? words.go : words.restart)}`;
        break;
      case "installing":
        foot = `<span class="update-said">${escape(this.phone ? words.phoneAsks : words.installing)}</span>`;
        break;
      case "permission":
        foot = `<span class="update-said">${escape(words.permission)}</span>${go(words.go)}`;
        break;
      case "failed": {
        const failures = this.network.view.failures;
        foot = `<span class="update-said update-failed" title="${escape(stage.why)}">${escape(words.failed(failures[stage.reason?.kind ?? "other"]))}</span>${go(words.retry)}`;
        break;
      }
    }
    const html = `${sheet.head(words.title, words.close)}
      <div class="update-card">
        <p class="update-name"><span>${escape(words.named(manifest.version))}</span>${beta ? `<span class="update-tag">${escape(words.beta)}</span>` : ""}</p>
        ${manifest.notes ? `<p class="update-notes">${escape(manifest.notes.split("\n")[0]!)}</p>` : ""}
        ${facts ? `<p class="update-facts">${escape(facts)}</p>` : ""}
        ${offered.minimum ? `<p class="update-minimum">${escape(words.minimum(offered.minimum))}</p>` : ""}
        <div class="update-foot">${foot}</div>
        ${this.others ? `<button class="text-button quiet-button update-others" type="button" data-update-others>${escape(words.others(this.others))}</button>` : ""}
      </div>`;
    if (html !== this.drawn) {
      this.drawn = html;
      sheet.body.innerHTML = html;
    }
    sheet.busy = stage.kind === "installing";
  }
}
