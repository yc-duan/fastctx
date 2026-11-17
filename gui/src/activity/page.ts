// The activity page (`gui-activity-page`): the files agents changed on the
// user's machines, and no more than that (the user, 2026-11-16). It is read
// as a list of changed files is read everywhere: a file's path, the lines
// added to it in green and removed from it in red, five squares for how
// they divide; a mark before the path says whether the file is new,
// changed, gone or a copy.
//
// Who changed them is the page's other half: the files stand under the
// conversation that changed them, its agent's mark before its name and the
// machines it changed them on after, the conversation that changed
// something last first; or, by the switch in the head, under the machine
// holding them, each row then saying whose it is. A file pressed opens
// under itself to its changes, each under who made it and when, its lines
// a row to a line; from there the file opens among its machine's folders.
//
// Every machine keeps its own book of what `run` did there; the page asks
// each it can reach and puts their answers together, saying which did not
// answer. This computer's is read again whenever it changes, the others'
// now and then.

import { BOOK, followState } from "../world/topics";
import { agentGlyph, glyph } from "../conversation/glyphs";
import type { DiffRow } from "../conversation/diff";
import { escape } from "../conversation/prose";
import type { Page } from "../app/shell";
import { waiting } from "../app/waiting";
import type { BookAnswer, BookQuery } from "../world/book";
import { bookChanges, type RowsOf } from "../files/book";
import { changeLine, diffLines, figures, fileRow, sessionName, type Named } from "../files/changed";
import { byMachine, byTalk, filesOf, tally, type ChangedFile, type FileChange, type MachineGroup, type TalkGroup } from "../files/model";
import type { ViewTab } from "../files/view";
import type { FilesWords } from "../files/words";
import type { ActivityWords, Grouping, Span } from "./words";
import "../files/files.css";
import "./activity.css";

const SPANS: readonly Span[] = ["day", "week", "month", "quarter"];
const SPAN_DAYS: Record<Span, number> = { day: 1, week: 7, month: 30, quarter: 90 };
const GROUPINGS: readonly Grouping[] = ["talk", "machine"];
const KEPT_SPAN = "activity.span";
const KEPT_GROUPING = "activity.grouping";
const DAY_MS = 24 * 3_600_000;
/** How often this computer's book is read again at most, as it changes; and
 *  how often the other machines are asked. */
const HERE_MS = 4000;
const THERE_MS = 30_000;
/** How long the answer from an offline machine's copy stands. */
const COPY_AGAIN_MS = 60_000;
const LIMIT = 2000;
/** The files a group lists, and the groups the page lists, before the rest
 *  are asked for. */
const FEW = 12;
const GROUPS = 40;
/** The changes an opened file shows, the latest, and the lines of each. */
const LISTED = 5;
const LINES = 120;

export interface BookMachine {
  /** None for this computer. */
  written: string | null;
  name: string;
  online: boolean;
}

export interface ActivityOptions {
  words: ActivityWords;
  files: FilesWords;
  head(title: string, actions?: string): string;
  machines(): BookMachine[];
  read(machine: string | null, query: BookQuery): Promise<BookAnswer>;
  /** What a session (`resident@machine`) is called here and where it is
   *  opened, where this device knows it as a conversation. */
  named?: Named;
  /** The lines of one change, where its machine kept them. */
  rows?: RowsOf;
  /** Where a file is opened among its machine's folders. */
  link(machine: string, path: string | null, tab: ViewTab | null): string;
  /** The page's own clock, for the specimen. */
  now?(): number;
  /** Makes a Codex session the user opened on their own on this computer a
   *  conversation of Fastctx's; none where the workbench is not open. */
  bringIn?: ((resident: string) => Promise<void>) | undefined;
}

/** Local midnight of the day holding `at`. */
function dayOf(at: number): number {
  const date = new Date(at);
  date.setHours(0, 0, 0, 0);
  return date.getTime();
}

export class ActivityPage implements Page {
  readonly element = document.createElement("section");
  /** The Codex sessions of this computer's own, as the book names them,
   *  that the World holds not; and those being brought in now. */
  private readonly unheld = new Set<string>();
  private readonly bringing = new Set<string>();
  readonly title: string;
  private span: Span = kept(KEPT_SPAN, SPANS, "week");
  private grouping: Grouping = kept(KEPT_GROUPING, GROUPINGS, "talk");
  private changes: FileChange[] = [];

  /** The files opened, each its group and its own key; the groups whose
   *  files are all listed; whether every group is. */
  private readonly opened = new Set<string>();
  private readonly whole = new Set<string>();
  private allGroups = false;
  /** The lines of the changes read, by the changes' ids. */
  private readonly lines = new Map<string, DiffRow[] | null | "reading">();
  /** The changes of opened files folded away or opened by hand (the user,
   *  2026-11-17: "这内部的一次次修改，都要支持前端上的展开和收起"); one
   *  not touched stands open where it is a file's latest, else shut. */
  private readonly changeFolds = new Map<string, boolean>();
  /** The files as last drawn, by what their rows are opened by. */
  private drawnFiles = new Map<string, ChangedFile>();
  /** A file the address asks to be opened, in a development build: for
   *  looking at an opened file without a hand to press it. */
  private wanted = import.meta.env.DEV ? new URLSearchParams(location.search).get("open") : null;
  /** What the page shows, as last written. */
  private written = "";
  private readonly body: HTMLElement;
  private readonly subscriptions: (() => void)[] = [];
  private readonly answers = new Map<string, Map<string, BookAnswer | null>>();
  private localFailed = false;
  /** Local and remote books advance independently; each group has one read. */
  private readonly reading = new Set<string>();
  private readonly again = new Set<string>();
  /** When each offline machine's copy was last asked, by window. */
  private readonly copiedAt = new Map<string, number>();
  private left = false;

  constructor(private readonly options: ActivityOptions) {
    const { words } = options;
    this.title = words.title;
    this.element.className = "page ay-page";
    this.element.innerHTML = `${options.head(
      escape(words.title),
      `<div class="segmented ay-groupings" role="radiogroup" aria-label="${escape(words.groupingLabel)}"></div><div class="segmented ay-spans" role="radiogroup" aria-label="${escape(words.spanLabel)}"></div>`,
    )}<div class="ay-body scroller"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".ay-body")!;
    this.drawSwitches();
    this.element.addEventListener("click", (event) => this.pressed(event));
    this.element.addEventListener("keydown", (event) => {
      const target = event.target as HTMLElement;
      if ((event.key === "Enter" || event.key === " ") && target.matches("[role='button']")) {
        event.preventDefault();
        target.click();
      }
    });
    this.subscriptions.push(
      followState(BOOK, async () => { await this.read("local"); if (this.localFailed) throw new Error("the local book did not answer"); }, HERE_MS),
      followState(null, () => this.read("remote"), THERE_MS),
    );
  }

  leave(): void {
    this.left = true;
    for (const stop of this.subscriptions) stop();
  }

  private now(): number {
    return this.options.now?.() ?? Date.now();
  }

  /** From when the span shown runs: the midnight its first day began at. */
  private from(): number {
    return dayOf(this.now()) - (SPAN_DAYS[this.span] - 1) * DAY_MS;
  }

  /** Every machine's answer to `query`, the ones that did not answer named. */
  private async askAll(query: BookQuery, only: "local" | "remote"): Promise<{ answers: BookAnswer[]; away: string[] }> {
    const all = this.options.machines();
    const machines = all.filter((machine) => (machine.written === null) === (only === "local"));
    // Keep the other machines' answers when only the local book changed.
    const key = query.from;
    let kept = this.answers.get(key);
    if (!kept) {
      kept = new Map();
      this.answers.set(key, kept);
      if (this.answers.size > 8) this.answers.delete(this.answers.keys().next().value!);
    }
    const known = kept;
    // A machine offline is asked too: this computer's node answers from the
    // copy of its activity it keeps, where it has one. That copy grows only
    // as what the machine sent before it went comes through, so it is asked
    // again for a window no oftener than `COPY_AGAIN_MS`.
    const now = Date.now();
    const asked = await Promise.allSettled(
      machines.map((machine) => {
        const away = machine.written !== null && !machine.online;
        const copied = away && now - (this.copiedAt.get(`${key}\n${machine.name}`) ?? 0) < COPY_AGAIN_MS ? known.get(machine.name) : undefined;
        if (copied) return Promise.resolve(copied);
        const reading = this.options.read(machine.written, query);
        if (away) void reading.then(() => this.copiedAt.set(`${key}\n${machine.name}`, now), () => undefined);
        return reading;
      }),
    );
    if (only === "local") this.localFailed = asked.some((result) => result.status === "rejected");
    for (const name of kept.keys()) if (!all.some((machine) => machine.name === name)) kept.delete(name);
    asked.forEach((result, index) => known.set(machines[index]!.name, result.status === "fulfilled" ? result.value : null));
    const answers = [...kept.values()].filter((answer): answer is BookAnswer => answer !== null);
    const away = [...kept.entries()].filter(([, answer]) => answer === null).map(([name]) => name);
    return { answers, away };
  }

  /** Reads what ran over the span. One reading at a time for this computer
   *  and one for the others: a machine slower than the next reading still
   *  has its answer drawn. One wanted meanwhile follows when it ends; only
   *  a reading of a span no longer chosen is let go. */
  private async read(group: "local" | "remote"): Promise<void> {
    if (this.reading.has(group)) {
      this.again.add(group);
      return;
    }
    this.reading.add(group);
    const span = this.span;
    try {
      const { answers } = await this.askAll({ ask: "deeds", from: new Date(this.from()).toISOString(), to: new Date(this.now() + 60_000).toISOString(), limit: LIMIT }, group);
      if (this.left) return;
      if (span !== this.span) {
        this.again.add(group);
        return;
      }
      const changes: FileChange[] = [];
      const from = this.from();
      for (const answer of answers) {
        if (answer.kind !== "deeds") continue;
        changes.push(...bookChanges(answer, from, this.options.rows));
        // The Codex sessions of this computer's own the World holds not:
        // each a press from being brought in.
        const here = this.options.machines().find((machine) => machine.written === null)?.name;
        if (answer.machine === here)
          for (const deed of answer.deeds) if (!deed.held && deed.agent === "codex" && deed.session?.endsWith(`@${here}`)) this.unheld.add(deed.session);
      }
      this.changes = changes;
      this.draw();
    } finally {
      this.reading.delete(group);
      if (this.again.delete(group) && !this.left) void this.read(group);
    }
  }

  private drawSwitches(): void {
    const { words } = this.options;
    const segment = (name: string, value: string, chosen: boolean, said: string) => `<button type="button" class="segment${chosen ? " chosen" : ""}" role="radio" aria-checked="${chosen}" data-${name}="${value}">${escape(said)}</button>`;
    this.element.querySelector<HTMLElement>(".ay-spans")!.innerHTML = SPANS.map((span) => segment("span", span, span === this.span, words.spans[span])).join("");
    this.element.querySelector<HTMLElement>(".ay-groupings")!.innerHTML = GROUPINGS.map((grouping) => segment("grouping", grouping, grouping === this.grouping, words.groupings[grouping])).join("");
  }

  /** Draws the files changed. Drawn again only where it differs from what
   *  is shown, so a reading that changed nothing moves nothing; `anew` says
   *  the reader asked for another look, and what comes fades in. */
  private draw(anew = false): void {
    const { words, files } = this.options;
    const groups: (TalkGroup | MachineGroup)[] = this.grouping === "talk" ? byTalk(this.changes) : byMachine(this.changes);
    this.drawnFiles = new Map(groups.flatMap((group) => group.files.map((file): [string, ChangedFile] => [`${group.key}\n${file.key}`, file])));
    if (this.wanted) {
      const asked = [...this.drawnFiles].find(([, file]) => file.name === this.wanted);
      if (asked) {
        this.opened.add(asked[0]);
        this.wanted = null;
      }
    }
    const sum = tally(filesOf(this.changes));

    const shown = this.allGroups ? groups : groups.slice(0, GROUPS);
    const rest = groups.length > shown.length ? `<button type="button" class="text-button ay-rest" data-groups>${escape(words.moreGroups(groups.length - shown.length))}</button>` : "";
    const html = sum.files
      ? `<div class="ay-sheet"><p class="ay-summary"><b>${escape(files.changed(sum.files))}</b>${figures(sum.added, sum.removed)}</p>${shown.map((group) => this.group(group)).join("")}${rest}</div>`
      : `<div class="ay-sheet ay-quiet"><p>${escape(words.quiet)}</p></div>`;
    if (html === this.written && !anew) return;
    this.written = html;
    const kept = this.body.scrollTop;
    this.body.innerHTML = html;
    this.body.scrollTop = anew ? 0 : kept;
    // An opened file's changes not yet read are read now.
    for (const key of this.opened) if (this.drawnFiles.has(key)) this.readLines(key);
    if (anew) {
      this.body.classList.remove("ay-in");
      void this.body.offsetWidth;
      this.body.classList.add("ay-in");
    }
  }

  /** A group's files under its name: a conversation's, or a machine's. */
  private group(group: TalkGroup | MachineGroup): string {
    const { words, files, named } = this.options;
    const talk = "machines" in group;
    let name: string;
    if (talk) {
      const said = sessionName(group, files, named);
      const resident = group.session?.slice(0, group.session.lastIndexOf("@")) ?? "";
      const bring =
        this.options.bringIn && group.session && this.unheld.has(group.session)
          ? `<button type="button" class="text-button ay-bring" data-bring="${escape(resident)}" title="${escape(words.bringInWhy)}"${this.bringing.has(resident) ? " disabled" : ""}>${escape(words.bringIn)}</button>`
          : "";
      name = `${group.agent ? agentGlyph(group.agent, 18) : glyph("agents", 18)}${said.link ? `<a class="ay-name" href="${escape(said.link)}">${escape(said.name)}</a>` : `<b class="ay-name">${escape(said.name)}</b>`}<span class="ay-on">${glyph("machine", 14)}${escape(group.machines.join("、"))}</span>${bring}`;
    } else {
      name = `${glyph("desktop", 18)}<a class="ay-name" href="${escape(this.options.link(group.machine, null, null))}" title="${escape(words.folders)}">${escape(group.machine)}</a>`;
    }
    const all = this.whole.has(group.key) || group.files.length <= FEW + 1;
    const listed = all ? group.files : group.files.slice(0, FEW);
    const rows = listed.map((file) => this.file(group, file, talk)).join("");
    const rest = all ? "" : `<li class="cf-rest"><button type="button" class="text-button" data-whole="${escape(group.key)}">${escape(files.moreFiles(group.files.length - listed.length))}</button></li>`;
    return `<section class="ay-group">
      <header class="ay-group-head">${name}<small>${escape(files.files(group.files.length))}</small>${figures(group.added, group.removed)}<time>${escape(files.stamp(group.last, this.now()))}</time></header>
      <ul class="cf-files">${rows}${rest}</ul>
    </section>`;
  }

  private file(group: TalkGroup | MachineGroup, file: ChangedFile, talk: boolean): string {
    const { files, named } = this.options;
    const key = `${group.key}\n${file.key}`;
    const open = this.opened.has(key);
    return fileRow(file, files, {
      attrs: `data-file="${escape(key)}"`,
      named,
      who: !talk,
      where: talk && (group as TalkGroup).machines.length > 1,
      open,
      fold: open ? this.fold(file, talk) : "",
    });
  }

  /** What an opened file shows: its latest changes, each under who made
   *  it and when, and the way to the file among its folders. */
  private fold(file: ChangedFile, talk: boolean): string {
    const { words, files, named } = this.options;
    const now = this.now();
    const listed = file.changes.slice(0, LISTED);
    const changes = listed
      .map((change, index) => {
        const line = changeLine(talk ? { ...change, session: null, agent: null, from: null } : change, files, named, now);
        // A change with no lines kept is its own line alone; any other
        // folds its lines away under that line, and opens them again.
        if (!change.rows) return `<section class="ay-change bare" data-change="${escape(change.id)}"><div class="ay-change-head">${line}</div></section>`;
        const open = this.changeOpen(change.id, index);
        return `<section class="ay-change${open ? " open" : ""}" data-change="${escape(change.id)}"><div class="ay-change-head" role="button" tabindex="0" aria-expanded="${open}">${line}${glyph("chevron", 16)}</div><div class="ay-change-fold"><div class="ay-change-body">${open ? this.linesOf(change, file.path) : ""}</div></div></section>`;
      })
      .join("");
    const earlier = file.changes.length > LISTED ? `<span>${escape(files.view.earlier(file.changes.length - LISTED))}</span>` : "";
    return `${changes}<p class="ay-fold-foot">${earlier}<a href="${escape(this.options.link(file.machine, file.path, file.how === "removed" ? "changes" : "content"))}">${escape(words.openFile)}</a><a href="${escape(this.options.link(file.machine, file.path, "changes"))}">${escape(words.allChanges)}</a></p>`;
  }

  /** Whether a change of an opened file shows its lines: as the user left
   *  it, else only the file's latest. */
  private changeOpen(id: string, index: number): boolean {
    return this.changeFolds.get(id) ?? index === 0;
  }

  /** A change's lines as they are held: drawn, being read, or not kept. */
  private linesOf(change: FileChange, path: string): string {
    const { files } = this.options;
    const held = this.lines.get(change.id);
    // What has no lines kept is its own line alone, as a list of changes.
    if (!change.rows) return "";
    if (held === undefined || held === "reading") return waiting({ label: files.view.reading, shape: "lines", rows: Math.min(6, Math.max(2, change.added + change.removed)) });
    if (held === null || !held.length) return `<p class="cf-note">${escape(change.uncounted ? files.view.uncounted : files.view.notKept)}</p>`;
    return diffLines(held, path, files, LINES);
  }

  /** Reads the lines of an opened file's changes that were not read yet,
   *  each drawn where it stands as it comes. */
  private readLines(key: string): void {
    const file = this.drawnFiles.get(key);
    if (!file) return;
    for (const [index, change] of file.changes.slice(0, LISTED).entries()) {
      // Lines folded away are not read until they are opened.
      if (!change.rows || this.lines.has(change.id) || !this.changeOpen(change.id, index)) continue;
      this.lines.set(change.id, "reading");
      void change.rows().then((rows) => rows, () => null).then((rows) => {
        if (this.left) return;
        this.lines.set(change.id, rows);
        if (this.lines.size > 400) this.lines.delete(this.lines.keys().next().value!);
        for (const body of this.body.querySelectorAll<HTMLElement>(".ay-change")) {
          if (body.dataset.change === change.id) body.querySelector<HTMLElement>(".ay-change-body")!.innerHTML = this.linesOf(change, file.path);
        }
        // What is shown is no longer what was written.
        this.written = "";
      });
    }
  }

  private pressed(event: MouseEvent): void {
    const target = event.target as HTMLElement;
    const span = target.closest<HTMLElement>("[data-span]")?.dataset.span as Span | undefined;
    if (span) {
      if (span === this.span) return;
      this.span = span;
      keep(KEPT_SPAN, span);
      this.drawSwitches();
      // What is read of the span chosen before is not this one's.
      this.changes = [];
      this.written = "";
      void Promise.all([this.read("local"), this.read("remote")]).then(() => this.draw(true));
      return;
    }
    const grouping = target.closest<HTMLElement>("[data-grouping]")?.dataset.grouping as Grouping | undefined;
    if (grouping) {
      if (grouping === this.grouping) return;
      this.grouping = grouping;
      keep(KEPT_GROUPING, grouping);
      this.drawSwitches();
      this.opened.clear();
      this.whole.clear();
      this.allGroups = false;
      this.draw(true);
      return;
    }
    const bring = target.closest<HTMLElement>("[data-bring]")?.dataset.bring;
    if (bring && this.options.bringIn) {
      this.bringing.add(bring);
      this.draw();
      void this.options.bringIn(bring).then(
        () => {
          this.unheld.delete(`${bring}@${this.options.machines().find((machine) => machine.written === null)?.name ?? ""}`);
          this.bringing.delete(bring);
          this.draw();
        },
        (error: unknown) => {
          this.bringing.delete(bring);
          console.error("bringing a session in", error);
          this.draw();
        },
      );
      return;
    }
    const whole = target.closest<HTMLElement>("[data-whole]")?.dataset.whole;
    if (whole !== undefined) {
      this.whole.add(whole);
      this.draw();
      return;
    }
    if (target.closest("[data-groups]")) {
      this.allGroups = true;
      this.draw();
      return;
    }
    // A link goes where it says; the row is not pressed.
    if (target.closest("a")) return;
    // A change's own line folds its lines away, or opens them, where it
    // stands; opened, its lines are read if they were not yet.
    const head = target.closest<HTMLElement>(".ay-change-head[role='button']");
    if (head) {
      const section = head.parentElement!;
      const id = section.dataset.change!;
      const key = section.closest(".cf-file")?.querySelector<HTMLElement>("[data-file]")?.dataset.file ?? "";
      const file = this.drawnFiles.get(key);
      const open = !section.classList.contains("open");
      this.changeFolds.set(id, open);
      head.setAttribute("aria-expanded", String(open));
      const change = file?.changes.find((one) => one.id === id);
      if (open && file && change) {
        section.querySelector<HTMLElement>(".ay-change-body")!.innerHTML = this.linesOf(change, file.path);
        this.readLines(key);
      }
      // Measured shut before it opens, so it is seen to open.
      void section.offsetHeight;
      section.classList.toggle("open", open);
      this.written = "";
      return;
    }
    const row = target.closest<HTMLElement>("[data-file]");
    if (!row) return;
    const key = row.dataset.file!;
    const file = this.drawnFiles.get(key);
    if (!file) return;
    const open = !this.opened.has(key);
    if (open) this.opened.add(key);
    else this.opened.delete(key);
    row.setAttribute("aria-expanded", String(open));
    const item = row.parentElement!;
    const inner = item.querySelector<HTMLElement>(".cf-fold-inner")!;
    if (open && !inner.innerHTML.trim()) {
      inner.innerHTML = this.fold(file, this.grouping === "talk");
      this.readLines(key);
    }
    // Measured shut before it opens, so it is seen to open.
    void item.offsetHeight;
    item.classList.toggle("open", open);
    // What is shown is no longer what was written: the next reading that
    // differs draws it with the file as it stands now.
    this.written = "";
  }
}

function kept<T extends string>(key: string, among: readonly T[], otherwise: T): T {
  try {
    const value = localStorage.getItem(key) as T | null;
    return value && among.includes(value) ? value : otherwise;
  } catch {
    return otherwise;
  }
}

function keep(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Kept for this visit only.
  }
}
