// The networks this device is in, and the one the interface shows
// (`world-gui.md`, 界面的结构): the name at the top of the sidebar is the
// network's, its server's name; pressed, it lists every network this
// device is in, and choosing one shows that network alone: its map,
// machines, projects and conversations. The choice is kept on this device.
// A computer in no network is a world of one, named after itself.

import type { HubAccounts } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { MachineWho, ResidentNow } from "../world/world";

const KEPT = "network";

export interface Network {
  hub: string;
  name: string;
  /** Whether one of this device's accounts there is its administrator. */
  admin: boolean;
  /** The accounts this device is logged in to there. */
  logins: string[];
  online: boolean;
  /** How many machines of it this device reaches. */
  machines: number;
  addresses: string[];
}

export function networksOf(world: WorldNow | null): Network[] {
  return (world?.accounts.hubs ?? []).map((hub) => ({
    hub: hub.hub,
    name: hub.name,
    admin: hub.accounts.some((account) => account.level === "super" && hub.logins.includes(account.name)),
    logins: hub.logins,
    online: hub.link.kind === "online",
    machines: world?.nodes.hubs.find((nodes) => nodes.hub === hub.hub)?.machines.length ?? 0,
    addresses: hub.addresses,
  }));
}

export function chosenNetwork(): string | null {
  try {
    return localStorage.getItem(KEPT);
  } catch {
    return null;
  }
}

export function keepNetwork(hub: string): void {
  try {
    localStorage.setItem(KEPT, hub);
  } catch {
    // Not kept: the first network is shown next time.
  }
}

/** The network shown: the one chosen on this device while it is still in
 *  it, else the first. */
export function shownNetwork(world: WorldNow | null): string | null {
  const hubs = world?.accounts.hubs ?? [];
  const chosen = chosenNetwork();
  return hubs.find((hub) => hub.hub === chosen)?.hub ?? hubs[0]?.hub ?? null;
}

/** What each machine read last showed in one network, by the machine read
 *  and what decided it: one unchanged keeps its object. */
const shownAs = new WeakMap<MachineWho, { by: string; shown: MachineWho | null }>();

/** The World as one network shows it. With one network or none it is the
 *  World as it was read. */
export function viewOf(world: WorldNow, hub: string | null): WorldNow {
  const hubs = world.accounts.hubs;
  if (hubs.length < 2 || hub === null) return world;
  const shown = hubs.find((one) => one.hub === hub);
  if (!shown) return world;
  const others = hubs.filter((one) => one.hub !== hub);
  const nodes = world.nodes.hubs.find((one) => one.hub === hub);
  const machines = new Set([world.here, ...(nodes?.machines.map((machine) => machine.name) ?? [])]);
  // A project belongs to the network whose server keeps its ledger; one no
  // server knows is this computer's own and shows in every network.
  const elsewhere = new Set(others.flatMap((one) => one.projects.map((project) => project.name)));
  const own = new Set(shown.projects.map((project) => project.name));
  const keepsTown = (town: string | null) => !town || own.has(town) || !elsewhere.has(town);
  const keepsResident = (resident: ResidentNow) =>
    keepsTown(resident.town) && (resident.about.acting.kind === "here" || resident.about.acting.accounts.some((account) => account.hub === hub));
  const by = [hub, [...machines].sort().join(" "), [...own].sort().join(" "), [...elsewhere].sort().join(" ")].join("|");
  const who = (machine: MachineWho): MachineWho | null => {
    const kept = shownAs.get(machine);
    if (kept?.by === by) return kept.shown;
    const name = machine.kind === "answered" ? machine.reply.machine : machine.machine;
    let shown: MachineWho | null = null;
    if (machines.has(name)) {
      const residents = machine.kind === "answered" ? machine.reply.residents.filter(keepsResident) : null;
      shown = machine.kind !== "answered" || residents!.length === machine.reply.residents.length ? machine : { ...machine, reply: { ...machine.reply, residents: residents! } };
    }
    shownAs.set(machine, { by, shown });
    return shown;
  };
  return {
    ...world,
    who: { ...world.who, machines: world.who.machines.map(who).filter((one): one is MachineWho => one !== null) },
    nodes: { ...world.nodes, hubs: nodes ? [nodes] : [] },
    towns: world.towns.filter((town) => keepsTown(town.town)),
    accounts: { ...world.accounts, hubs: [shown as HubAccounts] },
  };
}
