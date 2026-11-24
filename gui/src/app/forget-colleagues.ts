// The colleagues deleting a conversation takes with it (the user,
// 2026-11-23: 删除一段会话，那这段会话他派发的同事也要一并删除): those it
// asked and those they asked in turn, as the World lists them.

import type { Address } from "../world/events";
import type { ResidentNow } from "../world/world";

/** How many colleague conversations go with the conversation at `address`:
 *  each that names it, or one of those, as its asker. */
export function colleaguesAsked(residents: readonly ResidentNow[], address: Address): number {
  const key = (machine: string, resident: string): string => `${machine}\n${resident}`;
  const going = new Set<string>();
  let askers = new Set([key(address.machine, address.resident)]);
  while (askers.size > 0) {
    const next = new Set<string>();
    for (const r of residents) {
      const origin = r.about.origin;
      if (origin.kind !== "colleague") continue;
      const own = key(r.about.machine, r.about.id);
      if (going.has(own) || !askers.has(key(origin.asked_by.machine, origin.asked_by.resident))) continue;
      going.add(own);
      next.add(own);
    }
    askers = next;
  }
  return going.size;
}
