// Reachability belongs to the current World, never to a resident object's identity.

import type { WorldNow } from "../world/bridge";
import type { ResidentNow } from "../world/world";
import { indexOf, pair } from "./world-index";

export type MachineReach = "listed" | "unknown" | "not_granted" | "stopped";

/** A listed machine never counts as departed, even if a concurrent grant decision differs. */
export function machineReach(world: WorldNow | null, machine: string): MachineReach {
  if (!world) return "unknown";
  const index = indexOf(world);
  if (machine === world.here || index.who.has(machine)) return "listed";
  const known = index.presence.get(machine);
  const hubs = world.accounts.hubs;
  if (!known || !hubs.length) return "unknown";
  const decisions = hubs.map((hub) => known.hubs.find((one) => one.hub === hub.hub));
  if (decisions.some((one) => !one || one.access === "unknown" || one.access === "granted" || (one.access === "not_granted" && !one.current))) return "unknown";
  if (decisions.some((one) => one?.access === "stopped")) return "stopped";
  return decisions.every((one) => one?.access === "not_granted" && one.current) ? "not_granted" : "unknown";
}

/** Positive directory changes wake a refused watch; this token grants no permission itself. */
export function lookRevision(world: WorldNow | null, machine: string): string | null {
  if (!world) return null;
  const known = indexOf(world).presence.get(machine);
  if (known) {
    const hubs = new Set(world.accounts.hubs.map((hub) => hub.hub));
    const granted = known.hubs.filter((one) => hubs.has(one.hub) && one.access === "granted");
    return granted.length ? JSON.stringify(granted.map((one) => [one.hub, one.revision, !!one.current])) : null;
  }
  // An older producer has no permission revision: its changed grants can still prompt a new request.
  const bare = machine.split(".")[0]!;
  return JSON.stringify(world.accounts.hubs.map((hub) => [hub.hub, hub.link.kind, hub.assists.filter((one) => one.machine === bare), hub.shares.filter((one) => one.machine === bare)]));
}

/** A retained resident is unavailable only while the current listing does not answer for it. */
export function isUnreached(world: WorldNow | null, resident: ResidentNow): boolean {
  return !world || !indexOf(world).residents.has(pair(resident.about.machine, resident.about.id));
}

/** Only the node's explicit, current-directory decision establishes a lost grant. */
export function isDeparted(world: WorldNow | null, resident: ResidentNow): boolean {
  return machineReach(world, resident.about.machine) === "not_granted";
}
