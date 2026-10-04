// When the World calls the user from outside the window: an agent began
// waiting for their answer, or a conversation they opened finished a turn,
// while the window is not in front of them. What is said is the World's
// own fact, never a summary. A notification is taken back once what it
// called for no longer holds, and a click on it opens that conversation.
// The shell's own mechanics are in `world/desktop.ts`.

import { connectDesktop, desktopAvailable, dismissDesktopNotice, getWindowPresence, notifyDesktop } from "../world/desktop";
import type { Address } from "../world/events";
import type { ResidentNow } from "../world/world";
import { placeOf, talkLink } from "./places";
import { effect } from "./signals";
import { residentsOf, type Store } from "./store";

export interface PresenceWords {
  waiting(who: string): string;
  waitingBody(town: string | null): string;
  finished(who: string): string;
  finishedBody(town: string | null, last: string | null): string;
  open: string;
  stop: string;
}

export interface PresenceOptions {
  store: Store;
  words: PresenceWords;
  /** "Codex · laptop". */
  someone(resident: ResidentNow): string;
  /** Stops the turn of the conversation a notification is about. */
  stop(address: Address): Promise<unknown>;
}

const KEY = "notify";

/** Whether the user wants to be called; on unless they turned it off. */
export function notifying(): boolean {
  try {
    return localStorage.getItem(KEY) !== "off";
  } catch {
    return true;
  }
}

export function chooseNotifying(on: boolean): void {
  try {
    if (on) localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, "off");
  } catch {
    // Not kept: the next start calls the user again.
  }
}

export function startPresence(options: PresenceOptions): void {
  if (!desktopAvailable()) return;
  const { store, words } = options;
  void connectDesktop({
    async onActivation({ route, action }) {
      if (route) location.hash = route;
      const target = route ? placeOf(route) : null;
      if (action === "stop" && target?.kind === "talk") await options.stop(target.address);
    },
    onError(error, notificationId) {
      console.error("World desktop", notificationId, error);
    },
  }).catch((error: unknown) => console.error("Connecting World's desktop", error));

  // What each conversation was last seen doing; the first look only learns.
  let seen: Map<string, { waiting: boolean; busy: boolean }> | null = null;
  const shown = new Map<string, string>();
  const key = (r: ResidentNow) => `${r.about.id}@${r.about.machine}`;

  effect(() => {
    const world = store.world.get();
    if (!world) return;
    const residents = residentsOf(world);
    const before = seen;
    seen = new Map(residents.map((r) => [key(r), { waiting: !!r.now?.waiting_for_user, busy: r.now?.activity === "busy" }]));
    for (const r of residents) {
      const id = key(r);
      const now = seen.get(id)!;
      // What a notification called for no longer holds: it is taken back.
      if (!now.waiting && shown.has(`${id}:waiting`)) {
        void dismissDesktopNotice(shown.get(`${id}:waiting`)!).catch(() => {});
        shown.delete(`${id}:waiting`);
      }
      const was = before?.get(id);
      if (!before || !notifying()) continue;
      const address = { resident: r.about.id, machine: r.about.machine };
      if (now.waiting && !was?.waiting) {
        void call(`${id}:waiting`, {
          title: words.waiting(options.someone(r)),
          body: words.waitingBody(r.town),
          route: talkLink(address),
          actions: [{ id: "open", title: words.open }],
        });
      } else if (was?.busy && !now.busy && !now.waiting && r.about.origin.kind === "user" && r.about.world_started) {
        // Only conversations opened through the World: one the user opened
        // in an editor or an app is in front of them there.
        void call(`${id}:finished`, {
          title: words.finished(options.someone(r)),
          body: words.finishedBody(r.town, r.last_action?.what ?? null),
          route: talkLink(address),
          actions: [{ id: "open", title: words.open }],
        });
      }
    }
  });

  /** Calls the user, unless the window is in front of them already. */
  async function call(id: string, notice: { title: string; body: string; route: string; actions: { id: string; title: string }[] }): Promise<void> {
    const presence = await getWindowPresence();
    if (presence.focused) return;
    const previous = shown.get(id);
    if (previous) await dismissDesktopNotice(previous).catch(() => {});
    shown.set(id, await notifyDesktop(notice));
  }
}
