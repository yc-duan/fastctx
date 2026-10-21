// A login's requests, one row each, the newest first (`world-usage.md`,
// 调用记录; `world-gui.md`, 智能体页): the usage page's fifth view. Each row
// is when, whose, which model, what it was made of as a small bar in the
// kinds' inks, what it cost and how long it took; what went wrong is a mark,
// and the rest is said while the pointer rests on it. A row of the reader's
// own conversation opens that turn. The rows are those the login's machine
// lets the reader see, already narrowed and with others' conversations
// taken out there, a hundred to a page: only the page read is drawn, the
// pager under it steps to older pages and back, and the first page is read
// again each minute while it is open.

import { glyph } from "../../conversation/glyphs";
import { escape } from "../../conversation/prose";
import { modelName } from "../../conversation/view";
import type { HubAccount } from "../../world/accounts";
import type { CallCursor, CallOutcome, CallSource, UsageCall, UsageCallsPage, UsageCallsQuery } from "../../world/usage-calls";
import type { ResidentId } from "../../world/wire";
import type { ActsMenu, MenuEntry } from "../acts-menu";
import { detailOf } from "../problem";
import type { CallsWords } from "./calls-words";
import type { UsageWords } from "./words";

export interface CallsOptions {
  words: CallsWords;
  usage: UsageWords;
  /** Claude Code keeps what each request cost; Codex counts tokens alone. */
  claude: boolean;
  menu: ActsMenu;
  ask(query: UsageCallsQuery): Promise<UsageCallsPage>;
  /** Opens a turn of one of the reader's own conversations. */
  open(conversation: ResidentId, turn: string | null): void;
  /** One turn's requests, as its conversation asked for them, until let go. */
  turn: { conversation: ResidentId; turn: string } | null;
  /** The turn let go: the page's address forgets it. */
  unturn(): void;
}

/** What the page knows as it draws: who is who, the one it is narrowed to,
 *  and the models of its week, offered before rows show any others. */
export interface CallsContext {
  who(account: HubAccount | null): { key: string; name: string; ink: string };
  /** More than one used the login: rows say whose they are. */
  several: boolean;
  /** The person the page is narrowed to, undefined for everyone; by the
   *  account the machine narrows to, or, where the page counts the
   *  machine's own sessions as its owner's too, by their key here. */
  person: { key: string; account: { account: HubAccount | null } | null } | undefined;
  models: string[];
  now: number;
}

type Filters = { model: string | null; source: CallSource | null; outcome: CallOutcome | null };

/** The rows a page holds, as the login's machine answers them. */
const PAGE = 100;
/** Where the machine could not narrow the rows, the pages read for one
 *  page's worth of them, at most. */
const FILL = 10;
/** A request's tokens drawn at full width, and past it: the bars are
 *  scaled by the square root of their share of it, so small requests stay
 *  visible beside a long context read from cache. */
const FULL = 400_000;

export class CallsList {
  /** The panel itself, kept across the page's drawings. */
  readonly element: HTMLElement;
  private readonly head: HTMLElement;
  private readonly list: HTMLElement;
  private readonly foot: HTMLElement;
  private context: CallsContext | null = null;
  private filters: Filters = { model: null, source: null, outcome: null };
  private turn: CallsOptions["turn"];
  /** The page drawn, and where each page up to it starts: the first at the
   *  newest row, each later one after the last row of the one before. */
  private rows: UsageCall[] = [];
  private pages: (CallCursor | null)[] = [null];
  private next: CallCursor | null = null;
  /** The query the rows are of; another one starts them over. */
  private asked = "";
  private generation = 0;
  private reading = false;
  /** The first page not read; or another page not read, this one kept. */
  private problem: string | null = null;
  private slip: string | null = null;
  private models = new Set<string>();
  private headDrawn = "";
  private rowsDrawn = "";
  /** Where the rows were scrolled, kept while the page draws itself anew. */
  private scrolled = 0;

  constructor(private readonly options: CallsOptions) {
    this.turn = options.turn;
    this.element = document.createElement("section");
    this.element.className = "ledger-panel uc";
    const { words } = options;
    this.element.innerHTML = `<div class="ledger-panel-head uc-head"></div><div class="uc-rows"></div><div class="uc-foot" hidden><span class="uc-slip"></span><span class="ledger-grow"></span><button class="uc-newest" type="button" data-uc="newest" hidden>${escape(words.newest)}</button><span class="uc-pager"><button class="icon-button uc-step" type="button" data-uc="newer" title="${escape(words.newer)}" aria-label="${escape(words.newer)}">${glyph("back", 16)}</button><span class="uc-page" aria-live="polite"></span><button class="icon-button uc-step uc-older" type="button" data-uc="older" title="${escape(words.older)}" aria-label="${escape(words.older)}">${glyph("back", 16)}</button></span></div>`;
    this.head = this.element.querySelector<HTMLElement>(".uc-head")!;
    this.list = this.element.querySelector<HTMLElement>(".uc-rows")!;
    this.foot = this.element.querySelector<HTMLElement>(".uc-foot")!;
    this.element.addEventListener("click", (event) => this.click(event));
    this.list.addEventListener("scroll", () => (this.scrolled = this.list.scrollTop), { passive: true });
  }

  /** The page drew itself: the panel is back in it, as it was. */
  placed(context: CallsContext): void {
    this.context = context;
    this.list.scrollTop = this.scrolled;
    const key = this.queryKey();
    if (key !== this.asked) this.restart(key);
    else this.draw();
  }

  /** Read again, as the page does each minute: the first page takes in
   *  what is newer, the row the reader was looking at staying where it was;
   *  a later page stays as it is, read from where it starts. */
  async again(): Promise<void> {
    if (!this.context || this.reading) return;
    // Not read the last time, or nothing yet: read from the start.
    if (this.problem || (!this.rows.length && this.pages.length === 1)) return this.restart(this.asked);
    if (this.pages.length > 1) return;
    const generation = this.generation;
    const page = await this.fetch(null).catch(() => null);
    if (!page || generation !== this.generation || this.reading || this.pages.length > 1) return;
    const held = this.anchor();
    this.rows = page.rows;
    this.next = page.next;
    this.draw();
    if (held) this.restore(held);
  }

  leave(): void {
    this.generation += 1;
    this.reading = false;
  }

  /** The rows asked for: the query, and whom they are narrowed to here. */
  private queryKey(): string {
    return JSON.stringify([this.query(null), this.context?.person?.key ?? null]);
  }

  private query(before: CallCursor | null): UsageCallsQuery {
    const { filters, turn } = this;
    const person = this.context?.person;
    return {
      ...(person?.account ? { person: person.account } : {}),
      ...(filters.model ? { model: filters.model } : {}),
      ...(filters.source ? { source: filters.source } : {}),
      ...(filters.outcome ? { outcome: filters.outcome } : {}),
      ...(turn ? { conversation: turn.conversation, turn: turn.turn } : {}),
      ...(before ? { before } : {}),
    };
  }

  private restart(key: string): void {
    this.asked = key;
    this.generation += 1;
    this.rows = [];
    this.pages = [null];
    this.next = null;
    this.problem = null;
    this.slip = null;
    this.reading = false;
    this.list.scrollTop = 0;
    this.scrolled = 0;
    this.draw();
    void this.go([null]);
  }

  /** Reads the last of the pages given and shows it from its top; the page
   *  shown stays while it is read, and stays if it could not be. */
  private async go(pages: (CallCursor | null)[]): Promise<void> {
    if (this.reading) return;
    const generation = this.generation;
    this.reading = true;
    this.slip = null;
    this.draw();
    let moved = false;
    try {
      const page = await this.fetch(pages[pages.length - 1]!);
      if (generation !== this.generation) return;
      this.pages = pages;
      this.rows = page.rows;
      this.next = page.next;
      this.problem = null;
      moved = true;
    } catch (error) {
      if (generation !== this.generation) return;
      if (this.rows.length) this.slip = detailOf(error);
      else this.problem = detailOf(error);
    } finally {
      if (generation === this.generation) {
        this.reading = false;
        this.draw();
        if (moved) this.toTop();
      }
    }
  }

  /** A page from where it starts. Where the machine could not narrow the
   *  rows, the pages after it are read on until a page's worth shows, and
   *  the next page starts after the last row kept. */
  private async fetch(before: CallCursor | null): Promise<UsageCallsPage> {
    let page = await this.options.ask(this.query(before));
    const person = this.context?.person;
    if (!person || person.account) return page;
    let kept = this.keep(page.rows);
    for (let read = 1; page.next && read < FILL && kept.length < PAGE; read += 1) {
      page = await this.options.ask(this.query(page.next));
      kept = kept.concat(this.keep(page.rows));
    }
    if (kept.length > PAGE) return { rows: kept.slice(0, PAGE), next: kept[PAGE - 1]!.cursor };
    return { rows: kept, next: page.next };
  }

  /** The page's start in view: its rows from the top, and the panel's head
   *  where the page scrolls the panel itself. */
  private toTop(): void {
    this.list.scrollTop = 0;
    this.scrolled = 0;
    const scroller = this.element.closest<HTMLElement>(".scroller");
    if (!scroller) return;
    const above = this.element.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
    if (above < 0) scroller.scrollTop += above;
  }

  /** What scrolls the rows: their own room, or the page where they have none. */
  private scroller(): HTMLElement | null {
    if (this.list.scrollHeight > this.list.clientHeight + 1) return this.list;
    return this.element.closest<HTMLElement>(".scroller");
  }

  /** The first row in view and how far down it stands. */
  private anchor(): { key: string; top: number } | null {
    const scroller = this.scroller();
    if (!scroller || scroller.scrollTop <= 0) return null;
    const edge = scroller.getBoundingClientRect().top;
    for (const row of this.list.querySelectorAll<HTMLElement>(".uc-row")) {
      const top = row.getBoundingClientRect().top;
      if (top >= edge) return { key: row.dataset.key!, top: top - edge };
    }
    return null;
  }

  private restore(held: { key: string; top: number }): void {
    const scroller = this.scroller();
    const row = this.list.querySelector<HTMLElement>(`.uc-row[data-key="${CSS.escape(held.key)}"]`);
    if (!scroller || !row) return;
    scroller.scrollTop += row.getBoundingClientRect().top - scroller.getBoundingClientRect().top - held.top;
    this.scrolled = this.list.scrollTop;
  }

  private click(event: Event): void {
    const target = (event.target as HTMLElement).closest<HTMLElement>("[data-uc], .uc-row.own");
    if (!target) return;
    if (target.classList.contains("uc-row")) {
      const row = this.rows.find((one) => one.cursor.key === target.dataset.key);
      if (row?.conversation) this.options.open(row.conversation, row.turn);
      return;
    }
    const what = target.dataset.uc;
    if (what === "newer" && this.pages.length > 1) return void this.go(this.pages.slice(0, -1));
    if (what === "older" && this.next) return void this.go([...this.pages, this.next]);
    if (what === "newest") return void this.go([null]);
    if (what === "turn") {
      this.turn = null;
      this.options.unturn();
      this.restart(this.queryKey());
      return;
    }
    if (what === "model" || what === "source" || what === "outcome") this.choose(target, what);
  }

  /** A filter's menu: each value it can take, the one it has checked. */
  private choose(anchor: HTMLElement, what: keyof Filters): void {
    const { words, claude } = this.options;
    const now = this.filters[what];
    let entries: MenuEntry[];
    if (what === "model") {
      const models = [...this.knownModels()];
      entries = [{ act: "", label: words.allModels, checked: now === null }, ...models.map((model) => ({ act: model, label: modelName(model), checked: now === model }))];
    } else if (what === "source") {
      const sources: CallSource[] = claude ? ["main", "agent", "background"] : ["main", "agent"];
      entries = [{ act: "", label: words.sources.all, checked: now === null }, ...sources.map((source) => ({ act: source, label: words.sources[source], checked: now === source }))];
    } else {
      const outcomes: CallOutcome[] = claude ? ["completed", "failed", "interrupted", "held", "unknown"] : ["completed", "failed", "interrupted", "unknown"];
      entries = [{ act: "", label: words.outcomes.all, checked: now === null }, ...outcomes.map((outcome) => ({ act: outcome, label: words.outcomes[outcome], checked: now === outcome }))];
    }
    const title = what === "model" ? words.model : what === "source" ? words.source : words.outcome;
    this.options.menu.open(anchor, title, entries, (act) => {
      this.filters = { ...this.filters, [what]: act || null };
      const key = this.queryKey();
      if (key !== this.asked) this.restart(key);
    });
  }

  /** The models of the week and of the rows read, the one filtered by
   *  among them. */
  private knownModels(): Set<string> {
    const known = new Set(this.context?.models ?? []);
    for (const model of this.models) known.add(model);
    if (this.filters.model) known.add(this.filters.model);
    return known;
  }

  private draw(): void {
    this.drawHead();
    this.drawRows();
    this.drawFoot();
  }

  private drawHead(): void {
    const { words, usage, claude } = this.options;
    const filter = (what: keyof Filters, name: string, value: string | null) =>
      `<button class="uc-filter${value ? " on" : ""}" type="button" data-uc="${what}" data-menu-key="uc:${what}" aria-haspopup="menu"><span>${escape(value ?? name)}</span>${glyph("chevron", 14)}</button>`;
    const { model, source, outcome } = this.filters;
    const kinds = claude
      ? [["read", usage.kinds.cache_read], ["write", usage.kinds.cache_write], ["input", usage.kinds.input], ["output", usage.kinds.output]]
      : [["input", usage.kinds.fresh], ["read", usage.kinds.cached], ["output", usage.kinds.output], ["reason", usage.kinds.reasoning]];
    const legend = `<span class="ledger-legend uc-legend">${kinds.map(([kind, label]) => `<span class="ledger-keyed"><i class="uc-k-${kind}"></i>${escape(label!)}</span>`).join("")}</span>`;
    const html = `<span class="ledger-panel-title" title="${escape(words.kept)}">${escape(words.title)}</span>${
      this.turn ? `<button class="uc-filter on uc-turn" type="button" data-uc="turn" title="${escape(words.turnSaid)}" aria-label="${escape(`${words.turn}, ${words.allTurns}`)}"><span>${escape(words.turn)}</span>${glyph("close", 14)}</button>` : ""
    }<span class="ledger-grow"></span>${legend}<span class="uc-filters">${filter("model", words.model, model ? modelName(model) : null)}${filter("source", words.source, source ? words.sources[source] : null)}${filter("outcome", words.outcome, outcome ? words.outcomes[outcome] : null)}</span>`;
    if (html === this.headDrawn) return;
    this.headDrawn = html;
    this.head.innerHTML = html;
  }

  /** The rows of the one the page is narrowed to, where the machine could
   *  not narrow them. */
  private keep(rows: UsageCall[]): UsageCall[] {
    const person = this.context?.person;
    if (!person || person.account) return rows;
    const context = this.context!;
    return rows.filter((row) => context.who(row.who).key === person.key);
  }

  private drawRows(): void {
    const { words } = this.options;
    const context = this.context;
    if (!context) return;
    for (const row of this.rows) if (row.model) this.models.add(row.model);
    const rows = this.keep(this.rows);
    // Someone besides the one the page counts this week shows whose rows are whose.
    const several = context.several || new Set(this.rows.map((row) => context.who(row.who).key)).size > 1;
    let html = "";
    let day = "";
    for (const row of rows) {
      const at = new Date(row.timing.ended_at);
      const said = this.dayOf(at, context.now);
      if (said !== day) {
        day = said;
        html += `<div class="uc-day">${escape(said)}</div>`;
      }
      html += this.row(row, at, several, context);
    }
    const filtered = !!(this.filters.model || this.filters.source || this.filters.outcome || this.turn || context.person);
    // Only an empty list says why: what was not read, what is being read,
    // or that nothing is there.
    const empty = rows.length
      ? ""
      : this.problem
        ? `<p class="feature-note uc-note" title="${escape(this.problem)}">${escape(this.options.usage.failed)}</p>`
        : this.reading
          ? `<p class="feature-quiet uc-note">${escape(this.options.usage.reading)}</p>`
          : `<p class="feature-quiet uc-note">${escape(filtered ? words.noneFiltered : words.none)}</p>`;
    const shape = `${several ? " several" : ""}${this.options.claude ? " dollars" : ""}`;
    const drawn = `${shape}|${html}|${empty}`;
    if (drawn === this.rowsDrawn) return;
    this.rowsDrawn = drawn;
    this.list.className = `uc-rows${shape}`;
    this.list.innerHTML = html + empty;
  }

  /** The pager, once there is more than a page: which page this is, a step
   *  to the newer and to the older, and back to the newest from further. */
  private drawFoot(): void {
    const { words, usage } = this.options;
    const page = this.pages.length;
    this.list.setAttribute("aria-busy", String(this.reading));
    this.foot.hidden = page === 1 && !this.next;
    if (this.foot.hidden) return;
    const slip = this.foot.querySelector<HTMLElement>(".uc-slip")!;
    slip.textContent = this.slip ? usage.failed : this.reading ? usage.reading : "";
    slip.title = this.slip ?? "";
    slip.classList.toggle("wrong", !!this.slip);
    this.foot.querySelector<HTMLElement>(".uc-page")!.textContent = words.page(page);
    this.foot.querySelector<HTMLButtonElement>(".uc-newest")!.hidden = page < 3;
    this.foot.querySelector<HTMLButtonElement>("[data-uc=newer]")!.disabled = page === 1;
    this.foot.querySelector<HTMLButtonElement>("[data-uc=older]")!.disabled = !this.next;
  }

  private dayOf(at: Date, now: number): string {
    const { words, usage } = this.options;
    const start = new Date(now);
    start.setHours(0, 0, 0, 0);
    const day = new Date(at);
    day.setHours(0, 0, 0, 0);
    const back = Math.round((start.getTime() - day.getTime()) / 86_400_000);
    return back <= 0 ? words.today : back === 1 ? words.yesterday : usage.day(at);
  }

  /** One request's row: its marks, and the rest under the pointer. */
  private row(row: UsageCall, at: Date, several: boolean, context: CallsContext): string {
    const { words, usage, claude } = this.options;
    const who = context.who(row.who);
    const tokens = row.tokens;
    const kinds = tokens
      ? claude
        ? ([["read", tokens.cache_read ?? 0], ["write", tokens.cache_write ?? 0], ["input", tokens.input ?? 0], ["output", (tokens.output ?? 0) + (tokens.reasoning ?? 0)]] as const)
        : ([["input", tokens.input ?? 0], ["read", tokens.cache_read ?? 0], ["output", tokens.output ?? 0], ["reason", tokens.reasoning ?? 0]] as const)
      : [];
    const sum = kinds.reduce((all, [, value]) => all + value, 0);
    const background = row.source === "background";
    const started = row.timing.started_at ? Date.parse(row.timing.started_at) : null;
    const took = started === null ? null : Math.max(0, at.getTime() - started);
    const first = started !== null && row.timing.first_at ? Math.max(0, Date.parse(row.timing.first_at) - started) : null;
    const rough = row.timing.approximate;
    const own = row.conversation !== null;
    const model = row.model ? modelName(row.model) : null;
    const usd = row.usd === null ? null : row.usd / 1_000_000;

    // The sentences behind the row.
    const named = (kind: string) => (claude ? { read: usage.kinds.cache_read, write: usage.kinds.cache_write, input: usage.kinds.input, output: usage.kinds.output } : { input: usage.kinds.fresh, read: usage.kinds.cached, output: usage.kinds.output, reason: usage.kinds.reasoning })[kind as "input"] ?? kind;
    const failed = row.outcome !== "completed" && !(background && row.outcome === "unknown");
    const said = [
      words.at(at),
      several ? who.name : "",
      background ? words.backgroundSaid : [model ?? "", row.source === "agent" ? words.agentTag : ""].filter(Boolean).join(words.and),
      ...kinds.filter(([, value]) => value > 0).map(([kind, value]) => `${named(kind)} ${value.toLocaleString("en-US")}`),
      usd !== null ? usage.money(usd) : "",
      [took !== null ? words.took(took) : "", first !== null ? words.first(first) : ""].filter(Boolean).join(words.and),
      rough && took !== null ? words.roughly : "",
      failed ? [words.outcomeSaid[row.outcome as Exclude<CallOutcome, "completed">], row.phase === "before_response" || row.phase === "streaming" ? words.phase[row.phase] : ""].filter(Boolean).join(words.and) : "",
      own ? words.open : words.others,
    ]
      .filter(Boolean)
      .join("\n");

    const time = `<span class="uc-time">${at.getHours()}:${String(at.getMinutes()).padStart(2, "0")}<span class="uc-sec">:${String(at.getSeconds()).padStart(2, "0")}</span></span>`;
    const whoCell = several ? `<span class="uc-who"><i style="background:${who.ink}"></i><span>${escape(who.name)}</span></span>` : "";
    // A subagent's request: a tag where there is room, a hook off the main
    // loop where there is not.
    const sub = row.source === "agent";
    const modelCell = `<span class="uc-model${model ? "" : " quiet"}">${sub ? `<span class="uc-hook">${glyph("fed", 14)}</span>` : ""}<span class="uc-model-name">${escape(model ?? words.background)}</span>${sub ? `<span class="uc-tag">${escape(words.agentTag)}</span>` : ""}</span>`;
    const width = sum ? Math.max(3, Math.min(100, Math.sqrt(sum / FULL) * 100)) : 0;
    const mix = sum
      ? `<span class="uc-mix" style="width:${width.toFixed(1)}%">${kinds
          .filter(([, value]) => value > 0)
          .map(([kind, value]) => `<i class="uc-k-${kind}" style="flex-grow:${value}"></i>`)
          .join("")}</span>`
      : "";
    const figures = `<span class="uc-bar">${mix}</span><span class="uc-tokens">${sum ? escape(usage.tokens(sum)) : ""}</span>${claude ? `<span class="uc-usd">${usd !== null ? escape(usage.money(usd)) : ""}</span>` : ""}<span class="uc-took">${took !== null ? `${rough ? "≈" : ""}${escape(words.short(took))}` : ""}</span>`;
    const mark = failed
      ? `<span class="uc-mark ${row.outcome}">${row.outcome === "failed" ? glyph("cross", 14) : row.outcome === "interrupted" ? glyph("stop", 14) : row.outcome === "held" ? glyph("lock", 14) : "?"}</span>`
      : `<span class="uc-mark"></span>`;
    const open = `<span class="uc-open">${own ? glyph("chevron", 14) : ""}</span>`;
    const cells = time + whoCell + modelCell + figures + mark + open;
    return own
      ? `<button class="uc-row own" type="button" data-key="${escape(row.cursor.key)}" title="${escape(said)}">${cells}</button>`
      : `<div class="uc-row" data-key="${escape(row.cursor.key)}" title="${escape(said)}">${cells}</div>`;
  }
}
