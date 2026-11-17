// Last-known content survives an incomplete listing; only explicit deletion releases it.

import { loginsOf } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { Address } from "../world/events";
import type { ResidentNow } from "../world/world";
import { assistingOwner } from "./reach";
import { isUnreached } from "./reachability";
import { indexOf } from "./world-index";

interface Answered {
  residents: ResidentNow[];
  logins: { hub: string; account: string }[] | null;
  assisting: string | null;
}

const lastAnswered = new Map<string, Answered>();
const keptKey = "fastctx.last-answered";

function loginsThere(world: WorldNow, machine: string): { hub: string; account: string }[] {
  const [bare, network] = machine.split(".");
  return [...indexOf(world).hubs.values()]
    .filter(({ hub }) => !network || hub.name === network)
    .flatMap(({ hub, computer }) => (computer.get(bare!)?.logins ?? []).map((account) => ({ hub: hub.hub, account })));
}

/** A current answer updates what it contains; missing residents remain last-known content. */
export function remember(world: WorldNow): void {
  const answered = (machine: string, residents: ResidentNow[]) => {
    const previous = lastAnswered.get(machine);
    const named = new Set(residents.map((resident) => resident.about.id));
    const logins = loginsThere(world, machine);
    lastAnswered.set(machine, {
      residents: [...residents, ...(previous?.residents.filter((resident) => !named.has(resident.about.id)) ?? [])],
      logins: logins.length ? logins : (previous?.logins ?? []),
      assisting: assistingOwner(world, machine) ?? previous?.assisting ?? null,
    });
  };
  for (const machine of world.who.machines) {
    if (machine.kind === "answered") answered(machine.reply.machine, machine.reply.residents);
    if (machine.kind === "offline" && machine.last) answered(machine.machine, machine.last.residents);
  }
}

export function knownMachines(): string[] {
  return [...lastAnswered.keys()];
}

export function recall(): void {
  try {
    const kept = JSON.parse(localStorage.getItem(keptKey) ?? "{}") as Record<string, unknown>;
    for (const [machine, one] of Object.entries(kept)) {
      if (Array.isArray(one)) lastAnswered.set(machine, { residents: one as ResidentNow[], logins: null, assisting: null });
      else if (one && typeof one === "object" && Array.isArray((one as Answered).residents)) {
        const { residents, logins, assisting } = one as Answered;
        lastAnswered.set(machine, { residents, logins: Array.isArray(logins) ? logins : null, assisting: typeof assisting === "string" ? assisting : null });
      }
    }
  } catch {
    // Unavailable storage does not change the current World's conclusions.
  }
}

export function keep(): void {
  try {
    localStorage.setItem(keptKey, JSON.stringify(Object.fromEntries(lastAnswered)));
  } catch {
    // A later window starts from the node's listing when storage is unavailable.
  }
}

function accountsFor(world: WorldNow, resident: ResidentNow, kept: Answered): { hub: string; account: string }[] {
  const acting = resident.about.acting;
  if (acting && acting.kind !== "here") return acting.accounts;
  const named = loginsThere(world, resident.about.machine);
  return named.length ? named : (kept.logins ?? []);
}

/** Content not in this answer is retained without declaring its machine gone. */
export function unreachedOf(world: WorldNow | null, archived = false): ResidentNow[] {
  if (!world) return [];
  const ours = loginsOf(world.accounts);
  const current = new Set([...indexOf(world).residents.values()].map((resident) => resident.about.id));
  return [...lastAnswered.entries()].flatMap(([machine, kept]) =>
    machine === world.here ? [] : kept.residents.filter((resident) =>
      (archived || !resident.removed) && !current.has(resident.about.id) &&
      accountsFor(world, resident, kept).some((account) => ours.some((login) => login.hub === account.hub && login.account === account.account)),
    ),
  );
}

/** Historical attribution names retained content; it does not establish current permission. */
export function assistedBy(world: WorldNow | null, resident: ResidentNow): string | null {
  return assistingOwner(world, resident.about.machine) ?? (isUnreached(world, resident) ? (lastAnswered.get(resident.about.machine)?.assisting ?? null) : null);
}

/** Keeps last-known conversations under their account while its current listing is incomplete. */
export function unreachedFor(world: WorldNow | null, resident: ResidentNow, login: { hub: string; account: string }): boolean {
  if (!world || !isUnreached(world, resident)) return false;
  const kept = lastAnswered.get(resident.about.machine);
  return !!kept && accountsFor(world, resident, kept).some((account) => account.hub === login.hub && account.account === login.account);
}

export function forgetAnswered(): void {
  lastAnswered.clear();
  keep();
}

/** Removes this device's retained content after an explicit deletion or local forget. */
export function letGo(address: Address): void {
  // A renamed or relocated conversation can also be retained under its earlier machine name.
  for (const [machine, kept] of lastAnswered) {
    const residents = kept.residents.filter((resident) => resident.about.id !== address.resident);
    if (residents.length === kept.residents.length) continue;
    if (residents.length) lastAnswered.set(machine, { ...kept, residents });
    else lastAnswered.delete(machine);
  }
  keep();
}
