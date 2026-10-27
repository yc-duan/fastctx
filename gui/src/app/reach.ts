// What this computer or device reaches (`world-network.md`): every machine
// its logins may work on, this computer itself among them when it is one;
// the agents' logins of other accounts assisting them, on machines that are not
// theirs to work on; and for each login, the account's share of the World:
// its machines, the agents assisting it, its projects. Every page that lists
// machines, agents or groups by account reads them from here, so they all
// agree.

import { loginsOf, type AccountName, type HubAccounts, type HubId, type HubName, type Login, type Rights } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { MachineFacts, ResidentNow } from "../world/world";
import type { MachineName, Result, Timestamp } from "../world/wire";
import { indexOf, pair } from "./world-index";

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

/** One login of an agent of another account assisting one of this
 *  computer's logins, on a machine of theirs that login may not work on. */
export interface Assistant {
  /** The login it assists. */
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
  /** When the assistance ends by itself; null while its owner keeps it on. */
  until: Timestamp | null;
}

/** One login and the part of the World it sees. */
export interface Group {
  login: Login;
  /** The hub as this computer or device last heard it. */
  hub: HubAccounts | null;
  /** The machines it may work on, this computer first, where it is one. */
  machines: Machine[];
  /** The agents of others assisting it, on their machines. */
  assistants: Assistant[];
  /** The projects, by their short names. */
  towns: string[];
}

const all: Rights = { hands: true, agents: true };

/** Whether `machines` were asked of a computer with a node, which is then
 *  one of them, or of a device, which is not. */
export function isComputer(world: WorldNow | null): boolean {
  return !!world?.nodes.facts;
}

/** Whether this is a computer in no network yet: the only machine there
 *  is, of which nothing of the machines' side is shown, no drawing, no
 *  details (`world-gui.md`, 机器的详情). */
export function inNoNetwork(world: WorldNow | null): boolean {
  return isComputer(world) && !world!.nodes.hubs.length;
}

/** Each machine once, made once for each World the store holds; nothing
 *  changes what it returns. */
export function machinesOf(world: WorldNow | null): Machine[] {
  if (!world) return [];
  return once(machinesMade, world, () => machinesFrom(world));
}

const machinesMade = new WeakMap<WorldNow, Machine[]>();

/** The machine written `written`, or the first named `name`, looked up
 *  once for each World. */
export function machineWritten(world: WorldNow | null, written: string): Machine | null {
  return world ? (lookedUp(world).written.get(written) ?? null) : null;
}

export function machineNamed(world: WorldNow | null, name: MachineName): Machine | null {
  return world ? (lookedUp(world).named.get(name) ?? null) : null;
}

const lookUps = new WeakMap<WorldNow, { written: Map<string, Machine>; named: Map<string, Machine> }>();

function lookedUp(world: WorldNow): { written: Map<string, Machine>; named: Map<string, Machine> } {
  return once(lookUps, world, () => {
    const written = new Map<string, Machine>();
    const named = new Map<string, Machine>();
    for (const machine of machinesOf(world)) {
      if (!written.has(machine.written)) written.set(machine.written, machine);
      if (!named.has(machine.name)) named.set(machine.name, machine);
    }
    return { written, named };
  });
}

/** What `make` makes of `world`, made once for each World. */
function once<T>(made: WeakMap<WorldNow, T>, world: WorldNow, make: () => T): T {
  if (made.has(world)) return made.get(world)!;
  const value = make();
  made.set(world, value);
  return value;
}

function machinesFrom(world: WorldNow): Machine[] {
  const { nodes } = world;
  const machines: Machine[] = [];
  const byWritten = new Map<string, Machine>();
  if (nodes.facts) {
    // This computer, as the node says each login may use it; a node before
    // it said so is taken to give each login here the whole of it.
    const here: Machine = {
      name: nodes.machine,
      written: nodes.machine,
      here: true,
      online: true,
      since: null,
      through: null,
      facts: { Ok: nodes.facts },
      on: nodes.hubs.map((hub) => ({
        hub: hub.hub,
        hubName: hub.name,
        owner: hub.home,
        rights: hub.here?.length ? hub.here : hub.logins.map((account): [AccountName, Rights] => [account, all]),
      })),
    };
    machines.push(here);
    byWritten.set(here.written, here);
  }
  // While the line to a hub is down, its view says who was online then,
  // not now, and the node leaves that unsaid. Such a machine stands as it
  // last was, under a hub that says it is reconnecting, unless another hub
  // says how it is now (`world-network.md`, 隔着网络也要顺手).
  const lastSeen = new Map<Machine, boolean>();
  const index = indexOf(world);
  for (const hub of nodes.hubs) {
    const members = index.hubs.get(hub.hub)?.named;
    // A machine none of the logins may work on is not theirs to list,
    // whatever agents on it assist them.
    for (const node of hub.machines.filter((node) => node.rights.some(([, rights]) => rights.hands))) {
      const on: MachineOn = { hub: hub.hub, hubName: hub.name, owner: node.owner, rights: node.rights };
      let machine = byWritten.get(node.written);
      // One computer on two hubs: online when either hub says so.
      if (machine) {
        machine.on.push(on);
        if (node.online === true && machine.online !== true) {
          machine.online = true;
          machine.since = node.since;
          machine.through = node.through ?? null;
        } else if (machine.online === null) machine.online = node.online;
        if (!("Ok" in machine.facts) && "Ok" in node.facts) machine.facts = node.facts;
      } else {
        machine = {
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
      const seen = node.online === null ? members?.get(node.name)?.find((member) => member.kind === "computer") : undefined;
      if (seen && !lastSeen.get(machine)) lastSeen.set(machine, seen.online);
    }
  }
  for (const [machine, online] of lastSeen) if (machine.online === null) machine.online = online;
  return machines;
}

/** What `login` may do on `machine`. */
export function rightsOn(machine: Machine, login: Login): Rights {
  const on = machine.on.find((on) => on.hub === login.hub);
  return on?.rights.find(([account]) => account === login.account)?.[1] ?? { hands: false, agents: false };
}

/** Whether `machine` is registered under `login`'s account. */
export function owns(machine: Machine, login: Login): boolean {
  return machine.on.some((on) => on.hub === login.hub && on.owner === login.account);
}

/** Whether `machine` is in `login`'s hands: its own, or shared whole with
 *  its account, whose holders all manage it alike (`world-network.md`,
 *  共享机器与智能体协助). */
export function holds(machine: Machine, login: Login, world: WorldNow): boolean {
  if (owns(machine, login)) return true;
  if (!machine.on.some((on) => on.hub === login.hub)) return false;
  return !!indexOf(world).hubs.get(login.hub)?.shares.has(pair(machine.name, login.account));
}

/** Whether `machine` runs the network's hub `login` is on, which is shared
 *  with no one. */
export function runsHub(machine: Machine, login: Login, world: WorldNow): boolean {
  return world.accounts.hubs.find((one) => one.hub === login.hub)?.hub_machine === machine.name;
}

/** Whose `machine` is on `login`'s hub; none where it is registered under
 *  nobody there. */
export function ownerOf(machine: Machine, login: Login): AccountName | null {
  return machine.on.find((on) => on.hub === login.hub)?.owner ?? null;
}

/** Every agent's login assisting one of this computer's logins, on a
 *  machine that login may not work on: where it may work, the agent is its
 *  anyway. */
export function assistantsOf(world: WorldNow | null): Assistant[] {
  if (!world) return [];
  return once(assistantsMade, world, () => assistantsFrom(world));
}

const assistantsMade = new WeakMap<WorldNow, Assistant[]>();

function assistantsFrom(world: WorldNow): Assistant[] {
  const index = indexOf(world);
  const byName = new Map<string, Machine[]>();
  for (const machine of machinesOf(world)) {
    const same = byName.get(machine.name);
    if (same) same.push(machine);
    else byName.set(machine.name, [machine]);
  }
  // The hubs where a computer of each name is, or assists.
  const hubsOf = new Map<string, Set<string>>();
  const note = (name: string, hub: string) => {
    const known = hubsOf.get(name);
    if (known) known.add(hub);
    else hubsOf.set(name, new Set([hub]));
  };
  for (const hub of world.accounts.hubs) {
    for (const member of hub.members) note(member.name, hub.hub);
    for (const assist of hub.assists) note(assist.machine, hub.hub);
  }
  const assistants: Assistant[] = [];
  for (const hub of world.accounts.hubs) {
    for (const assist of hub.assists) {
      if (!hub.logins.includes(assist.account)) continue;
      const login: Login = { hub: hub.hub, hubName: hub.name, account: assist.account };
      const workable = (byName.get(assist.machine) ?? []).some((machine) => machine.on.some((on) => on.hub === hub.hub) && rightsOn(machine, login).hands);
      if (workable) continue;
      const member = index.hubs.get(hub.hub)?.named.get(assist.machine)?.[0];
      // Two hubs each with a computer of this name assisting: written
      // with its hub, as `on` writes it.
      const elsewhere = [...(hubsOf.get(assist.machine) ?? [])].some((other) => other !== hub.hub);
      assistants.push({
        login,
        machine: assist.machine,
        written: elsewhere ? `${assist.machine}.${hub.name}` : assist.machine,
        owner: member?.home ?? null,
        online: member ? member.online : null,
        since: member?.since ?? null,
        agent: assist.agent,
        home: assist.home,
        until: assist.until ?? null,
      });
    }
  }
  return assistants;
}

/** Whose agents assist the user on `machine`, where that is all the user
 *  has of it: the user never sees that computer itself, only whose agents
 *  they are (`world-network.md`, 共享机器与智能体协助). Null for any other
 *  machine. */
export function assistingOwner(world: WorldNow | null, machine: string): string | null {
  const one = assistantsOn(world, machine)[0];
  return one ? (one.owner ?? "") : null;
}

/** The assisting logins on one machine, as `name` or as written. */
export function assistantsOn(world: WorldNow | null, machine: string): Assistant[] {
  return assistantsOf(world).filter((one) => one.machine === machine || one.written === machine);
}

export function groupsOf(world: WorldNow | null): Group[] {
  if (!world) return [];
  return once(groupsMade, world, () => groupsFrom(world));
}

const groupsMade = new WeakMap<WorldNow, Group[]>();

function groupsFrom(world: WorldNow): Group[] {
  const machines = machinesOf(world);
  const assistants = assistantsOf(world);
  const logins = loginsOf(world.accounts);
  const groups = logins.map((login): Group => {
    const hub = world.accounts.hubs.find((hub) => hub.hub === login.hub) ?? null;
    return {
      login,
      hub,
      machines: machines.filter((machine) => machine.here || rightsOn(machine, login).hands),
      assistants: assistants.filter((one) => sameLogin(one.login, login)),
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
 *  computer's. One working for none of them is no one's here
 *  (`world-gui.md`, 侧栏、首页与机器页上列的对话). */
export function residentIn(world: WorldNow, group: Group, resident: ResidentNow): boolean {
  const lives = resident.about.machine;
  if (!group.machines.some((machine) => machine.name === lives) && !group.assistants.some((one) => one.machine === lives)) return false;
  const acting = resident.about.acting;
  if (!acting || acting.kind === "here") return true;
  const ours = loginsOf(world.accounts);
  const mine = acting.accounts.filter((account) => ours.some((login) => login.hub === account.hub && login.account === account.account));
  return mine.some((account) => account.hub === group.login.hub && account.account === group.login.account);
}

/** The assisting login a resident is of, where it is one: it lives on a
 *  machine none of the logins it works for may work on, among the agents
 *  assisting them there. */
export function assistantOf(world: WorldNow | null, resident: ResidentNow): Assistant | null {
  const found = assistantsOn(world, resident.about.machine).filter((one) => one.agent === resident.about.host.agent);
  const acting = resident.about.acting;
  return found.find((one) => acting?.kind === "accounts" && acting.accounts.some((account) => sameLogin(one.login, account))) ?? null;
}

/** The logins that may open a conversation with an agent on `machine`, a
 *  machine they may work on: there every agent is theirs to use. On a
 *  managed machine, worked on with no agent of its own, none may
 *  (`world-network.md`, 受管机器). */
export function openersOn(world: WorldNow | null, machine: Machine): Login[] {
  return loginsOf(world?.accounts).filter((login) => {
    const rights = rightsOn(machine, login);
    return rights.hands && rights.agents;
  });
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

/** Where files an assisting agent is given may go: the account's own
 *  machines it may work on, never one shared to it, whose owner's disk the
 *  original is not to land on (`world-run.md`, 受协助的人附的文件). */
export function filePlaces(world: WorldNow | null, login: Login): Machine[] {
  return machinesOf(world).filter((machine) => owns(machine, login) && (machine.here || rightsOn(machine, login).hands));
}

/** Where files other than images go for an assisting agent, which works on no
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

/** Whether the login given as `home` (its directory, or null for the
 *  default one) is the one a machine says as `dir` (null for its default
 *  one): written alike, but for case and slashes on a machine that writes
 *  paths either way. */
export function sameHome(home: string | null, dir: string | null): boolean {
  if (home === null || dir === null) return home === dir;
  const plain = (path: string) => path.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
  return plain(home) === plain(dir);
}

/** "home ada": a login as the user reads it. */
export function loginLabel(login: Login): string {
  return `${login.hubName} ${login.account}`;
}

export function sameLogin(a: Login, b: { hub: HubId; account: AccountName }): boolean {
  return a.hub === b.hub && a.account === b.account;
}

/** Whether a machine already acts for `login`'s account: the account is
 *  logged in on it, or used an agent there; otherwise the first use of an
 *  agent there lets it reach the account's machines (`world-network.md`). */
export function machineActsFor(world: WorldNow | null, machine: MachineName, login: Login): boolean {
  const hub = world ? indexOf(world).hubs.get(login.hub) : undefined;
  if (!hub) return false;
  if (hub.named.get(machine)?.some((member) => member.logins.includes(login.account))) return true;
  return hub.acting.has(pair(machine, login.account));
}

/** Whether `login`'s account is its hub's super administrator. */
export function isSuper(world: WorldNow | null, login: Login): boolean {
  const me = world ? indexOf(world).hubs.get(login.hub)?.accounts.get(login.account) : undefined;
  return me?.level === "super" && !me.locked;
}

/** Whether `login` is a visitor on its hub (`world-network.md`, 一个中枢，
 *  几个账号): its machines are its alone, and it shares none of them and
 *  lets none of its agents assist anyone. */
export function isVisitor(world: WorldNow | null, login: Login): boolean {
  return (world ? indexOf(world).hubs.get(login.hub)?.accounts.get(login.account)?.level : undefined) === "visitor";
}

/** The hub's super administrator, as `login` sees it. */
export function superOf(world: WorldNow | null, login: Login): AccountName | null {
  const hub = world?.accounts.hubs.find((hub) => hub.hub === login.hub);
  return hub?.accounts.find((account) => account.level === "super")?.name ?? null;
}
