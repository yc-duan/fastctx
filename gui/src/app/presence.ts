// When the World calls the user from outside the window: an agent asked
// them something, told them something through `message_user` (a reply, or
// something urgent; what can wait is not pushed), a turn the user's words
// began ended without telling them anything, or a device or a machine came
// into one of their accounts, while the window is not in front of them
// (`world-voice.md`). The title says who; the body is what the agent said,
// its question or its reply, never a summary. A question asked without waiting
// with a few answers to choose from carries them as buttons, answered from
// the notification without opening the window. A notification is taken
// back once what it called for no longer holds, and a click on it opens
// that conversation. What a conversation says is not called about while the
// window is in front of the user showing that conversation; from any other
// page it is. A network this computer was on, dissolved, is called about
// once too. The shell's own mechanics are in `world/desktop.ts`.

import { connectDesktop, desktopAvailable, dismissDesktopNotice, getWindowPresence, notifyDesktop } from "../world/desktop";
import type { Address } from "../world/events";
import type { Asked, ResidentNow } from "../world/world";
import type { MachineNotice } from "../world/accounts";
import { link, placeOf, talkLink } from "./places";
import { effect } from "./signals";
import { residentsOf, type Store } from "./store";

export interface PresenceWords {
  /** The agent asks the user something; `what` is what it wants of them,
   *  `waiting` when its host said no more than that it waits. */
  asked(who: string, what: Asked["what"] | "waiting"): string;
  finished(who: string): string;
  /** The agent told the user something they should know soon. */
  urgent(who: string): string;
  /** What a turn that compacted the conversation and said nothing else says. */
  compacted: string;
  open: string;
  stop: string;
  /** A device or a machine came into `login`, the account it logged in to. */
  arrived(name: string, device: boolean, login: string): string;
  /** Managed machines coming in by a code, called about once a batch. */
  arrivedManaged(login: string): string;
  arrivedBody: string;
  /** Networks this computer was on, dissolved by their administrators. */
  dissolved(hubs: string[]): string;
  dissolvedBody: string;
  /** What another who holds a machine with this account did to it. */
  machineNotice(notice: MachineNotice): string;
}

export interface PresenceOptions {
  store: Store;
  words: PresenceWords;
  /** "Codex laptop". */
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

/** What a notification about a resident's question is keyed by: a new
 *  question is a new call; none when it asks nothing. A host that only
 *  says it waits is a question too. */
export function askedKey(now: ResidentNow["now"]): string | null {
  return now?.asked?.id ?? (now?.waiting_for_user ? "waiting" : null);
}

/** Whether this window shows the conversation at `address`. */
function viewing(address: Address): boolean {
  const place = placeOf(location.hash);
  return place.kind === "talk" && place.address.resident === address.resident && place.address.machine === address.machine;
}

export function startPresence(options: PresenceOptions): void {
  if (!desktopAvailable()) return;
  const { store, words } = options;
  void connectDesktop({
    async onActivation({ route, action }) {
      const target = route ? placeOf(route) : null;
      if (route) location.hash = route;
      if (action === "stop" && target?.kind === "talk") await options.stop(target.address);
    },
    onError(error, notificationId) {
      console.error("World desktop", notificationId, error);
    },
  }).catch((error: unknown) => console.error("Connecting World's desktop", error));

  // Who came into an account this computer or device is logged in to:
  // each called about once, the first look only learns. Managed machines
  // that come in by one code are a batch, called about once while it is
  // not yet seen (`world-network.md`, 受管机器).
  let arrived: Set<string> | null = null;
  effect(() => {
    const world = store.world.get();
    if (!world) return;
    const now = world.accounts.hubs.flatMap((hub) => hub.arrivals.map((arrival) => ({ hub: hub.name, arrival })));
    const key = ({ hub, arrival }: (typeof now)[number]) => `${hub}/${arrival.account}/${arrival.code ? `code:${arrival.code}` : arrival.name}`;
    const before = arrived;
    arrived = new Set(now.map(key));
    if (!before || !notifying()) return;
    const called = new Set<string>();
    for (const one of now.filter((one) => !before.has(key(one)))) {
      if (called.has(key(one))) continue;
      called.add(key(one));
      const where = `${one.hub} ${one.arrival.account}`;
      void call(`arrived:${key(one)}`, {
        title: one.arrival.code ? words.arrivedManaged(where) : words.arrived(one.arrival.name, one.arrival.device, where),
        body: words.arrivedBody,
        route: link({ kind: "account" }),
        actions: [{ id: "open", title: words.open }],
      });
    }
  });

  // What others did to machines an account here holds, each called about
  // once; the first look only learns.
  let noticed: Set<string> | null = null;
  effect(() => {
    const world = store.world.get();
    if (!world) return;
    const now = world.accounts.hubs.flatMap((hub) => (hub.machine_notices ?? []).map((notice) => ({ hub: hub.name, notice })));
    const key = ({ hub, notice }: (typeof now)[number]) => `${hub}/${notice.account}/${notice.id}`;
    const before = noticed;
    noticed = new Set(now.map(key));
    if (!before || !notifying()) return;
    for (const one of now.filter((one) => !before.has(key(one)))) {
      const gone = one.notice.action.kind === "remove" || (one.notice.action.kind === "share" && !one.notice.action.shared && one.notice.action.to === one.notice.account);
      const name = one.notice.action.kind === "rename" ? one.notice.action.to : one.notice.machine;
      void call(`machine:${key(one)}`, {
        title: words.machineNotice(one.notice),
        body: `${one.hub} ${one.notice.account}`,
        route: gone ? link({ kind: "machines" }) : link({ kind: "machine", name }),
        actions: [{ id: "open", title: words.open }],
      });
    }
  });

  // The networks dissolved, each called about once; the first look only
  // learns, and the window says them itself when it opens.
  let dissolved: Set<string> | null = null;
  effect(() => {
    const world = store.world.get();
    if (!world) return;
    const now = world.accounts.dissolved ?? [];
    const key = (one: (typeof now)[number]) => `${one.hub}@${one.at}`;
    const before = dissolved;
    dissolved = new Set(now.map(key));
    if (!before || !notifying()) return;
    const fresh = now.filter((one) => !before.has(key(one)));
    if (!fresh.length) return;
    void call(`dissolved:${fresh.map(key).join(",")}`, {
      title: words.dissolved([...new Set(fresh.map((one) => one.hub))]),
      body: words.dissolvedBody,
      route: link({ kind: "machines" }),
      actions: [{ id: "open", title: words.open }],
    });
  });

  // What each conversation was last seen doing; the first look only learns.
  let seen: Map<string, { asked: string | null; busy: boolean; told: string | null }> | null = null;
  const shown = new Map<string, string>();
  const key = (r: ResidentNow) => `${r.about.id}@${r.about.machine}`;

  effect(() => {
    const world = store.world.get();
    if (!world) return;
    const residents = residentsOf(world);
    const before = seen;
    seen = new Map(residents.map((r) => [key(r), { asked: askedKey(r.now), busy: r.now?.activity === "busy", told: r.now?.told?.id ?? null }]));
    for (const r of residents) {
      const id = key(r);
      const now = seen.get(id)!;
      const was = before?.get(id);
      // What a notification called for no longer holds: it is taken back.
      if (now.asked !== (was?.asked ?? null) && shown.has(`${id}:asked`)) {
        void dismissDesktopNotice(shown.get(`${id}:asked`)!).catch(() => {});
        shown.delete(`${id}:asked`);
      }
      if (!before || !notifying()) continue;
      const address = { resident: r.about.id, machine: r.about.machine };
      if (now.asked && now.asked !== was?.asked) {
        const asked = r.now?.asked;
        void call(`${id}:asked`, {
          title: words.asked(options.someone(r), asked?.what ?? "waiting"),
          body: asked?.text ?? "",
          route: talkLink(address),
          actions: [{ id: "open", title: words.open }],
        }, address);
      } else if (was && now.told && now.told !== was.told && r.now?.told && r.now.told.status !== "info") {
        // What can wait is not pushed: it waits on the home page. Only what
        // is said while the conversation is in view of this window: one that
        // first appears is learned, not called about.
        const told = r.now.told;
        void call(`${id}:told`, {
          title: told.status === "urgent" ? words.urgent(options.someone(r)) : words.finished(options.someone(r)),
          body: told.text,
          route: talkLink(address),
          actions: [{ id: "open", title: words.open }],
        }, address);
      } else if (was?.busy && !now.busy && !now.asked && r.about.origin.kind === "user" && r.about.world_started && endedUntold(r.now)) {
        // Only conversations opened through the World: one the user opened
        // in an editor or an app is in front of them there.
        void call(`${id}:finished`, {
          title: words.finished(options.someone(r)),
          body: r.now?.last_compacted ? words.compacted : (r.now?.last_said ?? ""),
          route: talkLink(address),
          actions: [{ id: "open", title: words.open }],
        }, address);
      }
    }
  });

  /** Whether the turn that just ended stands for an answer: one the user's
   *  words began that told them nothing through `message_user`. A node that
   *  does not say whose words began it is taken as before. */
  function endedUntold(now: ResidentNow["now"]): boolean {
    if (!now || now.last_heard_user === false) return false;
    return !now.told?.turn || now.told.turn !== now.last_turn;
  }

  /** Calls the user, unless the window is in front of them already, and,
   *  for what the conversation at `about` says, shows that conversation. */
  async function call(id: string, notice: { title: string; body: string; route: string; actions: { id: string; title: string; quiet?: boolean }[] }, about?: Address): Promise<void> {
    const presence = await getWindowPresence();
    if (presence.focused && (!about || viewing(about))) return;
    const previous = shown.get(id);
    if (previous) await dismissDesktopNotice(previous).catch(() => {});
    shown.set(id, await notifyDesktop(notice));
  }
}
