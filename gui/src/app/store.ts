// The World as the interface holds it: followed in this one place as the
// node or the phone tells each change, and read by every part that shows
// it; nothing is asked of any machine while nothing changes
// (`world-network.md`, 近况). Outside the shell there is no World to follow;
// a specimen stands in, so the interface can be looked at in a plain
// browser.

import { loginsOf } from "../world/accounts";
import { inShell, watchWorld, worldNow, type WorldNow } from "../world/bridge";
import { applyChange } from "../world/change";
import type { Address } from "../world/events";
import type { MachineWho, ResidentNow, RunningJob, WhoFor, WhoReply } from "../world/world";
import { keepNetwork, shownNetwork, viewOf } from "./network";
import { signal, type Signal } from "./signals";

export interface Store {
  /** Whether the World is really there, or a specimen stands in. */
  readonly live: boolean;
  /** The World as last read, as the network shown sees it (`./network`);
   *  `null` until it first answers. */
  readonly world: Signal<WorldNow | null>;
  /** The World as last read, every network this device is in. */
  readonly whole: Signal<WorldNow | null>;
  /** Shows another network this device is in. */
  showNetwork(hub: string): void;
  /** Why the World cannot be followed from the local service now, or
   *  `null`: the watch broke and has not come back, or, while it has given
   *  nothing since it broke, the World could not be read either. */
  readonly trouble: Signal<string | null>;
  /** Now, moving on while the window is in view, for what is shown as so
   *  long ago; or the specimen's moment. */
  readonly clock: Signal<Date>;
  /** How many seconds the local service has not given the World since it
   *  was first asked for it or the watch broke, 0 while it is followed.
   *  Nothing gives up on it; this says so instead of the page standing
   *  still in silence. */
  readonly unanswered: Signal<number>;
  /** Reads it again at once, as after the user changed something. */
  refresh(): Promise<void>;
}

/** How often what is shown as so long ago moves on: the World itself comes
 *  as it changes. */
const tick = 2000;

export function createStore(specimen: () => { world: WorldNow; now: Date }): Store {
  const live = inShell();
  const world = signal<WorldNow | null>(null);
  const whole = signal<WorldNow | null>(null);
  const read = (given: WorldNow) => {
    const now = written(given);
    whole.set(now);
    world.set(viewOf(now, shownNetwork(now)));
  };
  const showNetwork = (hub: string) => {
    keepNetwork(hub);
    const now = whole.peek();
    if (now) world.set(viewOf(now, shownNetwork(now)));
  };
  const trouble = signal<string | null>(null);
  const clock = signal(new Date());
  const unanswered = signal(0);

  if (!live) {
    const shown = specimen();
    remember(shown.world);
    read(shown.world);
    clock.set(shown.now);
    // A specimen changed by what is done to it is read again, as a copy so
    // that it reads as new.
    const again = async () => {
      const changed = structuredClone(specimen().world);
      remember(changed);
      read(changed);
    };
    return { live, world, whole, showNetwork, trouble, clock, unanswered, refresh: again };
  }

  recall();
  // Kept as the window goes out of view or closes, not on every answer.
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") keep();
  });
  window.addEventListener("pagehide", keep);

  // Counted from when the World was first asked for, or the watch broke,
  // until it comes again.
  let waiting: number | null = Date.now();
  const moveOn = () => {
    clock.set(new Date());
    unanswered.set(waiting === null ? 0 : Math.floor((Date.now() - waiting) / 1000));
  };
  // What is shown as so long ago moves on while the window is in view.
  let ticking = 0;
  const follow = () => {
    clearInterval(ticking);
    if (document.visibilityState !== "visible") return;
    moveOn();
    ticking = window.setInterval(moveOn, tick);
  };
  document.addEventListener("visibilitychange", follow);
  follow();

  const came = (now: WorldNow) => {
    remember(now);
    read(now);
    waiting = null;
    moveOn();
  };
  // Why the watch broke, while it has not come back; why reading the World
  // failed, while the watch gave nothing since it broke. What the watch
  // says is newer than any reading begun before it came, so a reading that
  // answers after it says nothing.
  let broken: string | null = null;
  let failed: string | null = null;
  let pieces = 0;
  // The World as the watch last told it: what changes next builds on it,
  // not on a reading `refresh` made meanwhile.
  let watched: WorldNow | null = null;
  const say = () => trouble.set(broken ?? failed);
  void watchWorld((piece) => {
    pieces += 1;
    if (piece.kind === "world" || piece.kind === "changed") {
      if (piece.kind === "changed" && !watched) return;
      watched = piece.kind === "world" ? piece.world : (applyChange(watched, piece.change) as WorldNow);
      broken = null;
      failed = null;
      say();
      came(watched);
      return;
    }
    broken = piece.error;
    say();
    waiting ??= piece.since;
    moveOn();
  }).catch((error: unknown) => {
    broken = String(error);
    say();
  });
  const refresh = async () => {
    const before = pieces;
    try {
      const now = await worldNow();
      if (pieces !== before) return;
      failed = null;
      say();
      came(now);
    } catch (error) {
      if (pieces !== before || broken === null) return;
      failed = String(error);
      say();
    }
  };
  return { live, world, whole, showNetwork, trouble, clock, unanswered, refresh };
}

/** What each machine last answered with about its residents. A machine
 *  away keeps its conversations listed: one not removed is always listed
 *  (`world-gui.md`). Kept in this device's storage as well, so a window
 *  opened while the machine is away lists them too. */
const lastAnswered = new Map<string, ResidentNow[]>();
const unreached = new WeakSet<ResidentNow>();
const keptKey = "fastctx.last-answered";

function remember(world: WorldNow): void {
  for (const machine of world.who.machines) {
    if (machine.kind === "answered") lastAnswered.set(machine.reply.machine, machine.reply.residents);
    // What the hub kept of a machine gone offline is the latest there is.
    if (machine.kind === "offline" && machine.last) lastAnswered.set(machine.machine, machine.last.residents);
  }
}

function recall(): void {
  try {
    const kept = JSON.parse(localStorage.getItem(keptKey) ?? "{}") as Record<string, ResidentNow[]>;
    for (const [machine, residents] of Object.entries(kept)) if (Array.isArray(residents)) lastAnswered.set(machine, residents);
  } catch {
    // Storage may be unavailable or hold something else; the list then
    // starts from what the machines answer.
  }
}

function keep(): void {
  try {
    localStorage.setItem(keptKey, JSON.stringify(Object.fromEntries(lastAnswered)));
  } catch {
    // Not kept: the list starts from what the machines answer next time.
  }
}

/** The residents of machines that did not answer this time, as they last
 *  answered: listed, marked as out of reach, never taken for live; those
 *  archived too when `archived`. Only those working for an account this
 *  device is logged in to now: what was kept under another login, or an
 *  older World that answered with every conversation, is not listed. */
export function unreachedOf(world: WorldNow | null, archived = false): ResidentNow[] {
  if (!world) return [];
  const ours = loginsOf(world.accounts);
  const theirs = (machine: string): { hub: string; account: string }[] =>
    world.accounts.hubs.flatMap((hub) => (hub.members.find((member) => member.name === bareName(machine))?.logins ?? []).map((account) => ({ hub: hub.hub, account })));
  const worksForUs = (resident: ResidentNow): boolean => {
    const acting = resident.about.acting;
    const accounts = !acting || acting.kind === "here" ? theirs(resident.about.machine) : acting.accounts;
    return accounts.some((account) => ours.some((login) => login.hub === account.hub && login.account === account.account));
  };
  return world.who.machines.flatMap((machine) => {
    if (machine.kind === "answered") return [];
    const residents = (lastAnswered.get(machine.machine) ?? []).filter((r) => (archived || !r.removed) && worksForUs(r));
    for (const r of residents) unreached.add(r);
    return residents;
  });
}

/** A copy of `original` shown in its place is out of reach as it is. */
export function sameReach(copy: ResidentNow, original: ResidentNow): void {
  if (unreached.has(original)) unreached.add(copy);
}

/** Whether a resident is one its machine did not answer for this time. */
export function isUnreached(r: ResidentNow): boolean {
  return unreached.has(r);
}

/** Every resident each machine answered with that is still in the World;
 *  one the user archived is answered with too, and left out here unless
 *  `archived`. */
export function residentsOf(world: WorldNow | null, archived = false): ResidentNow[] {
  return (world?.who.machines ?? []).flatMap((machine) => (machine.kind === "answered" ? machine.reply.residents : [])).filter((r) => archived || !r.removed);
}

/** One resident as its machine answered for it, removed or not. */
export function residentAt(world: WorldNow | null, address: Address): ResidentNow | null {
  return (
    (world?.who.machines ?? [])
      .flatMap((machine) => (machine.kind === "answered" ? machine.reply.residents : []))
      .find((r) => r.about.id === address.resident && r.about.machine === address.machine) ?? null
  );
}

/** Every job still running, on every machine that answered. */
export function jobsOf(world: WorldNow | null): RunningJob[] {
  return (world?.who.machines ?? []).flatMap((machine) => (machine.kind === "answered" ? machine.reply.jobs : []));
}

/** Whom the resident or job `id` on `machine` works for, when that is other
 *  accounts than the viewer's: "bob", or "bob, carol". A machine whose
 *  agents assist several accounts shows what each runs there, marked with
 *  whose it is. */
export function worksFor(others: WhoFor[] | undefined, machine: string, id: string): string | null {
  const other = others?.find((o) => o.machine === machine && o.id === id);
  return other ? other.accounts.join(", ") : null;
}

export const sameAddress = (a: Address, b: Address) => a.resident === b.resident && a.machine === b.machine;

export const addressOf = (r: ResidentNow): Address => ({ resident: r.about.id, machine: r.about.machine });

/** A machine's name without the hub it is written with: what a hub's
 *  directory knows it by. */
export const bareName = (written: string): string => written.split(".")[0]!;

/** Each machine of `now` as this computer writes it, `name.hub` where two
 *  of its hubs each have a different computer of that name, as the node
 *  says (`Node.written`); and so the machine of each conversation and
 *  command shown, so that every call made of them names the one meant. */
function written(now: WorldNow): WorldNow {
  // A machine, its conversations and its jobs as named where the World was
  // read; each object changed is a new one, the rest are the same objects.
  const answer = (reply: WhoReply, as: string | undefined): WhoReply => {
    const residents = reply.residents.map((r) => {
      const machine = r.source?.written ?? as;
      return machine && machine !== r.about.machine ? { ...r, about: { ...r.about, machine } } : r;
    });
    const jobs = reply.jobs.map((job) => {
      const machine = job.source?.written ?? as;
      return machine && machine !== job.machine ? { ...job, machine } : job;
    });
    const renamed = !!as && as !== reply.machine;
    if (!renamed && residents.every((r, i) => r === reply.residents[i]) && jobs.every((job, i) => job === reply.jobs[i])) return reply;
    return { ...reply, ...(renamed ? { machine: as } : {}), residents, jobs };
  };
  const machines = (now.who?.machines ?? []).map((machine): MachineWho => {
    const as = machine.observation?.source.written;
    if (machine.kind === "answered") {
      const reply = answer(machine.reply, as);
      return reply === machine.reply ? machine : { ...machine, reply };
    }
    let next: MachineWho = as && as !== machine.machine ? { ...machine, machine: as } : machine;
    if (next.kind === "offline" && next.last) {
      const last = answer(next.last, as);
      if (last !== next.last) next = { ...next, last };
    }
    return next;
  });
  if (!now.who || machines.every((machine, i) => machine === now.who.machines[i])) return now;
  return { ...now, who: { ...now.who, machines } };
}
