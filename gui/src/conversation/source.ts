// A reading owns its live subscription and bounded page window. Part replies
// may replace that window; generation numbers keep crossed replies from
// changing a newer reading. Older nodes keep the original Watch path.

import type { Address, SessionEvent } from "../world/events";
import { after, whyOf } from "../world/again";
import { conversationSearch, conversationScan, conversationCut, conversationFind, conversationIndex, conversationLocate, conversationFollow, conversationObject, conversationPart, unwatch, watch, type Piece } from "../world/bridge";
import type { PartSource, Window } from "./conversation";

export interface ConversationFeed {
  /** The part shown, in place of all held; `parts` reads the rest. */
  window(window: Window, parts: PartSource): void;
  /** Events after all that was given. */
  extended(start: number, events: SessionEvent[]): void;
  /** What came before arrived: what is held moves along. */
  rebased(generation: number, offset: number, turnsAdded: number): void;
  /** One thing as it happens, the `at`th event. */
  happened(at: number, event: SessionEvent): void;
  live(event: SessionEvent, key: string, evicted?: string[], placement?: { after: string | null }, older?: boolean, object?: string, body?: SessionEvent): void;
  timeline(delta: import("./conversation").TimelineDelta): void;
  /** The live part broke and is being read again: why, and since when. */
  interrupted(why: string | null, since: Date): void;
  /** It came back. */
  resumed(): void;
  /** It is over: what is shown stays, no longer live. */
  ended(why: string | null): void;
  /** It was deleted: what is shown stays, and nothing more comes. */
  deleted(why: string): void;
  /** Its machine refused this account: what is shown stays until current facts permit a retry. */
  refused(why: string): void;
  /** Whether it works now, on which turn, and which words its machine
   *  keeps on their way in; after every event before it. */
  now(busy: boolean, turn: string | null, kept: import("./model").KeptWords[], halted: string | null): void;
}

export interface Reading {
  /** Reads everything again, as when the host replaced or removed turns,
   *  or what came did not follow what is held. */
  again(): void;
  /** Reconsiders a refusal when the current listing or its permission revision changes. */
  recheck?(revision: string | null): void;
  close(): void;
}

export interface ConversationSource {
  open(address: Address, feed: ConversationFeed): Reading;
}

/** The World's watch: the part it opens on, then live; pieces that come
 *  before it wait for it. Each reading is a watch of its own, and the parts
 *  it reads are read from its own. */
export const watched: ConversationSource = {
  open(address, feed) {
    let id: number | null = null;
    let reading = 0;
    let closed = false;
    let refused = false;
    let authority: string | null = null;
    const current = () => {
      if (id === null) throw new Error("the conversation is not watched now");
      return id;
    };
    let generation: number | null = null;
    let navigation = 0;
    let pending: Extract<Piece, { kind: "event" | "live" | "timeline" }>[] = [];
    let pendingBytes = 0;
    const buffered = (piece: Extract<Piece, { kind: "event" | "live" | "timeline" }>) => {
      pendingBytes += new TextEncoder().encode(JSON.stringify(piece)).byteLength;
      if (pending.length >= 800 || pendingBytes > 4 * 1024 * 1024) { restart(); return; }
      pending.push(piece);
    };
    const install = (window: Window) => {
      if (window.pages && generation !== null && window.generation < generation) return;
      generation = window.pages ? window.generation : null;
      feed.window(window, parts);
      const events = pending;
      pending = [];
      pendingBytes = 0;
      for (const event of events) {
        const at = event.kind === "timeline" ? event.delta.generation : event.generation;
        if (at === generation) {
          if (event.kind === "timeline") feed.timeline(event.delta);
          else if (event.kind === "live") feed.live(event.event, event.key, event.evicted, event.placement, event.older, event.object, event.body);
          else feed.happened(event.at, event.event);
        } else if (at! > (generation ?? -1)) buffered(event);
      }
    };
    const parts: PartSource = {
      identity: () => id === null ? `waiting:${reading}` : String(id),
      search: async (request) => { const owner = current(); const result = await conversationSearch(owner, request); if (closed || owner !== id) throw new Error("the conversation closed during search"); return result; },
      scan: async (cursor) => { const owner = current(); const scanned = await conversationScan(owner, cursor); if (closed || id !== owner) throw new Error("the conversation closed during export"); return scanned; },
      part: async (generation, edge) => {
        const owner = current();
        const request = ++navigation;
        const part = await conversationPart(owner, generation, edge, request);
        if (id !== owner || closed) return null;
        // A committed native generation must be installed even if a later
        // gesture superseded this request. Only a newer installed window can
        // supersede it; the later request may legitimately return no window.
        if (part?.window) { install(part.window); return null; }
        if (request !== navigation) return null;
        return part;
      },
      object: async (generation, object) => {
        const owner = current();
        const expanded = await conversationObject(owner, generation, object);
        if (closed || refused || id !== owner) throw new Error("the conversation closed while its content was opening");
        return expanded;
      },
      follow: async (revision, following) => conversationFollow(current(), revision, following),
      locate: async (generation, turn) => conversationLocate(current(), generation, turn),
      cut: async (generation, events) => conversationCut(current(), generation, events),
      index: async (generation, start, end, docs) => conversationIndex(current(), generation, start, end, docs),
      find: async (generation, query, held) => conversationFind(current(), generation, query, held),
    };
    const decline = (why: string, attempted: string | null) => {
      refused = true;
      feed.refused(why);
      // A reply from before a new grant must not close the newly usable conversation.
      if (authority !== null && authority !== attempted) restart();
    };
    const take = (piece: Piece, attempted: string | null) => {
      switch (piece.kind) {
        case "reopen":
          restart();
          return;
        case "resumed":
          feed.resumed();
          return;
        case "timeline":
          if (piece.delta.generation === generation) feed.timeline(piece.delta);
          else if (piece.delta.generation > (generation ?? -1)) buffered(piece);
          return;
        case "live":
        case "event":
          if (piece.generation !== undefined && piece.generation !== generation) {
            if (piece.generation > (generation ?? -1)) {
              buffered(piece);
            }
            return;
          }
          if (piece.kind === "live") feed.live(piece.event, piece.key, piece.evicted, piece.placement, piece.older, piece.object, piece.body); else feed.happened(piece.at, piece.event);
          return;
        case "window":
          install(piece.window);
          return;
        case "extended":
          feed.extended(piece.start, piece.events);
          return;
        case "rebased":
          feed.rebased(piece.generation, piece.offset, piece.turns_added);
          return;
        case "interrupted":
          feed.interrupted(piece.error, new Date(piece.since));
          return;
        case "ended":
          feed.ended(piece.error);
          return;
        case "deleted":
          closed = true;
          feed.deleted(piece.error);
          return;
        case "refused":
          decline(piece.error, attempted);
          return;
        case "now":
          feed.now(piece.busy, piece.turn, piece.kept ?? [], piece.halted ?? null);
      }
    };
    // A watch that could not start is tried again, as `../world/again.ts`
    // paces it (`world-network.md`, 断开不是结束).
    let tries = 0;
    let since: Date | null = null;
    let waiting: (() => void) | null = null;
    const read = () => {
      const mine = ++reading;
      const attempted = authority;
      let ready = false;
      const early: Piece[] = [];
      let earlyBytes = 0;
      watch(address, (piece) => {
        if (closed || refused || mine !== reading) return;
        if (!ready) {
          earlyBytes += new TextEncoder().encode(JSON.stringify(piece)).byteLength;
          if (early.length >= 800 || earlyBytes > 4 * 1024 * 1024) { restart(); return; }
          early.push(piece);
        } else take(piece, attempted);
      })
        .then((started) => {
          if (closed || mine !== reading) {
            void unwatch(started.id);
            return;
          }
          id = started.id;
          tries = 0;
          since = null;
          generation = null;
          pending = [];
          pendingBytes = 0;
          install(started.window);
          for (const piece of early) {
            if (closed || refused || mine !== reading) break;
            take(piece, attempted);
          }
          ready = true;
        })
        .catch((error: unknown) => {
          if (mine !== reading || closed) return;
          // Deletion is final. A refusal waits for changed facts instead of retrying on a timer.
          if (typeof error === "object" && error !== null && (error as { deleted?: unknown }).deleted === true) {
            closed = true;
            feed.deleted(whyOf(error));
            return;
          }
          if (typeof error === "object" && error !== null && (error as { refused?: unknown }).refused === true) {
            decline(whyOf(error), attempted);
            return;
          }
          since ??= new Date();
          feed.interrupted(whyOf(error), since);
          tries += 1;
          waiting = after(
            tries + 1,
            () => {
              waiting = null;
              read();
            },
            address.machine,
          );
        });
    };
    function restart(): void {
      if (closed) return;
      waiting?.();
      waiting = null;
      if (id !== null) void unwatch(id);
      id = null;
      generation = null;
      navigation += 1;
      pending = [];
      pendingBytes = 0;
      refused = false;
      read();
    }
    read();
    return {
      again: restart,
      recheck(revision) {
        const changed = authority !== revision;
        authority = revision;
        if (changed && authority !== null && refused) restart();
      },
      close() {
        closed = true;
        waiting?.();
        if (id !== null) void unwatch(id);
        id = null;
      },
    };
  },
};
