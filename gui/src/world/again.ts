// Trying again what broke (`world-network.md`, 断开不是结束), paced as the
// core paces it: at once, then after waits that double up to a minute,
// each a random part of that; at once whenever the window comes to the
// front or the system says the network is back.

import type { NotSaid } from "./bridge";

/** The wait before try number `tries` (the first is 1). */
export function backoff(tries: number): number {
  if (tries <= 1) return 0;
  const ceiling = Math.min(60_000, 1000 * 2 ** Math.min(6, tries - 2));
  return 250 + Math.random() * Math.max(1, ceiling - 250);
}

/** Calls `again` once the wait for try number `tries` is over, or as soon
 *  as the window comes to the front or the network is back; returns how to
 *  call it off. */
export function after(tries: number, again: () => void): () => void {
  let timer: number | undefined;
  const now = () => {
    off();
    again();
  };
  const off = () => {
    clearTimeout(timer);
    removeEventListener("focus", now);
    removeEventListener("online", now);
  };
  timer = window.setTimeout(now, backoff(tries));
  addEventListener("focus", now);
  addEventListener("online", now);
  return off;
}

/** Whether a failure says no answer came, so the same call goes again. */
export function unanswered(error: unknown): boolean {
  return typeof error === "object" && error !== null && (error as Partial<NotSaid>).again === true;
}

/** Why something was not done, as a line. */
export function whyOf(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) return String((error as { message: unknown }).message);
  return String(error);
}

/** Makes `call` until an answer comes: a call that got none is made again,
 *  as the same call, and `waiting` hears why each time. An answer that
 *  refuses ends it. It goes on while the page that made it is gone: words
 *  the user said still go in once the way is open. */
export function untilAnswered<T>(call: () => Promise<T>, waiting: (why: string) => void): Promise<T> {
  return new Promise((resolve, reject) => {
    let tries = 0;
    const attempt = () => {
      tries += 1;
      call().then(resolve, (error: unknown) => {
        if (!unanswered(error)) {
          reject(error);
          return;
        }
        waiting(whyOf(error));
        after(tries + 1, attempt);
      });
    };
    attempt();
  });
}
