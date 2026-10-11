// Where a conversation is read from, as one seam: everything known of it so
// far, then each thing as it happens. Today that is the World's watch on the
// session (`../world/bridge.ts`), which hands the whole history and then the
// live events. A copy kept on this device (`world-gui.md`, 对话: 每台设备上的
// 缓存) stands in the same place: it answers at once with what it holds
// (`replaced`), then with what is new since (`extended`), then live, and
// reads it all again when the host rewrote its history (`replaced` again);
// the page draws the first answer at once and each later part as it comes,
// keeping what it drew wherever the keys are the same.

import type { Address, SessionEvent } from "../world/events";
import { unwatch, watch, type Piece } from "../world/bridge";

export interface ConversationFeed {
  /** Everything known so far, replacing what is shown. */
  replaced(history: SessionEvent[]): void;
  /** More of what came before what is live, after what is shown: what a
   *  copy at hand did not hold yet. */
  extended(history: SessionEvent[]): void;
  /** One thing as it happens. */
  happened(event: SessionEvent): void;
  /** The live part broke and waits to come back (a phone's connection). */
  interrupted(why: string | null): void;
  /** It came back. */
  resumed(): void;
  /** It is over: what is shown stays, no longer live. */
  ended(why: string | null): void;
}

export interface Reading {
  /** Reads everything again, as when the host replaced or removed turns. */
  again(): void;
  close(): void;
}

export interface ConversationSource {
  open(address: Address, feed: ConversationFeed): Reading;
}

/** The World's watch: a whole history, then live; events that come before
 *  the history wait for it. Each reading is a watch of its own. */
export const watched: ConversationSource = {
  open(address, feed) {
    let id: number | null = null;
    let reading = 0;
    let closed = false;
    const take = (piece: Piece) => {
      switch (piece.kind) {
        case "event":
          feed.resumed();
          feed.happened(piece.event);
          return;
        case "reset":
          feed.resumed();
          feed.replaced(piece.history);
          return;
        case "interrupted":
          feed.interrupted(piece.error);
          return;
        case "ended":
          feed.ended(piece.error);
      }
    };
    const read = () => {
      const mine = ++reading;
      let ready = false;
      const early: Piece[] = [];
      watch(address, (piece) => {
        if (closed || mine !== reading) return;
        if (!ready) early.push(piece);
        else take(piece);
      })
        .then((started) => {
          if (closed || mine !== reading) {
            void unwatch(started.id);
            return;
          }
          id = started.id;
          feed.replaced(started.history);
          for (const piece of early) take(piece);
          ready = true;
        })
        .catch((error: unknown) => {
          if (mine === reading && !closed) feed.ended(String(error));
        });
    };
    read();
    return {
      again() {
        if (id !== null) void unwatch(id);
        id = null;
        read();
      },
      close() {
        closed = true;
        if (id !== null) void unwatch(id);
        id = null;
      },
    };
  },
};
