// What this computer or device reaches (`world-network.md`): every machine
// its logins may work on, this computer itself among them when it is one;
// the agents' logins other accounts lent them, on machines that are not
// theirs to work on; and for each login, the account's share of the World:
// its machines, the agents lent to it, its projects. Every page that lists
// machines, agents or groups by account reads them from here, so they all
// agree.

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

/** One login of an agent another account lent to one of this computer's
 *  logins, on a machine of theirs that login may not work on. */
export interface Borrowed {
  /** The login it is lent to. */
  login: Login;
  machine: MachineName;
  written: string;
  /** Whose machine it is. */
  owner: AccountName | null;
  online: boolean | null;
  since: Timestamp | null;
  agent: string;
  /** The login's directory as that machine writes it; null for the
   *  agent's default login there. */
  home: string | null;
}

/** One login and the part of the World it sees. */
export interface Group {
  login: Login;
  /** The hub as this computer or device last heard it. */
  hub: HubAccounts | null;
  /** The machines it may work on, this computer first, where it is one. */
  machines: Machine[];
  /** The agents lent to it on machines of others. */
  borrowed: Borrowed[];
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
    // A machine none of the logins may work on is not theirs to list,
    // whatever agents on it are lent to them.
    for (const node of hub.machines.filter((node) => node.rights.some(([, rights]) => rights.hands))) {
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

/** Whose `machine` is on `login`'s hub; none where it is registered under
 *  nobody there. */
export function ownerOf(machine: Machine, login: Login): AccountName | null {
  return machine.on.find((on) => on.hub === login.hub)?.owner ?? null;
}

/** Every agent's login lent to one of this computer's logins, on a machine
 *  that login may not work on: where it may work, the agent is its anyway. */
export function borrowedOf(world: WorldNow | null): Borrowed[] {
  if (!world) return [];
  const machines = machinesOf(world);
  const named = new Map<string, number>();
  for (const hub of world.accounts.hubs) for (const lend of hub.lends) named.set(lend.machine, (named.get(lend.machine) ?? 0) + 1);
  const borrowed: Borrowed[] = [];
  for (const hub of world.accounts.hubs) {
    for (const lend of hub.lends) {
      if (!hub.logins.includes(lend.account)) continue;
      const login: Login = { hub: hub.hub, hubName: hub.name, account: lend.account };
      const workable = machines.some((machine) => machine.name === lend.machine && machine.on.some((on) => on.hub === hub.hub) && rightsOn(machine, login).hands);
      if (workable) continue;
      const member = hub.members.find((member) => member.name === lend.machine);
      // Two hubs each with a computer of this name lending it: written
      // with its hub, as `on` writes it.
      const elsewhere = world.accounts.hubs.some((other) => other.hub !== hub.hub && (other.members.some((m) => m.name === lend.machine) || other.lends.some((l) => l.machine === lend.machine)));
      borrowed.push({
        login,
        machine: lend.machine,
        written: elsewhere ? `${lend.machine}.${hub.name}` : lend.machine,
        owner: member?.home ?? null,
        online: member ? member.online : null,
        since: member?.since ?? null,
        agent: lend.agent,
        home: lend.home,
      });
    }
  }
  return borrowed;
}

/** The borrowed logins on one machine, as `name` or as written. */
export function borrowedOn(world: WorldNow | null, machine: string): Borrowed[] {
  return borrowedOf(world).filter((one) => one.machine === machine || one.written === machine);
}

export function groupsOf(world: WorldNow | null): Group[] {
  if (!world) return [];
  const machines = machinesOf(world);
  const borrowed = borrowedOf(world);
  const logins = loginsOf(world.accounts);
  const groups = logins.map((login): Group => {
    const hub = world.accounts.hubs.find((hub) => hub.hub === login.hub) ?? null;
    return {
      login,
      hub,
      machines: machines.filter((machine) => machine.here || rightsOn(machine, login).hands),
      borrowed: borrowed.filter((one) => sameLogin(one.login, login)),
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
  const lives = resident.about.machine;
  if (!group.machines.some((machine) => machine.name === lives) && !group.borrowed.some((one) => one.machine === lives)) return false;
  const acting = resident.about.acting;
  if (!acting || acting.kind === "here") return true;
  const ours = loginsOf(world.accounts);
  const mine = acting.accounts.filter((account) => ours.some((login) => login.hub === account.hub && login.account === account.account));
  return !mine.length || mine.some((account) => account.hub === group.login.hub && account.account === group.login.account);
}

/** The login a resident on a machine lent to this computer's logins
 *  borrows, where it is one: it lives on a machine none of the logins it
 *  works for may work on, among the agents lent to them there. */
export function borrowedBy(world: WorldNow | null, resident: ResidentNow): Borrowed | null {
  const lent = borrowedOn(world, resident.about.machine).filter((one) => one.agent === resident.about.host.agent);
  const acting = resident.about.acting;
  return lent.find((one) => acting?.kind === "accounts" && acting.accounts.some((account) => sameLogin(one.login, account))) ?? null;
}

/** The logins that may open a conversation with an agent on `machine`, a
 *  machine they may work on: there every agent is theirs to use. */
export function openersOn(world: WorldNow | null, machine: Machine): Login[] {
  return loginsOf(world?.accounts).filter((login) => rightsOn(machine, login).hands);
}

/** Where a conversation living on `lives` and working for `login` may
 *  run its commands: machines the account may work on, reached from where
 *  it lives. This computer is reached from elsewhere only where it is the
 *  account's own. */
export function workPlaces(world: WorldNow | null, lives: Machine, login: Login | null): Machine[] {
  return machinesOf(world).filter((machine) => {
    if (machine.written === lives.written) return !login || rightsOn(machine, login).hands;
    if (!login) return false;
    if (machine.here) return lives.here || owns(machine, login);
    return rightsOn(machine, login).hands;
  });
}

/** Where files a lent agent is given may go: the machines the account it
 *  works for may work on, this computer only where it is the account's own
 *  machine, for only a machine can take them. */
export function filePlaces(world: WorldNow | null, login: Login): Machine[] {
  return machinesOf(world).filter((machine) => (machine.here ? owns(machine, login) : rightsOn(machine, login).hands));
}

/** Where files other than images go for a lent agent, which works on no
 *  machine of its own: the machine in front of the user, where it can take
 *  them; else the first of the account's own that is online; else the first
 *  online it may work on. The model is told where they are. */
export function fileHome(world: WorldNow | null, login: Login): string | null {
  const places = filePlaces(world, login).filter((machine) => machine.here || machine.online !== false);
  return (places.find((machine) => machine.here) ?? places.find((machine) => owns(machine, login)) ?? places[0])?.written ?? null;
}

/** One agent's login on a machine, as the new conversation names it. */
export function loginKey(machine: string, agent: string, home: string | null): string {
  return `${machine}|${agent}|${home ?? ""}`;
}

/** Whether a login lent with `lent` (its directory, or null for the
 *  default one) is the one a machine says as `dir` (null for its default
 *  one): written alike, but for case and slashes on a machine that writes
 *  paths either way. */
export function sameHome(lent: string | null, dir: string | null): boolean {
  if (lent === null || dir === null) return lent === dir;
  const plain = (path: string) => path.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
  return plain(lent) === plain(dir);
}

/** "hangzhou · ada": a login as the user reads it. */
export function loginLabel(login: Login): string {
  return `${login.hubName} · ${login.account}`;
}

export function sameLogin(a: Login, b: { hub: HubId; account: AccountName }): boolean {
  return a.hub === b.hub && a.account === b.account;
}

/** Whether a machine already acts for `login`'s account: the account is
 *  logged in on it, or used an agent there; otherwise the first use of an
 *  agent there lets it reach the account's machines (`world-network.md`). */
export function machineActsFor(world: WorldNow | null, machine: MachineName, login: Login): boolean {
  const hub = world?.accounts.hubs.find((hub) => hub.hub === login.hub);
  if (!hub) return false;
  if (hub.members.some((member) => member.name === machine && member.logins.includes(login.account))) return true;
  return hub.acting.some((acting) => acting.machine === machine && acting.account === login.account);
}

/** Whether `login`'s account is its hub's super administrator. */
export function isSuper(world: WorldNow | null, login: Login): boolean {
  const hub = world?.accounts.hubs.find((hub) => hub.hub === login.hub);
  const me = hub?.accounts.find((account) => account.name === login.account);
  return me?.level === "super" && !me.locked;
}

/** The hub's super administrator, as `login` sees it. */
export function superOf(world: WorldNow | null, login: Login): AccountName | null {
  const hub = world?.accounts.hubs.find((hub) => hub.hub === login.hub);
  return hub?.accounts.find((account) => account.level === "super")?.name ?? null;
}
