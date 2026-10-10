// Which World this computer runs, beside the interface's carried program.
// Putting a newer program in place
// is the user's to start; the program the interface carries does it and
// keeps its own record, and this row says where it stands, step by step,
// asked again every two seconds while a step is under way, as the World
// is. The one step that needs an administrator is asked for only when it
// is reached.

import { escape } from "../conversation/prose";
import type { RenewalPhase, RenewalStatus } from "../world/renewal";
import { buildLabel, buildOrder, sameBuild } from "../world/renewal";
import { pathFacts } from "./path-facts";
import type { SettingsWords } from "./words";

export interface Renewing {
  state(): Promise<RenewalStatus>;
  /** Puts the carried program in place and asks the node to hand over. */
  renew(): Promise<void>;
  /** The administrator's step: the part that keeps this computer in the
   *  World after it starts. */
  guardian(): Promise<void>;
}

/** Steps the World is taking by itself, after which it is asked again. */
const UNDER_WAY: RenewalPhase[] = ["preparing", "waiting_for_idle", "updating_guardian", "returning_to_service"];

export class RenewalRow {
  private status: RenewalStatus | null = null;
  private said = "";
  /** The kernel's own words for what failed, for the hover. */
  private why = "";
  private busy = false;
  private timer = 0;
  private left = false;

  constructor(
    private readonly element: HTMLElement,
    private readonly words: SettingsWords,
    private readonly renewing: Renewing,
  ) {
    element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-renew]");
      if (button && !button.disabled) this.act(button.dataset.renew as "renew" | "guardian");
    });
    this.ask();
  }

  leave(): void {
    this.left = true;
    clearTimeout(this.timer);
  }

  private ask(): void {
    clearTimeout(this.timer);
    this.renewing.state().then(
      (status) => {
        if (this.left) return;
        this.status = status;
        this.draw();
        if (status.phase && UNDER_WAY.includes(status.phase)) this.timer = window.setTimeout(() => this.ask(), 2000);
      },
      (error: unknown) => {
        if (this.left) return;
        this.said = String(error);
        this.draw();
      },
    );
  }

  private act(what: "renew" | "guardian"): void {
    if (this.busy) return;
    this.busy = true;
    this.said = "";
    this.why = "";
    this.draw();
    (what === "renew" ? this.renewing.renew() : this.renewing.guardian())
      .catch((error: unknown) => {
        this.said = this.words.renewFailed(String(error));
        this.why = String(error);
      })
      .finally(() => {
        this.busy = false;
        this.ask();
      });
  }

  private draw(): void {
    const { words } = this;
    const status = this.status;
    const node = status?.node?.build ?? null;
    const mismatch = status?.mismatch ?? null;
    let state = "";
    let builds = "";
    if (mismatch) {
      const legacy = { version: mismatch.node_version, protocol: mismatch.node_protocol, digest: "?" };
      state = words.versionDifferent(buildOrder(status?.client, legacy));
      builds = `${buildLabel(status?.client)}\n${buildLabel(legacy)}`;
    } else if (node && sameBuild(status?.client, node)) state = words.versionHere(node.version);
    else if (node) {
      state = words.versionDifferent(buildOrder(status?.client, node));
      builds = `${buildLabel(status?.client)}\n${buildLabel(node)}`;
    } else if (status) state = words.versionNone;
    const phase = status?.phase ?? null;
    const step = phase ? words.renewPhase[phase] : "";
    const failed = phase === "failed" && status?.error ? words.renewFailed(status.error) : "";
    // Every difference is visible; the installer determines whether the
    // carried program can advance the World or is already older than it.
    const differs = node ? !sameBuild(status?.client, node) : Boolean(status?.client && !sameBuild(status.client, status.installed));
    const pending = status?.pending && !sameBuild(status.pending, node ?? status.installed);
    const offer = (mismatch || pending || differs) && (!phase || phase === "staged" || phase === "failed" || phase === "complete");
    const buttons = [
      offer ? `<button class="text-button quiet-button" type="button" data-renew="renew"${this.busy ? " disabled" : ""}>${escape(words.renew)}</button>` : "",
      phase === "awaiting_administrator"
        ? `<button class="text-button quiet-button" type="button" data-renew="guardian" title="${escape(words.guardianWhy)}"${this.busy ? " disabled" : ""}>${escape(words.guardian)}</button>`
        : "",
    ].join("");
    const why = [phase === "failed" ? (status?.error ?? "") : "", this.why].filter(Boolean).join("\n");
    const installed = status?.client && status.installed && !sameBuild(status.client, status.installed);
    const paths = pathFacts(status?.commands, words);
    const notes: { text: string; title: string; warn?: boolean }[] = [
      ...(installed ? [{ text: words.versionInstalled, title: `${buildLabel(status!.client)}\n${buildLabel(status!.installed)}` }] : []),
      ...paths.map((fact) => ({ text: fact.text, title: fact.detail })),
      ...(paths.some((fact) => fact.legacy) ? [{ text: words.pathLegacy, title: "", warn: true }] : []),
      ...[step, failed, this.said].filter(Boolean).map((text) => ({ text, title: why })),
    ];
    this.element.hidden = !state && !notes.length;
    this.element.innerHTML = `<dt>${escape(words.worldVersion)}</dt><dd><span${builds ? ` title="${escape(builds)}"` : ""}>${escape(state)}</span>${buttons}${notes
      .map((note) => `<p class="settings-said${note.warn ? " settings-warn" : ""}" role="status"${note.title ? ` title="${escape(note.title)}"` : ""}>${escape(note.text)}</p>`)
      .join("")}</dd>`;
  }
}
