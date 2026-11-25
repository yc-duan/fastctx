// What the other machines' answers stand at, kind by kind
// (`fastctx_types::numbers`; `interaction-design`, 变化编号): their hubs tell
// them beside each machine in the World. A page holding a machine's answer
// reads it again once that machine's number of the kind moved, and not
// before; a machine whose number is not told (an older node or hub, one
// offline, a kind not told to the logins here) is read on the page's own
// timer, as every machine was before.

import type { WorldNow } from "../world/bridge";
import type { Numbers } from "../world/world";
import { followState, type Following } from "../world/topics";
import { machinesOf } from "./reach";
import { effect, type Signal } from "./signals";

export type NumberKind = keyof Numbers;

/** How long a machine whose number is told goes unread all the same: a
 *  move may have gone unseen while the World was not followed. */
export const QUIET_MS = 10 * 60_000;

/** Where each machine's number of `kind` stands in `world`, by how the
 *  machine is written; one not telling it is left out. */
export function numbersOf(world: WorldNow | null, kind: NumberKind): Map<string, number> {
  const told = new Map<string, number>();
  for (const machine of machinesOf(world)) {
    const number = machine.numbers?.[kind];
    if (number !== undefined) told.set(machine.written, number);
  }
  return told;
}

/** Calls `moved` with the machines, as written, whose number of `kind`
 *  moved as `world` changed, that began to tell one, or that tell none any
 *  more (gone offline, or away); what stood when it began is taken as read.
 *  Returns what stops it. */
export function followNumbers(world: Signal<WorldNow | null>, kind: NumberKind, moved: (machines: string[]) => void): () => void {
  let known: Map<string, number> | null = null;
  return effect(() => {
    const given = world.get();
    if (!given) return;
    const now = numbersOf(given, kind);
    const was = known;
    known = now;
    if (!was) return;
    const changed = [...now].filter(([machine, number]) => was.get(machine) !== number).map(([machine]) => machine);
    for (const machine of was.keys()) if (!now.has(machine)) changed.push(machine);
    // Called apart from the effect, so that what it reads is not followed.
    if (changed.length) queueMicrotask(() => moved(changed));
  });
}

/** Reads with `read`, `begun` as it begins; one that fails calls `failed`,
 *  so the next tick of the clock reads again rather than taking the move as
 *  read, and fails as it did. */
function readMarked(read: () => void | Promise<unknown>, begun: () => void, failed: () => void): void | Promise<unknown> {
  begun();
  let result: void | Promise<unknown>;
  try {
    result = read();
  } catch (error) {
    failed();
    throw error;
  }
  return result instanceof Promise
    ? result.catch((error: unknown) => {
        failed();
        throw error;
      })
    : result;
}

/** Calls `read` whenever `machine`'s answers of `kind` may have changed:
 *  once its number moved, where its hub tells it; every `every` while it
 *  tells none; every `QUIET_MS` regardless. Nothing is read while the
 *  window is out of view: a number that moved meanwhile is read as soon as
 *  it is back. The first reading is the caller's. Returns what stops it. */
export function followMachine(world: Signal<WorldNow | null>, kind: NumberKind, machine: string, read: () => void | Promise<unknown>, every: number): () => void {
  let last = Date.now();
  let missed = false;
  let first = true;
  const now = () => readMarked(read, () => {
    missed = false;
    last = Date.now();
  }, () => {
    missed = true;
  });
  const stopNumbers = followNumbers(world, kind, (machines) => {
    if (!machines.includes(machine)) return;
    if (document.hidden) missed = true;
    else void Promise.resolve(now()).catch(() => {});
  });
  const timer = followState(null, () => {
    if (first) {
      first = false;
      return;
    }
    const told = numbersOf(world.peek(), kind).has(machine);
    if (missed || !told || Date.now() - last >= QUIET_MS) return now();
  }, every);
  return () => {
    stopNumbers();
    timer();
  };
}

/** Calls `read` whenever what it reads of every machine may have changed:
 *  as `topic` moves on this computer, first as it opens; as any other
 *  machine's number of `kind` moves; and on the clock, every `every` while
 *  a machine online tells no such number, else every `QUIET_MS`. Nothing
 *  is read while the window is out of view: a number that moved meanwhile
 *  is read as soon as it is back. */
export function followAll(world: Signal<WorldNow | null>, kind: NumberKind, topic: string, read: () => void | Promise<unknown>, every: number | (() => number | false)): Following {
  let last = Date.now();
  let missed = false;
  let first = true;
  const now = () => readMarked(read, () => {
    missed = false;
    last = Date.now();
  }, () => {
    missed = true;
  });
  const untold = () => machinesOf(world.peek()).some((machine) => !machine.here && machine.online === true && machine.numbers?.[kind] === undefined);
  const here = followState(topic, now, every);
  const stopNumbers = followNumbers(world, kind, () => {
    if (document.hidden) missed = true;
    else void Promise.resolve(now()).catch(() => {});
  });
  const timer = followState(null, () => {
    if (first) {
      first = false;
      return;
    }
    if (missed || untold() || Date.now() - last >= QUIET_MS) return now();
  }, every);
  return Object.assign(
    () => {
      here();
      stopNumbers();
      timer();
    },
    {
      refresh: () => {
        here.refresh();
        timer.refresh();
      },
    },
  );
}
