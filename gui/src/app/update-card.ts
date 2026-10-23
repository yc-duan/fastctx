// This device's updates in one card (`world-upgrade.md`, 界面上的样子), opened
// from the sidebar's button, the settings and this device's row of the
// machines' update view: a new version, when there is one, with what it says
// of itself, how large it is, a bar as it comes, and the one button that
// brings it, then installs it, why it did not go through in the interface's
// words, the update's own a hover away; then the version this device runs
// and a look for newer ones, its automatic updates and passing packages on,
// and the World's program on this computer beside the interface's; and,
// where other machines this device's accounts may update can take a newer
// version too, a word on how many, opening their update view. Where the
// update did not go through, or this device is stuck where nothing else
// helps, it offers a repair, and follows it layer by layer in the card.
// (`dev-notes/2026-10-22-更新的修复-设计.md`, 谁来按、在哪里按)

import { Sheet } from "./confirm";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { RepairLayers } from "./repair-layers";
import { RenewalRow, type Renewing } from "./renewal";
import { effect } from "./signals";
import { UpdateRows } from "./update-rows";
import { troubled, type Offered, type Updates } from "./updates";
import type { UpdateViewWords } from "./update-view-words";
import type { UpdateWords } from "./update-words";
import type { SettingsWords } from "./words";

/** What the card says of this device beside a new version. */
export interface CardHere {
  /** The window of versions, looking on GitHub again when asked. */
  versions(look: boolean): void;
  /** Whether this device passes packages on: a computer does, a phone not. */
  relays: boolean;
  /** The World's program on this computer, where it has one. */
  renewing: Renewing | null;
  settings: SettingsWords;
}

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
  /** This device's rows while the card is open. */
  private rows: UpdateRows | null = null;
  private renewal: RenewalRow | null = null;
  /** The repair's layers while the card follows one. */
  private layers: RepairLayers | null = null;
  private stopRepair: (() => void) | null = null;

  constructor(
    private readonly updates: Updates,
    private readonly words: UpdateWords,
    /** Whether this is a phone, whose system asks before it installs. */
    private readonly phone: boolean,
    private readonly network: CardOthers,
    private readonly here: CardHere,
  ) {}

  open(): void {
    if (!this.sheet) {
      const sheet = new Sheet("update-sheet");
      sheet.body.addEventListener("click", (event) => {
        const target = event.target as HTMLElement;
        if (target.closest("[data-update-go]")) this.updates.go();
        if (target.closest("[data-update-repair]")) this.updates.repair();
        if (target.closest("[data-update-others]")) {
          sheet.close();
          this.network.open();
        }
      });
      sheet.dialog.addEventListener("close", () => {
        this.stop?.();
        this.stop = null;
        this.stopRepair?.();
        this.stopRepair = null;
        this.layers = null;
        // A repair that ended is not shown again; one under way goes on.
        const repairing = this.updates.repairing.peek();
        if (repairing && (repairing.error || repairing.report?.finished)) this.updates.forgetRepair();
        this.rows?.leave();
        this.renewal?.leave();
        this.rows = null;
        this.renewal = null;
      });
      this.sheet = sheet;
    }
    const sheet = this.sheet;
    this.stop?.();
    this.stopRepair?.();
    this.rows?.leave();
    this.renewal?.leave();
    this.drawn = "";
    this.layers = null;
    sheet.body.innerHTML = `<div class="update-head"></div><div class="update-card"><div class="update-new" hidden></div><dl class="facts update-here"></dl><dl class="facts update-renewal" hidden></dl><div class="update-repair" hidden></div><div class="update-more"></div></div>`;
    const { here, updates, words } = this;
    this.rows = new UpdateRows(sheet.body.querySelector<HTMLElement>(".update-here")!, updates, words, this.network.view, here.versions, here.relays);
    this.renewal = here.renewing ? new RenewalRow(sheet.body.querySelector<HTMLElement>(".update-renewal")!, here.settings, here.renewing) : null;
    this.stop = effect(() => this.draw(sheet, this.updates.offered.get()));
    this.stopRepair = effect(() => this.drawRepair(sheet));
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
    const head = sheet.head(offered ? words.title : words.here, words.close);
    const more = this.others ? `<button class="text-button quiet-button update-others" type="button" data-update-others>${escape(words.others(this.others))}</button>` : "";
    if (!offered) {
      this.place(sheet, head, null, more);
      sheet.busy = false;
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
        const repair = this.updates.state.peek()?.repair_supported ? this.repairButton(words.repair.repair, false) : "";
        foot = `<span class="update-said update-failed" title="${escape(stage.why)}">${escape(words.failed(failures[stage.reason?.kind ?? "other"]))}</span>${repair}${go(words.retry)}`;
        break;
      }
    }
    const fresh = `<p class="update-name"><span>${escape(words.named(manifest.version))}</span>${beta ? `<span class="update-tag">${escape(words.beta)}</span>` : ""}</p>
        ${manifest.notes ? `<p class="update-notes">${escape(manifest.notes.split("\n")[0]!)}</p>` : ""}
        ${facts ? `<p class="update-facts">${escape(facts)}</p>` : ""}
        ${offered.minimum ? `<p class="update-minimum">${escape(words.minimum(offered.minimum))}</p>` : ""}
        <div class="update-foot">${foot}</div>`;
    this.place(sheet, head, fresh, more);
    sheet.busy = stage.kind === "installing";
  }

  private repairButton(label: string, main: boolean): string {
    return `<button class="${main ? "sheet-go" : "text-button"} update-repair-go" type="button" data-update-repair>${glyph("repair", 16)}<span>${escape(label)}</span></button>`;
  }

  /** The repair: offered where this device is stuck and the update's own
   *  line does not offer it already; once begun, its layers ticked in place
   *  and how it ended, with another try where it stopped short. */
  private drawRepair(sheet: Sheet): void {
    const part = sheet.body.querySelector<HTMLElement>(".update-repair");
    if (!part) return;
    const words = this.words.repair;
    const following = this.updates.repairing.get();
    const state = this.updates.state.get();
    const offered = this.updates.offered.get();
    if (!following) {
      this.layers = null;
      const offer = state?.repair_supported && troubled(state, offered) && offered?.stage.kind !== "failed";
      part.hidden = !offer;
      part.innerHTML = offer ? `<div class="update-foot">${this.repairButton(words.repair, false)}</div>` : "";
      return;
    }
    const report = following.report;
    const ended = following.error !== null || (report?.finished ?? null) !== null;
    const went = following.error !== null ? false : (report?.success ?? false);
    const said = !ended ? words.repairing : went ? (report?.unchanged ? words.fine : words.repaired) : words.repairFailed;
    const why = following.error ?? report?.error ?? "";
    if (!this.layers) {
      this.layers = new RepairLayers(words);
      part.innerHTML = `<p class="update-repair-said"></p><div class="update-repair-layers"></div><div class="update-foot update-repair-acts"></div>`;
      part.querySelector(".update-repair-layers")!.append(this.layers.element);
    }
    part.hidden = false;
    const line = part.querySelector<HTMLElement>(".update-repair-said")!;
    line.textContent = said;
    line.classList.toggle("update-failed", ended && !went);
    line.title = why;
    this.layers.update(report?.steps ?? []);
    const acts = part.querySelector<HTMLElement>(".update-repair-acts")!;
    const again = ended && !went ? this.repairButton(words.repairAgain, false) : "";
    if (acts.innerHTML !== again) acts.innerHTML = again;
    acts.hidden = !again;
    sheet.anew();
  }

  /** Puts in what changed of the card's own parts; this device's rows
   *  keep themselves. */
  private place(sheet: Sheet, head: string, fresh: string | null, more: string): void {
    const html = `${head}|${fresh ?? ""}|${more}`;
    if (html === this.drawn) return;
    this.drawn = html;
    sheet.body.querySelector(".update-head")!.innerHTML = head;
    const part = sheet.body.querySelector<HTMLElement>(".update-new")!;
    part.hidden = fresh === null;
    part.innerHTML = fresh ?? "";
    sheet.body.querySelector(".update-more")!.innerHTML = more;
    sheet.anew();
  }
}
