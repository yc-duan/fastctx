// The activity page (`gui-activity-page`): the files agents changed on the
// user's machines, and no more than that (the user, 2026-11-16). It is read
// as a list of changed files is read everywhere: a file's path, the lines
// added to it in green and removed from it in red, five squares for how
// they divide; a mark before the path says whether the file is new,
// changed, gone or a copy. The files of a machine stand together under its
// name, in the order of their paths. A file pressed opens under itself to
// each of its changes: when, by which session, how many lines.
//
// Every machine keeps its own book of what `run` did there; the page asks
// each it can reach and puts their answers together, saying which did not
// answer. This computer's is read again whenever it changes, the others'
// now and then.

import { BOOK, followState } from "../world/topics";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Page } from "../app/shell";
import type { BookAnswer, BookQuery, TouchHow } from "../world/book";
import { agentName } from "../world/names";
import { DAY_MS, changedFiles, dayOf, squares, type Change, type FileChanged, type MachineChanged, type Placed } from "./model";
import type { ActivityWords, Span } from "./words";
import "./activity.css";

const SPANS: readonly Span[] = ["day", "week", "month", "quarter"];
const SPAN_DAYS: Record<Span, number> = { day: 1, week: 7, month: 30, quarter: 90 };
const KEPT_SPAN = "activity.span";
/** How often this computer's book is read again at most, as it changes; and
 *  how often the other machines are asked. */
const HERE_MS = 4000;
const THERE_MS = 30_000;
/** How long the answer from an offline machine's copy stands. */
const COPY_AGAIN_MS = 60_000;
const LIMIT = 2000;
/** The changes an opened file lists, the latest. */
const LISTED = 10;

export interface BookMachine {
  /** None for this computer. */
  written: string | null;
  name: string;
  online: boolean;
}

export interface ActivityOptions {
  words: ActivityWords;
  head(title: string, actions?: string): string;
  machines(): BookMachine[];
  read(machine: string | null, query: BookQuery): Promise<BookAnswer>;
  /** What a session (`resident@machine`) is called here and where it is
   *  opened, where this device knows it as a conversation. */
  named?(session: string): { name: string; link: string } | null;
  /** The page's own clock, for the specimen. */
  now?(): number;
}

const figure = (n: number) => n.toLocaleString("en-US");

/** The lines added and removed, as they are written beside a file. */
function figures(added: number, removed: number): string {
  return `<span class="ay-figures">${added ? `<span class="ay-add">+${figure(added)}</span>` : ""}${removed ? `<span class="ay-del">−${figure(removed)}</span>` : ""}</span>`;
}

/** The mark before a path: a small square holding a plus for a new file, a
 *  dot for a changed one, a minus for one gone, an arrow for a copy. */
function mark(how: TouchHow, said: string): string {
  const inside =
    how === "added" ? `<path d="M8 4.75v6.5M4.75 8h6.5"/>` : how === "removed" ? `<path d="M4.75 8h6.5"/>` : how === "copied" ? `<path d="M4.75 8h6M8.5 5.5 11 8l-2.5 2.5"/>` : `<circle cx="8" cy="8" r="1.6" class="ay-dot"/>`;
  return `<svg class="ay-mark ay-${how}" viewBox="0 0 16 16" width="16" height="16" role="img" aria-label="${escape(said)}"><title>${escape(said)}</title><rect x="1.5" y="1.5" width="13" height="13" rx="3.5"/>${inside}</svg>`;
}

export class ActivityPage implements Page {
  readonly element = document.createElement("section");
  readonly title: string;
  private span: Span = kept(KEPT_SPAN, SPANS, "week");
  private deeds: Placed[] = [];
  /** Machines asked that did not answer, and whether some machine left
   *  runs out past the limit. */
  private away: string[] = [];
  private more = false;
  /** The files opened, each its machine and path. */
  private readonly opened = new Set<string>();
  /** A file the address asks to be opened, in a development build: for
   *  looking at an opened file without a hand to press it. */
  private wanted = import.meta.env.DEV ? new URLSearchParams(location.search).get("open") : null;
  /** What the page shows, as last written. */
  private written = "";
  private readonly body: HTMLElement;
  private readonly switcher: HTMLElement;
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
    this.element.innerHTML = `${options.head(escape(words.title), `<div class="segmented ay-spans" role="radiogroup" aria-label="${escape(words.spanLabel)}"></div>`)}<div class="ay-body scroller"></div>`;
    this.switcher = this.element.querySelector<HTMLElement>(".ay-spans")!;
    this.body = this.element.querySelector<HTMLElement>(".ay-body")!;
    this.drawSwitcher();
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
      const { answers, away } = await this.askAll({ ask: "deeds", from: new Date(this.from()).toISOString(), to: new Date(this.now() + 60_000).toISOString(), limit: LIMIT }, group);
      if (this.left) return;
      if (span !== this.span) {
        this.again.add(group);
        return;
      }
      const deeds: Placed[] = [];
      let more = false;
      for (const answer of answers) {
        if (answer.kind !== "deeds") continue;
        more ||= answer.more;
        for (const deed of answer.deeds) deeds.push({ ...deed, machine: answer.machine });
      }
      this.deeds = deeds;
      this.away = away;
      this.more = more;
      this.draw();
    } finally {
      this.reading.delete(group);
      if (this.again.delete(group) && !this.left) void this.read(group);
    }
  }

  private drawSwitcher(): void {
    const { words } = this.options;
    this.switcher.innerHTML = SPANS.map((span) => `<button type="button" class="segment${span === this.span ? " chosen" : ""}" role="radio" aria-checked="${span === this.span}" data-span="${span}">${escape(words.spans[span])}</button>`).join("");
  }

  /** Draws the files changed. Drawn again only where it differs from what
   *  is shown, so a reading that changed nothing moves nothing; `anew` says
   *  the reader asked for another span, and what comes fades in. */
  private draw(anew = false): void {
    const { words } = this.options;
    const machines = changedFiles(this.deeds, this.from());
    const asked = this.wanted && machines.flatMap((machine) => machine.files).find((file) => file.name === this.wanted);
    if (asked) {
      this.opened.add(`${asked.machine}\n${asked.path}`);
      this.wanted = null;
    }
    const count = machines.reduce((sum, machine) => sum + machine.files.length, 0);
    const added = machines.reduce((sum, machine) => sum + machine.added, 0);
    const removed = machines.reduce((sum, machine) => sum + machine.removed, 0);
    const notes = [this.more ? words.truncated(LIMIT) : "", this.away.length ? words.unanswered(this.away.join("、")) : ""].filter(Boolean);
    const foot = notes.length ? `<p class="ay-notes">${notes.map((note) => `<span>${escape(note)}</span>`).join("")}</p>` : "";
    const html = count
      ? `<div class="ay-sheet"><p class="ay-summary"><b>${escape(words.changed(count))}</b>${figures(added, removed)}</p>${machines.map((machine) => this.machine(machine)).join("")}${foot}</div>`
      : `<div class="ay-sheet ay-quiet"><p>${escape(words.quiet)}</p>${foot}</div>`;
    if (html === this.written && !anew) return;
    this.written = html;
    const kept = this.body.scrollTop;
    this.body.innerHTML = html;
    this.body.scrollTop = anew ? 0 : kept;
    if (anew) {
      this.body.classList.remove("ay-in");
      void this.body.offsetWidth;
      this.body.classList.add("ay-in");
    }
  }

  /** A machine's files, under its name. */
  private machine(machine: MachineChanged): string {
    const { words } = this.options;
    return `<section class="ay-machine">
      <header class="ay-machine-head">${glyph("desktop", 18)}<b>${escape(machine.machine)}</b><small>${escape(words.files(machine.files.length))}</small>${figures(machine.added, machine.removed)}</header>
      <ul class="ay-files">${machine.files.map((file) => this.file(file)).join("")}</ul>
    </section>`;
  }

  private file(file: FileChanged): string {
    const { words } = this.options;
    const key = `${file.machine}\n${file.path}`;
    const open = this.opened.has(key);
    const blocks = squares(file.added, file.removed)
      .map((kind) => `<i class="${kind}"></i>`)
      .join("");
    return `<li class="ay-file${open ? " open" : ""}">
      <div class="ay-row" role="button" tabindex="0" aria-expanded="${open}" data-file="${escape(key)}">
        ${mark(file.how, words.how[file.how])}
        <span class="ay-path" title="${escape(file.path)}"><span class="ay-folder"><bdi>${escape(file.folder)}</bdi></span><b>${escape(file.name)}</b></span>
        ${figures(file.added, file.removed)}
        <span class="ay-squares" aria-hidden="true">${blocks}</span>
      </div>
      <div class="ay-fold"><div class="ay-fold-inner"><ul class="ay-changes">${file.changes.slice(0, LISTED).map((change) => this.change(change)).join("")}${file.changes.length > LISTED ? `<li class="ay-earlier">${escape(words.earlier(file.changes.length - LISTED))}</li>` : ""}</ul></div></div>
    </li>`;
  }

  /** One change of a file: when, by which session, how, how many lines. */
  private change(change: Change): string {
    const { words } = this.options;
    const { deed } = change;
    const known = deed.session ? (this.options.named?.(deed.session) ?? null) : null;
    const agent = deed.agent ? agentName(deed.agent) : "";
    const name = known?.name ?? (agent ? words.own(agent) : (deed.session?.split("@")[0] ?? ""));
    const who = known ? `<a href="${escape(known.link)}">${escape(name)}</a>` : escape(name);
    const from = deed.from ? `<small>${escape(words.from(deed.from))}</small>` : "";
    return `<li>
      <time>${escape(words.stamp(change.at, this.now()))}</time>
      <span class="ay-who">${deed.agent ? glyph(deed.agent, 16) : ""}<span>${who}</span>${from}</span>
      <span class="ay-how"${change.seen ? ` title="${escape(words.seen)}"` : ""}>${change.how === "changed" ? "" : escape(words.how[change.how])}</span>
      ${figures(change.added, change.removed)}
    </li>`;
  }

  private pressed(event: MouseEvent): void {
    const target = event.target as HTMLElement;
    const span = target.closest<HTMLElement>("[data-span]")?.dataset.span as Span | undefined;
    if (span) {
      if (span === this.span) return;
      this.span = span;
      keep(KEPT_SPAN, span);
      this.drawSwitcher();
      // What is read of the span chosen before is not this one's.
      this.deeds = [];
      this.written = "";
      void Promise.all([this.read("local"), this.read("remote")]).then(() => this.draw(true));
      return;
    }
    // A link in an opened file goes where it says; the row is not pressed.
    if (target.closest("a")) return;
    const row = target.closest<HTMLElement>("[data-file]");
    if (!row) return;
    const key = row.dataset.file!;
    const open = !this.opened.has(key);
    if (open) this.opened.add(key);
    else this.opened.delete(key);
    row.setAttribute("aria-expanded", String(open));
    row.parentElement!.classList.toggle("open", open);
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
