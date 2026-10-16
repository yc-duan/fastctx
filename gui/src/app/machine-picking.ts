// Machines chosen together, on the drawing and the list alike (`world-gui.md`,
// 账号、机器、共享与协助): which can be chosen, which a box or a group's band
// holds, and how a press with Ctrl, a frame or a box changes those chosen.
// Only machines the account works on are chosen this way: not the hub, a
// phone, the machine of an agent that assists or a visitor's.

import { boxParts } from "./machine-groups";
import type { MapNode } from "./map-nodes";

/** Whether a machine on the drawing or the list may be chosen with others. */
export function pickable(node: MapNode): boolean {
  return (node.hold === "mine" || node.hold === "others") && !node.device;
}

/** The machines that may be chosen in a box or a group's band, by its key on
 *  the drawing (`group:<hub>:<kind>`), its "⋯" (`gm:`, `who:`) or the list's
 *  heading (`list:<kind>`, on any hub). */
export function pickableIn(nodes: readonly MapNode[], key: string): string[] {
  const parsed = boxOf(key);
  if (!parsed) return [];
  const { hub, kind } = parsed;
  const band = boxParts(kind).group !== null;
  return nodes
    .filter((node) => pickable(node) && (hub === null || node.hubs.includes(hub)))
    .filter((node) => (band ? node.group?.key === kind : (node.group?.within?.key ?? node.group?.key ?? "mine") === kind))
    .map((node) => node.key);
}

/** The hub and box kind a key stands for: no hub for a list's heading,
 *  which holds its machines on every hub. A hub's id has no colon. */
function boxOf(key: string): { hub: string | null; kind: string } | null {
  const drawn = key.match(/^(?:group|gm):([^:]+):(.+)$/);
  if (drawn) return { hub: drawn[1]!, kind: drawn[2]! };
  const person = key.match(/^who:([^:]+):(.+)$/);
  if (person) return { hub: person[1]!, kind: `of:${person[2]!}` };
  const listed = key.match(/^list:(.+)$/);
  return listed ? { hub: null, kind: listed[1]! } : null;
}

/** `chosen` with `key` added, or taken out when it was there. */
export function toggled(chosen: readonly string[], key: string): string[] {
  return chosen.includes(key) ? chosen.filter((one) => one !== key) : [...chosen, key];
}

/** `chosen` with each of `keys` added, in their order after those there. */
export function added(chosen: readonly string[], keys: readonly string[]): string[] {
  return [...chosen, ...keys.filter((key) => !chosen.includes(key))];
}

/** The keys from `from` to `to` in `order`, both ends in, either way round;
 *  `to` alone when `from` is not there. */
export function between(order: readonly string[], from: string | null, to: string): string[] {
  const a = from === null ? -1 : order.indexOf(from);
  const b = order.indexOf(to);
  if (b < 0) return [];
  if (a < 0) return [to];
  return order.slice(Math.min(a, b), Math.max(a, b) + 1);
}
