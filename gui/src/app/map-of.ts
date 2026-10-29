// The account's World as the drawing shows it (`machine-map.ts`): each hub
// the account is on, each machine it may work on, the phones signed in to
// it, and each machine of another whose agent assists it, with what the
// drawing marks on it. The front page and the machines page draw the same
// thing; a machine is keyed `m:<name>`, a phone `device:<hub>/<name>`, one
// whose agent assists `assist:<name>`, a hub `hub:<id>`.
// Between two machines, the direct link each holds with the other, as the
// two say. Where the account is the network's administrator, every other
// person there, whose box the machines page draws even with no machine in
// it.

import { loginsOf, type HubAccounts, type LinkSeen, type Login, type MemberSeen } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import { deviceKey, type MapHub, type MapLink, type MapNode, type MapPerson } from "./map-nodes";
import { osName } from "../world/names";
import { assistantsOf, isComputer, isSuper, machineActsFor, machinesOf, ownerOf, owns } from "./reach";
import { indexOf } from "./world-index";

export interface MapWords {
  ownedBy(account: string): string;
  /** On a machine others may connect to directly, for a pointer resting on
   *  its ring. */
  takesDirect: string;
  mineGroup: string;
  assistGroup(owner: string): string;
  assistShort(owner: string): string;
  /** An assisting machine's name on the map: whose agents, never the machine. */
  assistAgents(owner: string): string;
  assistantSaid(owner: string): string;
  reachingWhat: string;
  /** The names a machine had, each still naming it. */
  formerly(names: string[]): string;
  visitorMachine: string;
  visitorWhat: string;
  /** Under a managed machine's name. */
  managed: string;
  /** Under a phone's name, and what it is behind a pointer's rest. */
  phone: string;
  phoneWhat: string;
  hubOnline: string;
  hubConnecting: string;
  hubOff: string;
}

export interface MapNow {
  hubs: MapHub[];
  nodes: MapNode[];
  /** What stands under each hub's name. */
  hubLabels: Map<string, string>;
  links: MapLink[];
  /** For the network's administrator, everyone else on the hub. */
  people: MapPerson[];
}

/** The map of `world`, made once for each World the store holds: the page
 *  draws itself every couple of seconds, and a hundred machines are worth
 *  laying out only when something of them changed. Nothing is changed in
 *  what it returns; whoever adds to it adds to a copy. */
export function mapOf(world: WorldNow, words: MapWords): MapNow {
  const kept = made.get(world);
  if (kept?.words === words) return kept.map;
  const map = mapMade(world, words);
  made.set(world, { words, map });
  return map;
}

const made = new WeakMap<WorldNow, { words: MapWords; map: MapNow }>();

function mapMade(world: WorldNow, words: MapWords): MapNow {
  // How many agents are at work or waiting on each machine its owner sees;
  // a machine away has none now.
  const busy = new Map<string, number>();
  for (const machine of world.who.machines) if (machine.kind === "answered" && machine.reply.busy) busy.set(machine.reply.machine, machine.reply.busy);
  const logins = loginsOf(world.accounts);
  const hubs: MapHub[] = world.accounts.hubs.filter((hub) => hub.logins.length).map((hub) => ({ id: hub.hub, name: hub.name, online: hub.link.kind === "online" }));
  // Nothing is said beside a hub that is reached: its knot's drop says it.
  const hubLabels = new Map(world.accounts.hubs.map((hub) => [hub.hub, hub.link.kind === "online" ? "" : hub.link.kind === "connecting" ? words.hubConnecting : words.hubOff]));
  const nodes: MapNode[] = [];
  const index = indexOf(world);
  for (const machine of machinesOf(world)) {
    const facts = "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
    const mine = machine.here || logins.some((login) => owns(machine, login));
    const owner = mine ? null : (logins.map((login) => ownerOf(machine, login)).find(Boolean) ?? null);
    const reaching = !mine && logins.some((login) => machineActsFor(world, machine.name, login));
    const member = index.computer.get(machine.name);
    const direct = member?.direct ?? [];
    const was = formerNames(world, machine);
    // Written with its hub where another hub has a computer of its name.
    const suffix = machine.written.startsWith(machine.name) ? machine.written.slice(machine.name.length) : "";
    const hubLink = machine.here ? world.accounts.hubs.map((hub) => hub.link).find((link) => link.kind === "online") : undefined;
    // How its line to the hub goes is known only while a hub it is on is
    // connected: a round trip measured before stays unsaid.
    const heard = world.accounts.hubs.some((hub) => hub.link.kind === "online" && machine.on.some((on) => on.hub === hub.hub));
    // A managed machine says so under its name.
    const said = owner ? words.ownedBy(owner) : facts ? osName(facts.os) : "";
    nodes.push({
      key: `m:${machine.written}`,
      name: machine.written,
      sub: [said, member?.managed ? words.managed : ""].filter(Boolean).join(" "),
      hubs: machine.on.map((on) => on.hub).filter((hub) => hubs.some((one) => one.id === hub)),
      here: machine.here,
      online: machine.here ? true : machine.online,
      hold: mine ? "mine" : "others",
      reaching,
      busy: busy.get(machine.name) ?? 0,
      title: [machine.written, owner ? words.ownedBy(owner) : "", reaching ? words.reachingWhat : "", direct.length ? words.takesDirect : "", was.length ? words.formerly(was) : ""].filter(Boolean).join("\n"),
      ...(was.length ? { was: was.map((name) => `m:${name}${suffix}`) } : {}),
      directAt: direct,
      through: machine.through ?? null,
      hubMs: (machine.here ? (hubLink?.kind === "online" ? hubLink.ms : undefined) : heard ? member?.paths?.hub_ms : undefined) ?? undefined,
      group: boxOf(owner ? { key: `of:${owner}`, label: words.ownedBy(owner) } : { key: "mine", label: words.mineGroup }, member?.group),
    });
  }
  // The phones signed in to an account stand in its box after its
  // machines: where a phone added from this page is seen to arrive.
  for (const { hub, member, owner } of phonesOf(world)) {
    if (!hubs.some((one) => one.id === hub.hub)) continue;
    nodes.push({
      key: deviceKey(hub.hub, member.name),
      name: member.name,
      sub: words.phone,
      hubs: [hub.hub],
      here: member.name === world.accounts.machine && !isComputer(world),
      online: member.online,
      since: member.since ?? null,
      hold: owner ? "others" : "mine",
      reaching: false,
      busy: 0,
      title: [member.name, words.phoneWhat, owner ? words.ownedBy(owner) : ""].filter(Boolean).join("\n"),
      group: owner ? { key: `of:${owner}`, label: words.ownedBy(owner) } : { key: "mine", label: words.mineGroup },
      device: true,
    });
  }
  // A visitor's machines, for the network's administrator alone: their
  // names and whether they are online, nothing to do on them.
  for (const hub of world.accounts.hubs) {
    if (!logins.some((login) => login.hub === hub.hub && isSuper(world, login))) continue;
    const visitors = new Set(hub.accounts.filter((account) => account.level === "visitor").map((account) => account.name));
    for (const member of hub.members) {
      if (member.kind !== "computer" || !member.home || !visitors.has(member.home)) continue;
      nodes.push({
        key: `visitor:${member.name}`,
        name: member.name,
        sub: words.visitorMachine,
        hubs: [hub.hub],
        here: false,
        online: member.online,
        hold: "visitor",
        reaching: false,
        busy: 0,
        title: words.visitorWhat,
        group: { key: `of:${member.home}`, label: words.ownedBy(member.home) },
      });
    }
  }
  const assisting = new Set<string>();
  for (const one of assistantsOf(world)) {
    if (assisting.has(one.written)) continue;
    assisting.add(one.written);
    nodes.push({
      key: `assist:${one.written}`,
      name: words.assistAgents(one.owner ?? ""),
      sub: words.assistShort(one.owner ?? ""),
      hubs: [one.login.hub],
      here: false,
      online: one.online,
      hold: "assist",
      reaching: machineActsFor(world, one.machine, one.login),
      busy: 0,
      title: words.assistantSaid(one.owner ?? ""),
      group: { key: `assist:${one.owner ?? ""}`, label: words.assistGroup(one.owner ?? "") },
    });
  }
  return { hubs, nodes, hubLabels, links: linksOf(world, nodes), people: peopleOf(world, words) };
}

type Phone = { hub: HubAccounts; member: MemberSeen; login: Login; owner: string | null };

/** The phones this device sees, hub by hub: those signed in to an account
 *  it is logged in to, with that login and no owner; and, where it is the
 *  network's administrator, those signed in to a member's account, with
 *  the administrator's login and that member as their owner, as the
 *  member's machines are (`world-gui.md`, 机器页). A visitor's phones stay
 *  out of the administrator's reach, as their machines' insides do. */
export function phonesOf(world: WorldNow): Phone[] {
  const logins = loginsOf(world.accounts);
  return world.accounts.hubs.flatMap((hub) => {
    const admin = logins.find((one) => one.hub === hub.hub && isSuper(world, one));
    const visitors = new Set(hub.accounts.filter((account) => account.level === "visitor").map((account) => account.name));
    return hub.members.flatMap((member): Phone[] => {
      if (member.kind !== "phone") return [];
      const login = logins.find((one) => one.hub === hub.hub && member.logins.includes(one.account));
      if (login) return [{ hub, member, login, owner: null }];
      const owner = admin ? member.logins.find((account) => !visitors.has(account)) : undefined;
      return admin && owner ? [{ hub, member, login: admin, owner }] : [];
    });
  });
}

/** The box a machine stands in: its owner's, or, put in one of the owner's
 *  groups, that group's box inside the owner's (`world-network.md`, 机器的分组). */
function boxOf(box: { key: string; label: string }, group: string | undefined): NonNullable<MapNode["group"]> {
  return group ? { key: `${box.key}/${group}`, label: group, within: box } : box;
}

/** Everyone on a hub but the administrator looking, for the administrator
 *  alone: a member sees only the people whose machines they may use. */
function peopleOf(world: WorldNow, words: MapWords): MapPerson[] {
  const logins = loginsOf(world.accounts);
  return world.accounts.hubs.flatMap((hub) => {
    const admin = logins.find((login) => login.hub === hub.hub && isSuper(world, login));
    if (!admin) return [];
    return hub.accounts
      .filter((account) => !logins.some((login) => login.hub === hub.hub && login.account === account.name))
      .map((account) => ({
        hub: hub.hub,
        account: account.name,
        label: words.ownedBy(account.name),
        locked: account.locked,
        visitor: account.level === "visitor",
        promotion: !!account.promotion,
      }));
  });
}

/** The names a machine had, as the hubs it is on and, for this computer,
 *  its node remember them. */
export function formerNames(world: WorldNow, machine: { name: string; here: boolean; on: { hub: string }[] }): string[] {
  const was = new Set<string>(machine.here ? (world.accounts.was ?? []) : []);
  const index = indexOf(world);
  for (const hub of world.accounts.hubs) {
    if (!machine.on.some((on) => on.hub === hub.hub)) continue;
    for (const member of index.hubs.get(hub.hub)?.named.get(machine.name) ?? []) for (const name of member.was ?? []) was.add(name);
  }
  was.delete(machine.name);
  return [...was];
}

/** The direct links between machines on the drawing, each pair once: up
 *  when either end holds it up, with the faster round trip of the two
 *  ends; down with why, when neither does and one tried. */
function linksOf(world: WorldNow, nodes: MapNode[]): MapLink[] {
  const keys = new Map<string, string>();
  for (const machine of machinesOf(world)) keys.set(machine.name, `m:${machine.written}`);
  const drawn = new Set(nodes.map((node) => node.key));
  const pairs = new Map<string, MapLink>();
  const note = (from: string, seen: LinkSeen) => {
    const a = keys.get(from);
    const b = keys.get(seen.peer);
    if (!a || !b || a === b || !drawn.has(a) || !drawn.has(b)) return;
    const [first, second] = a < b ? [a, b] : [b, a];
    const key = `${first}|${second}`;
    const known = pairs.get(key);
    if (!known) {
      pairs.set(key, { a: first, b: second, up: seen.up, ms: seen.up ? seen.ms : undefined, why: seen.up ? undefined : seen.why });
      return;
    }
    if (seen.up) {
      known.up = true;
      known.why = undefined;
      if (seen.ms !== undefined && (known.ms === undefined || seen.ms < known.ms)) known.ms = seen.ms;
    } else if (!known.up && !known.why) known.why = seen.why;
  };
  for (const hub of world.accounts.hubs) {
    for (const member of hub.members) for (const seen of member.paths?.links ?? []) note(member.name, seen);
    for (const seen of hub.links ?? []) note(world.nodes.machine, seen);
  }
  return [...pairs.values()];
}
