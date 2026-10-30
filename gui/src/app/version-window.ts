// The window of versions (`world-upgrade.md`, 版本窗): what a machine runs,
// and beneath it every release, the newest first, each with its day and
// how large its package is for that machine. A newer one is picked to
// update to, not only the newest; an older one is listed greyed and never
// taken, as versions only go forward. Filtered by line, the machine's own
// first. The same window picks for this device, for another machine, for
// several, and the network's lowest version.

import { Sheet } from "./confirm";
import { escape } from "../conversation/prose";
import { compareVersions, isTest, majorOf, type Manifest, type Package, type UpdateState } from "../world/updates";
import type { VersionWords } from "./update-words";

/** What a version picked is for. */
export type VersionPurpose =
  /** Updating the machine the state is of, or several like it. */
  | { kind: "update"; pick(version: string): Promise<void> }
  /** The network's lowest version, or none. */
  | { kind: "minimum"; current: string | null; pick(version: string | null): Promise<void> };

/** The machine whose versions are shown. */
export interface VersionSubject {
  /** The machine's name, as the window's first line says it; none for
   *  this device. */
  who: string | null;
  /** The version it runs. */
  running: string;
  /** How updates stand there, as its node says. */
  state: UpdateState;
  /** Looks on GitHub again, there. */
  look?(): Promise<UpdateState>;
  /** The window's first line, for several machines: what they run. */
  said?: string;
}

type Filter = "all" | "stable" | "test";

interface Row {
  version: string;
  test: boolean;
  day: string | null;
  size: number | null;
  newer: boolean;
  /** Newer, and its package for that machine can be had now. */
  takes: boolean;
  /** Past the major version of the hubs it is on. */
  pastHub: boolean;
}

/** The package of `manifest` for the machine `state` is of: its
 *  application's, or its World program's when no application carries it. */
export function packageFor(manifest: Manifest | undefined, state: UpdateState): Package | undefined {
  if (!manifest) return undefined;
  if (state.app) return manifest.platforms[state.app.target];
  const triple = state.kernel_target.replace(/^fastctx-/, "");
  return manifest.fastctx?.[triple];
}

/** Every manifest `state` knows, by version. */
export function manifestsOf(state: UpdateState): Map<string, Manifest> {
  const found = new Map<string, Manifest>();
  const known = [
    ...(state.looked.newer ?? []),
    state.offer,
    state.chosen,
    state.looked.stable,
    state.looked.beta,
    state.network?.stable,
    state.network?.beta,
  ];
  for (const manifest of known) if (manifest && !found.has(manifest.version)) found.set(manifest.version, manifest);
  return found;
}

/** The releases a machine can be shown, the newest first. */
export function rowsOf(subject: VersionSubject): Row[] {
  const { state, running } = subject;
  const manifests = manifestsOf(state);
  const listed = new Map<string, { day: string | null; test: boolean }>();
  for (const release of state.looked.releases ?? []) {
    listed.set(release.version, { day: release.published ?? null, test: release.prerelease === true || isTest(release.version) });
  }
  for (const manifest of manifests.values()) {
    if (!listed.has(manifest.version)) listed.set(manifest.version, { day: manifest.pub_date ?? null, test: isTest(manifest.version) });
  }
  if (!listed.has(running)) listed.set(running, { day: null, test: isTest(running) });
  return [...listed.entries()]
    .map(([version, { day, test }]) => {
      const newer = compareVersions(version, running) > 0;
      const pack = packageFor(manifests.get(version), state);
      return {
        version,
        test,
        day,
        size: pack?.size ?? null,
        newer,
        takes: newer && pack !== undefined,
        pastHub: state.hub_major !== undefined && majorOf(version) > state.hub_major,
      };
    })
    .sort((a, b) => compareVersions(b.version, a.version));
}

export class VersionWindow {
  private sheet: Sheet | null = null;
  private subject: VersionSubject | null = null;
  private purpose: VersionPurpose | null = null;
  private filter: Filter = "all";
  private looking = false;
  private picking: string | null = null;
  private said = "";
  /** What the World said, for the pointer's rest. */
  private why = "";
  private drawn = "";

  constructor(private readonly words: VersionWords) {}

  /** Opens on `subject`'s versions, looking on GitHub again at once when
   *  `look` says so, as 「检查更新」 does. */
  open(subject: VersionSubject, purpose: VersionPurpose, look = false): void {
    this.subject = subject;
    this.purpose = purpose;
    this.filter = purpose.kind === "minimum" || subject.state.channel === "beta" ? "all" : "stable";
    this.said = "";
    this.picking = null;
    this.drawn = "";
    const sheet = this.sheet ?? this.make();
    this.draw();
    sheet.show();
    if (look) this.look();
  }

  /** The machine's state changed while the window is open. */
  update(state: UpdateState): void {
    if (!this.subject || !this.sheet?.dialog.open) return;
    this.subject = { ...this.subject, state };
    this.draw();
  }

  close(): void {
    this.sheet?.close();
  }

  private make(): Sheet {
    const sheet = new Sheet("version-sheet");
    this.sheet = sheet;
    sheet.body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const filter = target.closest<HTMLElement>("[data-vw-filter]")?.dataset.vwFilter as Filter | undefined;
      if (filter) {
        this.filter = filter;
        this.draw();
        return;
      }
      if (target.closest("[data-vw-look]")) {
        this.look();
        return;
      }
      const pick = target.closest<HTMLElement>("[data-vw-pick]");
      if (pick) this.pick(pick.dataset.vwPick === "" ? null : pick.dataset.vwPick!);
    });
    return sheet;
  }

  private look(): void {
    const subject = this.subject;
    if (!subject?.look || this.looking) return;
    this.looking = true;
    this.said = "";
    this.draw();
    subject.look().then(
      (state) => {
        this.looking = false;
        if (this.subject === subject) this.update(state);
      },
      (error: unknown) => {
        this.looking = false;
        this.why = error instanceof Error ? error.message : String(error);
        this.said = this.words.failed(this.why);
        this.draw();
      },
    );
  }

  private pick(version: string | null): void {
    const purpose = this.purpose;
    if (!purpose || this.picking !== null) return;
    this.picking = version ?? "";
    this.said = "";
    this.draw();
    const picked = purpose.kind === "minimum" ? purpose.pick(version) : version ? purpose.pick(version) : Promise.resolve();
    picked.then(
      () => {
        this.picking = null;
        this.close();
      },
      (error: unknown) => {
        this.picking = null;
        this.why = error instanceof Error ? error.message : String(error);
        this.said = this.words.failed(this.why);
        this.draw();
      },
    );
  }

  private draw(): void {
    const { sheet, subject, purpose, words } = this;
    if (!sheet || !subject || !purpose) return;
    const { state } = subject;
    const rows = rowsOf(subject).filter((row) => this.filter === "all" || (this.filter === "test") === row.test);
    const minimum = purpose.kind === "minimum" ? purpose.current : null;
    // The one to press is what an automatic update there would take: the
    // newest on its own line that does not pass the hubs' major version.
    const first = rows.find((row) => row.takes && !row.pastHub && (state.channel === "beta" || !row.test))?.version;
    const unreachable = state.looked.github === false || !state.looked.releases?.length;
    const filters = (["all", "stable", "test"] as const)
      .map(
        (filter) =>
          `<button type="button" class="choice${filter === this.filter ? " chosen" : ""}" role="radio" aria-checked="${filter === this.filter}" data-vw-filter="${filter}">${escape(words.filter[filter])}</button>`,
      )
      .join("");
    const facts = (row: Row) =>
      [row.day ? words.day(row.day) : "", row.size ? words.size(row.size) : ""].filter(Boolean).join(" ");
    const act = (row: Row): string => {
      if (purpose.kind === "minimum") {
        if (row.version === minimum) return `<span class="vw-now">${escape(words.asked)}</span>`;
        return `<button class="vw-go" type="button" data-vw-pick="${escape(row.version)}"${this.picking !== null ? " disabled" : ""}>${escape(words.ask)}</button>`;
      }
      if (row.version === subject.running) return `<span class="vw-now">${escape(words.running)}</span>`;
      if (!row.newer) return "";
      if (!row.takes) return `<span class="vw-none">${escape(words.unavailable)}</span>`;
      const picking = this.picking === row.version;
      return `<button class="vw-go${row.version === first ? " first" : ""}" type="button" data-vw-pick="${escape(row.version)}"${this.picking !== null ? " disabled" : ""}>${escape(picking ? words.picking : words.pick)}</button>`;
    };
    const list = rows.length
      ? rows
          .map((row) => {
            const kind =
              purpose.kind === "minimum"
                ? row.version === minimum
                  ? " now"
                  : ""
                : row.version === subject.running
                  ? " now"
                  : row.newer
                    ? ""
                    : " older";
            return `<li class="vw-row${kind}">
              <span class="vw-name"><span class="vw-version">${escape(row.version)}</span>${row.test ? `<span class="tag">${escape(words.test)}</span>` : ""}${
                row.pastHub && row.newer ? `<span class="tag vw-past" title="${escape(words.pastHubWhy)}">${escape(words.pastHub)}</span>` : ""
              }</span>
              <span class="vw-facts">${escape(facts(row))}</span>
              <span class="vw-act">${act(row)}</span>
            </li>`;
          })
          .join("")
      : `<li class="vw-empty">${escape(words.none)}</li>`;
    const looked = this.looking
      ? words.looking
      : unreachable
        ? words.unreachable
        : state.looked.at
          ? words.looked(state.looked.at)
          : "";
    const clear =
      purpose.kind === "minimum" && minimum
        ? `<button class="text-button quiet-button" type="button" data-vw-pick=""${this.picking !== null ? " disabled" : ""}>${escape(words.askNone)}</button>`
        : "";
    const html = `${sheet.head(purpose.kind === "minimum" ? words.minimumTitle : words.title, words.close)}
      <p class="vw-running">${escape(purpose.kind === "minimum" ? words.minimumNow(minimum) : (subject.said ?? words.runs(subject.who, subject.running)))}</p>
      <div class="vw-tools">
        <div class="choice-options" role="radiogroup" aria-label="${escape(words.filterLabel)}">${filters}</div>
        <span class="vw-looked"><span>${escape(looked)}</span>${subject.look ? `<button class="text-button quiet-button" type="button" data-vw-look${this.looking ? " disabled" : ""}>${escape(words.lookAgain)}</button>` : ""}</span>
      </div>
      <ol class="vw-list">${list}</ol>
      ${clear ? `<div class="vw-foot">${clear}</div>` : ""}
      <p class="sheet-note" role="alert"${this.said ? "" : " hidden"}${this.said && this.why ? ` title="${escape(this.why)}"` : ""}>${escape(this.said)}</p>`;
    if (html === this.drawn) return;
    this.drawn = html;
    sheet.body.innerHTML = html;
  }
}
