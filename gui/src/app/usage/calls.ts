// A login's requests, one row each, the newest first (`world-usage.md`,
// 调用记录; `world-gui.md`, 智能体页): the usage page's fifth view. Each row
// is when, whose, which model, what it was made of as a small bar in the
// kinds' inks, what it cost and how long it took; what went wrong is a mark,
// and the rest is said while the pointer rests on it. A row of the reader's
// own conversation opens that turn. The rows are those the login's machine
// lets the reader see, already narrowed and with others' conversations
// taken out there; a hundred are asked at a time as the list is scrolled
// to its end, and again each minute while it is open.

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

/** The list's last rows this far from view ask for the next hundred. */
const AHEAD = "480px";
/** A request's tokens drawn at full width, and past it: the bars are
 *  scaled by the square root of their share of it, so small requests stay
 *  visible beside a long context read from cache. */
const FULL = 400_000;

export class CallsList {
  /** The panel itself, kept across the page's drawings. */
  readonly element: HTMLElement;
  private readonly head: HTMLElement;
  private readonly list: HTMLElement;
  private readonly end: HTMLElement;
  private readonly watch: IntersectionObserver;
  private context: CallsContext | null = null;
  private filters: Filters = { model: null, source: null, outcome: null };
  private turn: CallsOptions["turn"];
  private rows: UsageCall[] = [];
  private next: CallCursor | null = null;
  /** The query the rows are of; another one starts them over. */
  private asked = "";
  private generation = 0;
  private reading = false;
  private problem: string | null = null;
  private models = new Set<string>();
  private headDrawn = "";
  private rowsDrawn = "";
  /** Where the rows were scrolled, kept while the page draws itself anew. */
  private scrolled = 0;

  constructor(private readonly options: CallsOptions) {
    this.turn = options.turn;
    this.element = document.createElement("section");
    this.element.className = "ledger-panel uc";
    this.element.innerHTML = `<div class="ledger-panel-head uc-head"></div><div class="uc-rows"></div>`;
    this.head = this.element.querySelector<HTMLElement>(".uc-head")!;
    this.list = this.element.querySelector<HTMLElement>(".uc-rows")!;
    this.end = document.createElement("div");
    this.end.className = "uc-end";
    this.watch = new IntersectionObserver((seen) => {
      if (seen.some((one) => one.isIntersecting)) void this.more();
    }, { rootMargin: AHEAD });
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

  /** Read again, as the page does each minute: rows newer than the first
   *  join the top; a minute with more than a page of them starts over. */
  async again(): Promise<void> {
    if (!this.context || this.reading) return;
    // Not read the last time, or nothing yet: read from the start.
    if (this.problem || !this.rows.length) return this.restart(this.asked);
    const generation = this.generation;
    const page = await this.options.ask(this.query(null)).catch(() => null);
    if (!page || generation !== this.generation) return;
    const top = this.rows[0]!.cursor.key;
    const at = page.rows.findIndex((row) => row.cursor.key === top);
    if (at < 0) {
      this.rows = page.rows;
      this.next = page.next;
    } else if (at > 0) {
      const height = this.list.scrollHeight;
      this.rows = [...page.rows.slice(0, at), ...this.rows];
      this.draw();
      // The rows the reader was looking at stay where they were.
      if (this.list.scrollTop > 0) this.list.scrollTop += this.list.scrollHeight - height;
      return;
    }
    this.draw();
  }

  leave(): void {
    this.watch.disconnect();
    this.generation += 1;
  }

  /** The rows asked for: the query, and whom they are narrowed to here. */
  private queryKey(): string {
    return JSON.stringify([this.query(null), this.context?.person?.key ?? null]);
  }

  /** The rows shown: those of the one the page is narrowed to, where the
   *  machine could not narrow them. */
  private shown(): UsageCall[] {
    const person = this.context?.person;
    if (!person || person.account) return this.rows;
    const context = this.context!;
    return this.rows.filter((row) => context.who(row.who).key === person.key);
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
    this.next = null;
    this.problem = null;
    this.reading = false;
    this.list.scrollTop = 0;
    this.scrolled = 0;
    this.draw();
    void this.more(true);
  }

  /** The next hundred, or the first. */
  private async more(first = false): Promise<void> {
    if (this.reading || this.problem || (!first && !this.next)) return;
    const generation = this.generation;
    this.reading = true;
    this.draw();
    try {
      const page = await this.options.ask(this.query(first ? null : this.next));
      if (generation !== this.generation) return;
      this.rows = first ? page.rows : [...this.rows, ...page.rows];
      this.next = page.next;
    } catch (error) {
      if (generation !== this.generation) return;
      this.problem = detailOf(error);
    } finally {
      if (generation === this.generation) {
        this.reading = false;
        this.draw();
      }
    }
  }

  private click(event: Event): void {
    const target = (event.target as HTMLElement).closest<HTMLElement>("[data-uc], [data-open]");
    if (!target) return;
    if (target.dataset.open !== undefined) {
      const row = this.rows.find((one) => one.cursor.key === target.dataset.open);
      if (row?.conversation) this.options.open(row.conversation, row.turn);
      return;
    }
    const what = target.dataset.uc;
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
    const html = `<span class="ledger-panel-title">${escape(words.title)}</span>${
      this.turn ? `<button class="uc-filter on uc-turn" type="button" data-uc="turn" title="${escape(words.turnSaid)}" aria-label="${escape(`${words.turn}, ${words.allTurns}`)}"><span>${escape(words.turn)}</span>${glyph("close", 14)}</button>` : ""
    }<span class="ledger-grow"></span>${legend}<span class="uc-filters">${filter("model", words.model, model ? modelName(model) : null)}${filter("source", words.source, source ? words.sources[source] : null)}${filter("outcome", words.outcome, outcome ? words.outcomes[outcome] : null)}</span>`;
    if (html === this.headDrawn) return;
    this.headDrawn = html;
    this.head.innerHTML = html;
  }

  private drawRows(): void {
    const { words } = this.options;
    const context = this.context;
    if (!context) return;
    for (const row of this.rows) if (row.model) this.models.add(row.model);
    const rows = this.shown();
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
    const foot = this.problem
      ? `<p class="feature-note uc-note" title="${escape(this.problem)}">${escape(this.options.usage.failed)}</p>`
      : this.reading
        ? `<p class="feature-quiet uc-note">${escape(this.options.usage.reading)}</p>`
        : this.next
          ? ""
          : !rows.length
            ? `<p class="feature-quiet uc-note">${escape(filtered ? words.noneFiltered : words.none)}</p>`
            : `<p class="uc-kept">${escape(words.kept)}</p>`;
    const shape = `${several ? " several" : ""}${this.options.claude ? " dollars" : ""}`;
    const drawn = `${shape}|${html}|${foot}`;
    if (drawn === this.rowsDrawn) return;
    this.rowsDrawn = drawn;
    this.list.className = `uc-rows${shape}`;
    this.list.innerHTML = html + foot;
    this.list.append(this.end);
    this.watch.disconnect();
    if (this.next && !this.reading && !this.problem) this.watch.observe(this.end);
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

    // The sentences behind the row.
    const named = (kind: string) => (claude ? { read: usage.kinds.cache_read, write: usage.kinds.cache_write, input: usage.kinds.input, output: usage.kinds.output } : { input: usage.kinds.fresh, read: usage.kinds.cached, output: usage.kinds.output, reason: usage.kinds.reasoning })[kind as "input"] ?? kind;
    const failed = row.outcome !== "completed" && !(background && row.outcome === "unknown");
    const said = [
      words.at(at),
      several ? who.name : "",
      background ? words.backgroundSaid : [model ?? "", row.source === "agent" ? words.agentTag : ""].filter(Boolean).join(words.and),
      ...kinds.filter(([, value]) => value > 0).map(([kind, value]) => `${named(kind)} ${value.toLocaleString("en-US")}`),
      row.usd !== null ? usage.money(row.usd) : "",
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
    const figures = `<span class="uc-bar">${mix}</span><span class="uc-tokens">${sum ? escape(usage.tokens(sum)) : ""}</span>${claude ? `<span class="uc-usd">${row.usd !== null ? escape(usage.money(row.usd)) : ""}</span>` : ""}<span class="uc-took">${took !== null ? `${rough ? "≈" : ""}${escape(words.short(took))}` : ""}</span>`;
    const mark = failed
      ? `<span class="uc-mark ${row.outcome}">${row.outcome === "failed" ? glyph("cross", 14) : row.outcome === "interrupted" ? glyph("stop", 14) : row.outcome === "held" ? glyph("lock", 14) : "?"}</span>`
      : `<span class="uc-mark"></span>`;
    const open = `<span class="uc-open">${own ? glyph("chevron", 14) : ""}</span>`;
    const cells = time + whoCell + modelCell + figures + mark + open;
    return own
      ? `<button class="uc-row own" type="button" data-open="${escape(row.cursor.key)}" title="${escape(said)}">${cells}</button>`
      : `<div class="uc-row" title="${escape(said)}">${cells}</div>`;
  }
}
