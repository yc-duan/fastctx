// What this computer or device reaches (`world-network.md`): every machine
// its logins may use, this computer itself among them when it is one, and
// for each login, the account's share of the World: its machines, its
// projects and the agents on them. Every page that lists machines or groups
// by account reads them from here, so they all agree.

import { loginsOf, type AccountName, type HubAccounts, type HubId, type HubName, type Login, type Rights } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { MachineFacts, ResidentNow } from "../world/world";
import type { MachineName, Result, Timestamp } from "../world/wire";

/** One machine as this computer or device reaches it. */
export interface Machine {
  name: MachineName;
  /** As it is written: `name.hub` where two hubs each have a different
   *  computer of that name. */
  written: string;
  /** This computer. */
  here: boolean;
  online: boolean | null;
  since: Timestamp | null;
  /** The relay it connects through. */
  through: string | null;
  facts: Result<MachineFacts, string>;
  /** Hub by hub: whose it is there, and what each login may do on it. */
  on: MachineOn[];
}

export interface MachineOn {
  hub: HubId;
  hubName: HubName;
  /** The account it is registered under; none where it only logs in. */
  owner: AccountName | null;
  rights: [AccountName, Rights][];
}

/** One login and the part of the World it sees. */
export interface Group {
  login: Login;
  /** The hub as this computer or device last heard it. */
  hub: HubAccounts | null;
  /** This computer first, where it is one. */
  machines: Machine[];
  /** The projects, by their short names. */
  towns: string[];
}

const all: Rights = { hands: true, agents: true };

/** Whether `machines` were asked of a computer with a node, which is then
 *  one of them, or of a device, which is not. */
export function isComputer(world: WorldNow | null): boolean {
  return !!world?.nodes.facts;
}

export function machinesOf(world: WorldNow | null): Machine[] {
  if (!world) return [];
  const { nodes } = world;
  const machines: Machine[] = [];
  const byWritten = new Map<string, Machine>();
  if (nodes.facts) {
    // Whoever the user is logged in as here, this computer is theirs to use.
    const here: Machine = {
      name: nodes.machine,
      written: nodes.machine,
      here: true,
      online: true,
      since: null,
      through: null,
      facts: { Ok: nodes.facts },
      on: nodes.hubs.map((hub) => ({ hub: hub.hub, hubName: hub.name, owner: hub.home, rights: hub.logins.map((account) => [account, all]) })),
    };
    machines.push(here);
    byWritten.set(here.written, here);
  }
  for (const hub of nodes.hubs) {
    for (const node of hub.machines) {
      const on: MachineOn = { hub: hub.hub, hubName: hub.name, owner: node.owner, rights: node.rights };
      const known = byWritten.get(node.written);
      // One computer on two hubs: online when either hub says so.
      if (known) {
        known.on.push(on);
        if (node.online === true && known.online !== true) {
          known.online = true;
          known.since = node.since;
          known.through = node.through ?? null;
        } else if (known.online === null) known.online = node.online;
        if (!("Ok" in known.facts) && "Ok" in node.facts) known.facts = node.facts;
        continue;
      }
      const machine: Machine = {
        name: node.name,
        written: node.written,
        here: false,
        online: node.online,
        since: node.since,
        through: node.through ?? null,
        facts: node.facts,
        on: [on],
      };
      machines.push(machine);
      byWritten.set(machine.written, machine);
    }
  }
  return machines;
}

/** What `login` may do on `machine`. */
export function rightsOn(machine: Machine, login: Login): Rights {
  const on = machine.on.find((on) => on.hub === login.hub);
  const found = on?.rights.find(([account]) => account === login.account)?.[1];
  return found ?? (machine.here ? all : { hands: false, agents: false });
}

/** Whether `machine` is registered under `login`'s account. */
export function owns(machine: Machine, login: Login): boolean {
  return machine.on.some((on) => on.hub === login.hub && on.owner === login.account);
}

export function groupsOf(world: WorldNow | null): Group[] {
  if (!world) return [];
  const machines = machinesOf(world);
  const logins = loginsOf(world.accounts);
  const groups = logins.map((login): Group => {
    const hub = world.accounts.hubs.find((hub) => hub.hub === login.hub) ?? null;
    return {
      login,
      hub,
      machines: machines.filter((machine) => {
        if (machine.here) return true;
        const rights = rightsOn(machine, login);
        return rights.hands || rights.agents;
      }),
      towns: (hub?.projects ?? [])
        .filter((project) => project.owner === login.account || project.granted.includes(login.account))
        .map((project) => project.name),
    };
  });
  // A project no hub knows yet is this computer's own, on its way to the
  // account it is registered under; it shows with the first.
  const claimed = new Set(groups.flatMap((group) => group.towns));
  const unclaimed = world.towns.map((town) => town.town).filter((town) => !claimed.has(town));
  if (groups[0]) groups[0].towns.push(...unclaimed);
  return groups;
}

/** The logins that see a resident: those it works for, among this
 *  computer's; otherwise every one that may use the machine it is on. */
export function residentIn(world: WorldNow, group: Group, resident: ResidentNow): boolean {
  if (!group.machines.some((machine) => machine.name === resident.about.machine)) return false;
  const acting = resident.about.acting;
  if (!acting || acting.kind === "here") return true;
  const ours = loginsOf(world.accounts);
  const mine = acting.accounts.filter((account) => ours.some((login) => login.hub === account.hub && login.account === account.account));
  return !mine.length || mine.some((account) => account.hub === group.login.hub && account.account === group.login.account);
}

/** The logins that may open a conversation with an agent on `machine`. */
export function openersOn(world: WorldNow | null, machine: Machine): Login[] {
  return loginsOf(world?.accounts).filter((login) => rightsOn(machine, login).agents);
}

/** Where a conversation living on `lives` and working for `login` may
 *  run its commands: machines the account may work on, reached from where
 *  it lives. This computer is reached from elsewhere only where it is the
 *  account's own. */
export function workPlaces(world: WorldNow | null, lives: Machine, login: Login | null): Machine[] {
  return machinesOf(world).filter((machine) => {
    if (machine.written === lives.written) return true;
    if (!login) return false;
    if (machine.here) return lives.here || owns(machine, login);
    return rightsOn(machine, login).hands;
  });
}

/** Where the colleagues of a conversation living on `lives` and working
 *  for `login` may live: machines whose agents the account may use,
 *  reached from where it lives. */
export function colleagueHomes(world: WorldNow | null, lives: Machine, login: Login | null): Machine[] {
  return machinesOf(world).filter((machine) => {
    if (machine.written === lives.written) return true;
    if (!login) return false;
    if (machine.here) return lives.here || owns(machine, login);
    return rightsOn(machine, login).agents;
  });
}

/** "hangzhou · ada": a login as the user reads it. */
export function loginLabel(login: Login): string {
  return `${login.hubName} · ${login.account}`;
}

export function sameLogin(a: Login, b: { hub: HubId; account: AccountName }): boolean {
  return a.hub === b.hub && a.account === b.account;
}
