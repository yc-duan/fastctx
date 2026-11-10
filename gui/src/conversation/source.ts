// Where a conversation is read from, as one seam: the part its page opens
// on, then each thing as it happens. Today that is the World's watch on the
// session (`../world/bridge.ts`), whose shell keeps the whole conversation
// and hands the page its latest turns, and the rest a part at a time as the
// reader comes to it (`src-tauri/src/transcript.rs`). A copy kept on this
// device (`world-gui.md`, 对话: 每台设备上的缓存) answers at once with what
// it holds, then with what is new since, then live; the page draws the
// first answer at once and each later part as it comes, keeping what it
// drew wherever the keys are the same.

import type { Address, SessionEvent } from "../world/events";
import { after, whyOf } from "../world/again";
import { conversationCut, conversationFind, conversationIndex, conversationLocate, conversationPart, unwatch, watch, type Piece } from "../world/bridge";
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
  now(busy: boolean, turn: string | null, kept: import("./model").KeptWords[]): void;
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
    const parts: PartSource = {
      part: async (generation, edge) => conversationPart(current(), generation, edge),
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
        case "resumed":
          feed.resumed();
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
          feed.now(piece.busy, piece.turn, piece.kept ?? []);
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
      watch(address, (piece) => {
        if (closed || refused || mine !== reading) return;
        if (!ready) early.push(piece);
        else take(piece, attempted);
      })
        .then((started) => {
          if (closed || mine !== reading) {
            void unwatch(started.id);
            return;
          }
          id = started.id;
          tries = 0;
          since = null;
          feed.window(started.window, parts);
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
