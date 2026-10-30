// Which World this computer runs, beside the interface's carried program.
// A newer program the interface carries goes in place by itself when the
// interface opens (`take_carried`); the program does it and keeps its own
// record, and this row says where it stands, step by step: asked again
// every two seconds while a step is under way, as the World is, and every
// few while it waits for an old version to exit or its turn, so it changes
// by itself once it is done (`world-upgrade.md`), and every half minute
// once it is done or failed, as the World tries a failed change again by
// itself. Its button starts that at once. A node that does not answer
// while it hands over is asked again; only one silent for long is said to
// fail.

import { escape } from "../conversation/prose";
import type { RenewalPhase, RenewalStatus } from "../world/renewal";
import { buildLabel, buildOrder, sameBuild } from "../world/renewal";
import { pathFacts } from "./path-facts";
import type { SettingsWords } from "./words";

export interface Renewing {
  state(): Promise<RenewalStatus>;
  /** Puts the carried program in place and asks the node to hand over. */
  renew(): Promise<void>;
}

/** Steps the World is taking by itself, after which it is asked again. */
const UNDER_WAY: RenewalPhase[] = ["preparing", "waiting_for_idle", "updating_guardian", "returning_to_service"];
/** Steps it waits on someone or something else in, asked less often. */
const WAITING: RenewalPhase[] = ["staged", "awaiting_administrator", "awaiting_legacy_exit"];
const UNDER_WAY_MS = 2_000;
const WAITING_MS = 5_000;
/** Done or failed, it is still asked now and then: a failed change is tried
 *  again by the World on its own schedule, and a later one may begin. */
const SETTLED_MS = 30_000;
/** How long a node handing over may not answer before that is said. */
const SILENT_MS = 20_000;

export class RenewalRow {
  private status: RenewalStatus | null = null;
  private said = "";
  /** The kernel's own words for what failed, for the hover. */
  private why = "";
  private busy = false;
  /** What is said is that the node did not answer. */
  private unanswered = false;
  private timer = 0;
  private left = false;
  /** Since when the node has not answered, while a change goes on. */
  private silent: number | null = null;

  constructor(
    private readonly element: HTMLElement,
    private readonly words: SettingsWords,
    private readonly renewing: Renewing,
  ) {
    element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-renew]");
      if (button && !button.disabled) this.act();
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
        this.silent = null;
        // What a node that did not answer left said goes once it answers.
        if (this.unanswered) this.said = "";
        this.unanswered = false;
        this.draw();
        const phase = status.phase;
        const after = phase && UNDER_WAY.includes(phase) ? UNDER_WAY_MS : phase && WAITING.includes(phase) ? WAITING_MS : SETTLED_MS;
        this.timer = window.setTimeout(() => this.ask(), after);
      },
      (error: unknown) => {
        if (this.left) return;
        const phase = this.status?.phase;
        const going = !!phase && (UNDER_WAY.includes(phase) || WAITING.includes(phase));
        this.silent ??= Date.now();
        if (going && Date.now() - this.silent < SILENT_MS) {
          this.timer = window.setTimeout(() => this.ask(), UNDER_WAY_MS);
          return;
        }
        this.why = String(error instanceof Error ? error.message : error);
        this.said = this.words.notDone(this.why);
        this.unanswered = true;
        this.draw();
        this.timer = window.setTimeout(() => this.ask(), SETTLED_MS);
      },
    );
  }

  private act(): void {
    if (this.busy) return;
    this.busy = true;
    this.said = "";
    this.why = "";
    this.draw();
    this.renewing.renew()
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
    const buttons = offer ? `<button class="text-button quiet-button" type="button" data-renew="renew"${this.busy ? " disabled" : ""}>${escape(words.renew)}</button>` : "";
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
