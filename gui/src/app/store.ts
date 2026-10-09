// The World as the interface holds it: asked once every two seconds while
// the window is in view, by this one place, and read by every part that
// shows it. Outside the shell there is no World to ask; a specimen stands
// in, so the interface can be looked at in a plain browser.

import { inShell, worldNow, type WorldNow } from "../world/bridge";
import type { Address } from "../world/events";
import type { ResidentNow, RunningJob, WhoFor } from "../world/world";
import { signal, type Signal } from "./signals";

export interface Store {
  /** Whether the World is really there, or a specimen stands in. */
  readonly live: boolean;
  /** The World as last read; `null` until it first answers. */
  readonly world: Signal<WorldNow | null>;
  /** Why the World could not be asked the last time, or `null`. */
  readonly trouble: Signal<string | null>;
  /** When the World was last read, or the specimen's moment. */
  readonly clock: Signal<Date>;
  /** How many seconds the World has been asked without answering yet, 0
   *  while it is not being asked. Nothing gives up on it: a machine that
   *  keeps its connection but never answers holds the answer back, and
   *  this says so instead of the page standing still in silence. */
  readonly unanswered: Signal<number>;
  /** Asks again at once, as after the user changed something. */
  refresh(): Promise<void>;
}

const interval = 2000;

export function createStore(specimen: () => { world: WorldNow; now: Date }): Store {
  const live = inShell();
  const world = signal<WorldNow | null>(null);
  const trouble = signal<string | null>(null);
  const clock = signal(new Date());
  const unanswered = signal(0);

  if (!live) {
    const shown = specimen();
    world.set(shown.world);
    clock.set(shown.now);
    return { live, world, trouble, clock, unanswered, refresh: async () => {} };
  }

  let timer = 0;
  let asking: Promise<void> | null = null;
  const ask = (): Promise<void> => {
    if (asking) return asking;
    const began = Date.now();
    const counting = window.setInterval(() => unanswered.set(Math.floor((Date.now() - began) / 1000)), 1000);
    asking = worldNow()
      .then((now) => {
        world.set(now);
        trouble.set(null);
      })
      .catch((error: unknown) => trouble.set(String(error)))
      .finally(() => {
        clearInterval(counting);
        unanswered.set(0);
        clock.set(new Date());
        asking = null;
      });
    return asking;
  };
  // A window out of view asks nothing; coming back into view asks at once.
  const loop = async () => {
    clearTimeout(timer);
    if (document.visibilityState !== "visible") return;
    await ask();
    timer = window.setTimeout(loop, interval);
  };
  document.addEventListener("visibilitychange", () => void loop());
  void loop();
  return { live, world, trouble, clock, unanswered, refresh: ask };
}

/** Every resident each machine answered with that is still in the World;
 *  one the user removed is answered with too, and left out here. */
export function residentsOf(world: WorldNow | null): ResidentNow[] {
  return (world?.who.machines ?? []).flatMap((machine) => (machine.kind === "answered" ? machine.reply.residents : [])).filter((r) => !r.removed);
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
 *  accounts than the viewer's: "bob", or "bob, carol". A machine lent to
 *  several accounts shows what each runs there, marked with whose it is. */
export function worksFor(others: WhoFor[] | undefined, machine: string, id: string): string | null {
  const other = others?.find((o) => o.machine === machine && o.id === id);
  return other ? other.accounts.join(", ") : null;
}

export const sameAddress = (a: Address, b: Address) => a.resident === b.resident && a.machine === b.machine;

export const addressOf = (r: ResidentNow): Address => ({ resident: r.about.id, machine: r.about.machine });
