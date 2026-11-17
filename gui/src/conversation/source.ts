// A reading of a conversation: what the shell opens it on, and what follows.
// One shown from its timeline opens on its latest part, is told the records
// the timeline gains, and reads the parts its reader comes to; one on a
// machine of an earlier release is watched whole, its window first, then
// every event as it happens. Each reading is a watch of its own, and what
// it reads is read from its own.

import type { Address, SessionEvent } from "../world/events";
import { after, whyOf } from "../world/again";
import { conversationSearch, conversationCut, conversationFiles, conversationFind, conversationIndex, conversationLocate, conversationObject, conversationPage, conversationPart, unwatch, watch, type Piece } from "../world/bridge";
import type { PartSource, Window } from "./conversation";
import type { ReplicaPart, ReplicaRecord } from "./replica";

export interface ConversationFeed {
  /** The part shown, in place of all held; `parts` reads the rest. */
  window(window: Window, parts: PartSource): void;
  /** Events after all that was given. */
  extended(start: number, events: SessionEvent[]): void;
  /** What came before arrived: what is held moves along. */
  rebased(generation: number, offset: number, turnsAdded: number): void;
  /** One thing as it happens, the `at`th event. */
  happened(at: number, event: SessionEvent): void;
  /** The latest part of its timeline, in place of all held; `parts` reads
   *  the rest. */
  replica(part: ReplicaPart, parts: PartSource): void;
  /** Records its timeline gained. */
  records(generation: number, records: ReplicaRecord[]): void;
  /** Whether its timeline is still being made from the history. */
  preparing(preparing: boolean): void;
  /** The requests that wait for the user now, all of them. */
  waiting(events: SessionEvent[]): void;
  /** A request that waits for the user, or the end of one. */
  asked(event: SessionEvent): void;
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

/** The try a refused watch is paced as when it is asked again: within a
 *  minute (`again.ts`, `backoff`). */
const REFUSED_AGAIN = 8;

/** The World's watch: the part it opens on, then what follows; pieces that
 *  come before it wait for it. */
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
    const parts: PartSource = {
      identity: () => id === null ? `waiting:${reading}` : String(id),
      search: async (request) => { const owner = current(); const result = await conversationSearch(owner, request); if (closed || owner !== id) throw new Error("the conversation closed during search"); return result; },
      part: async (generation, edge) => {
        const owner = current();
        const part = await conversationPart(owner, generation, edge);
        if (id !== owner || closed) return null;
        return part;
      },
      page: async (generation, from) => {
        const owner = current();
        const part = await conversationPage(owner, generation, from);
        if (id !== owner || closed) return null;
        return part;
      },
      object: async (object, bytes) => {
        const owner = current();
        const expanded = await conversationObject(owner, object, bytes);
        if (closed || refused || id !== owner) throw new Error("the conversation closed while its content was opening");
        return expanded;
      },
      files: async (after) => {
        const owner = current();
        const told = await conversationFiles(owner, after);
        if (closed || id !== owner) throw new Error("the conversation closed while its files were read");
        return told;
      },
      locate: async (generation, turn) => conversationLocate(current(), generation, turn),
      cut: async (generation, events) => conversationCut(current(), generation, events),
      index: async (generation, start, end, docs) => conversationIndex(current(), generation, start, end, docs),
      find: async (generation, query, held) => conversationFind(current(), generation, query, held),
    };
    const decline = (why: string, attempted: string | null) => {
      refused = true;
      feed.refused(why);
      // A reply from before a new grant must not close the newly usable conversation.
      if (authority !== null && authority !== attempted) return restart();
      // A machine refuses too while its own word from the hub is not current,
      // as when the hub has just started again, and nothing granted changes
      // afterwards: it is asked again now and then, and at once when a way
      // to it opens.
      waiting?.();
      waiting = after(
        REFUSED_AGAIN,
        () => {
          waiting = null;
          if (refused && !closed) restart();
        },
        address.machine,
      );
    };
    const take = (piece: Piece, attempted: string | null) => {
      switch (piece.kind) {
        case "resumed":
          feed.resumed();
          return;
        case "replica":
          feed.replica(piece.part, parts);
          return;
        case "records":
          feed.records(piece.generation, piece.records);
          return;
        case "preparing":
          feed.preparing(piece.preparing);
          return;
        case "waiting":
          feed.waiting(piece.events);
          return;
        case "asked":
          feed.asked(piece.event);
          return;
        case "event":
          feed.happened(piece.at, piece.event);
          return;
        case "window":
          feed.window(piece.window, parts);
          return;
        case "extended":
          feed.extended(piece.start, piece.events);
          return;
        case "rebased":
          feed.rebased(piece.generation, piece.offset, piece.turns_added);
          return;
        case "interrupted":
          // This interface fell behind while the line held: the shell reads
          // the conversation again at once, its window first; nothing broke.
          if (piece.gap) return;
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
          if (started.replica) feed.replica(started.replica, parts);
          else if (started.window) feed.window(started.window, parts);
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
