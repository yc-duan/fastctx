import type { SessionEvent } from "../world/events";
import type { Conversation, TimelinePosition } from "./conversation";
import type { FindMatch } from "./find-index";

/** An event as a search tells it: its body, or none where the page holds its
 *  text as of `sequence` already (`known`). */
export interface SearchEvent { place: TimelinePosition; sequence: string; event?: SessionEvent }
/** An event whose text the page holds, as of its sequence. */
export interface KnownText { item: string; sequence: string }
interface Ranking { after: TimelinePosition; before: string }
interface Seeking { after: TimelinePosition; left: string }
export interface SearchHit { place: TimelinePosition; ordinal: number }
export type SearchRequest = { do: "start"; known?: KnownText[] } | { do: "next"; token: string; counts: number[] }
  | { do: "hit"; token: string; ordinal: string; cursor: Seeking | null }
  | { do: "rank"; token: string; item: string; ordinal: number; cursor: Ranking | null } | { do: "close"; token: string };
export type SearchReply = { kind: "batch"; token: string; events: SearchEvent[]; total: string; examined: string; complete: boolean; previews: string }
  | { kind: "hit"; hit: { found: SearchHit | null; next: Seeking | null } }
  | { kind: "rank"; rank: { found: [TimelinePosition, string] | null; next: Ranking | null } } | { kind: "closed" };

/** Search holds one page and query-local metadata, independently of navigation. */
export class TimelineFind {
  total = 0;
  examined = 0;
  previews = 0;
  complete = false;
  error: string | null = null;
  grown = 0;
  private query = "";
  private owner: string | undefined;
  private token: string | null = null;
  private epoch = 0;
  private signal = new AbortController();
  private working: Promise<void> | null = null;
  private request: Promise<SearchReply> | null = null;
  private navigating: Promise<void> | null = null;
  private navigationDepth = 0;
  private releaseNavigation: (() => void) | null = null;
  private selectedHit: SearchHit | null = null;
  private version = -1;

  constructor(private readonly conversation: Conversation,
    private readonly count: (events: SearchEvent[], query: string, signal: AbortSignal) => Promise<number[]>,
    private readonly locate: (hit: SearchHit, query: string, signal: AbortSignal) => Promise<FindMatch | null>,
    private readonly grew: () => void,
    private readonly known: () => KnownText[] = () => []) {}

  async search(query: string): Promise<void> {
    const owner = this.conversation.searchIdentity;
    if (query !== this.query || owner !== this.owner) {
      this.close(); this.query = query; this.owner = owner;
      if (!query) return;
      this.total = this.examined = this.previews = 0; this.complete = false; this.error = null;
      this.version = this.conversation.searchVersion;
      this.run(true);
    } else if (!this.working && this.version !== this.conversation.searchVersion) {
      this.version = this.conversation.searchVersion; this.run(this.token === null);
    }
  }

  private run(start: boolean): void {
    const epoch = this.epoch; const signal = this.signal.signal;
    this.error = null;
    this.working = (async () => {
      let request: SearchRequest = start ? { do: "start", known: this.known() } : { do: "next", token: this.token!, counts: [] };
      for (;;) {
        if (this.navigating) await this.navigating;
        signal.throwIfAborted();
        const reply: SearchReply = await (this.request = this.conversation.search(request));
        if (epoch !== this.epoch) { if (reply.kind === "batch") void this.conversation.search({ do: "close", token: reply.token }).catch(() => {}); return; }
        if (reply.kind !== "batch") throw new Error("the search did not answer a batch");
        this.token = reply.token;
        this.total = Number(reply.total); this.examined = Number(reply.examined); this.previews = Number(reply.previews); this.complete = reply.complete;
        this.grown += 1; this.grew();
        if (reply.complete) return;
        const counts = await this.count(reply.events, this.query, signal);
        signal.throwIfAborted();
        request = { do: "next", token: reply.token, counts };
        await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      }
    })().catch((error: unknown) => {
      if (epoch !== this.epoch || signal.aborted) return;
      this.error = String(error); this.grew();
    }).finally(() => { if (epoch === this.epoch) { this.working = null; this.request = null; if (!signal.aborted && this.version !== this.conversation.searchVersion) { this.version = this.conversation.searchVersion; this.run(this.token === null); } } });
  }

  async match(ordinal: number): Promise<FindMatch | null> {
    if (!this.token || ordinal < 0) return null;
    const release = this.holdNavigation();
    const epoch = this.epoch; const signal = this.signal.signal;
    try {
      // A pending batch may commit counts. Afterwards the scanner stays paused
      // until the metadata cursor has resolved and navigation has finished.
      await this.request;
      let cursor: Seeking | null = null;
      for (;;) {
        signal.throwIfAborted();
        const reply: SearchReply = await this.conversation.search({ do: "hit", token: this.token, ordinal: String(ordinal), cursor });
        if (epoch !== this.epoch) return null;
        if (reply.kind !== "hit") throw new Error("the search did not answer a match");
        if (reply.hit.found) { this.selectedHit = reply.hit.found; return await this.locate(reply.hit.found, this.query, signal); }
        cursor = reply.hit.next;
        if (!cursor) return null;
      }
    } finally { release(); }
  }

  private holdNavigation(): () => void {
    const epoch = this.epoch;
    if (this.navigationDepth++ === 0) this.navigating = new Promise<void>((resolve) => { this.releaseNavigation = resolve; });
    return () => {
      if (epoch !== this.epoch || --this.navigationDepth !== 0) return;
      this.releaseNavigation?.(); this.releaseNavigation = null; this.navigating = null;
    };
  }

  async position(): Promise<number | null> {
    const hit = this.selectedHit; const token = this.token;
    if (!hit || !token) return null;
    const release = this.holdNavigation(); const epoch = this.epoch;
    try {
      await this.request;
      let cursor: Ranking | null = null;
      for (;;) {
        this.signal.signal.throwIfAborted();
        const reply: SearchReply = await this.conversation.search({ do: "rank", token, item: hit.place.item, ordinal: hit.ordinal, cursor });
        if (epoch !== this.epoch) return null;
        if (reply.kind !== "rank") throw new Error("the search did not answer the match's position");
        if (reply.rank.found) { hit.place = reply.rank.found[0]; return Number(reply.rank.found[1]); }
        cursor = reply.rank.next;
        if (!cursor) { this.selectedHit = null; return null; }
      }
    } finally { release(); }
  }

  async step(by: number, current: number): Promise<{ match: FindMatch | null; ordinal: number }> {
    const release = this.holdNavigation();
    try {
      const known = await this.position();
      const ordinal = ((known ?? current) + by + this.total) % this.total;
      return { match: await this.match(ordinal), ordinal };
    } finally { release(); }
  }

  close(): void {
    this.releaseNavigation?.(); this.releaseNavigation = null; this.navigationDepth = 0; this.selectedHit = null;
    this.epoch += 1; this.signal.abort(); this.signal = new AbortController();
    const token = this.token;
    if (token) void this.conversation.search({ do: "close", token }).catch(() => {});
    this.query = ""; this.token = null; this.working = null; this.request = null; this.navigating = null;
  }
}
