// The account's World as the drawing shows it (`machine-map.ts`): each hub
// the account is on, each machine it may work on, and each machine of
// another whose agent assists it, with what the drawing marks on it. The
// front page and the machines page draw the same thing; a machine is keyed
// `m:<name>`, one whose agent assists `assist:<name>`, a hub `hub:<id>`.
// Between two machines, the direct link each holds with the other, as the
// two say. Where the account is the network's administrator, every other
// person there, whose box the machines page draws even with no machine in
// it.

import { loginsOf, type LinkSeen } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { MapHub, MapLink, MapNode, MapPerson } from "./map-nodes";
import { osName } from "../world/names";
import { assistantsOf, isSuper, machineActsFor, machinesOf, ownerOf, owns } from "./reach";

export interface MapWords {
  ownedBy(account: string): string;
  /** On a machine others may connect to directly, for a pointer resting on
   *  its ring. */
  takesDirect: string;
  mineGroup: string;
  assistGroup(owner: string): string;
  assistShort(owner: string): string;
  assistantSaid(owner: string): string;
  reachingWhat: string;
  /** The names a machine had, each still naming it. */
  formerly(names: string[]): string;
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

export function mapOf(world: WorldNow, words: MapWords): MapNow {
  // How many agents are at work or waiting on each machine its owner sees;
  // a machine away has none now.
  const busy = new Map<string, number>();
  for (const machine of world.who.machines) if (machine.kind === "answered" && machine.reply.busy) busy.set(machine.reply.machine, machine.reply.busy);
  const logins = loginsOf(world.accounts);
  const hubs: MapHub[] = world.accounts.hubs.filter((hub) => hub.logins.length).map((hub) => ({ id: hub.hub, name: hub.name, online: hub.link.kind === "online" }));
  const hubLabels = new Map(world.accounts.hubs.map((hub) => [hub.hub, hub.link.kind === "online" ? words.hubOnline : hub.link.kind === "connecting" ? words.hubConnecting : words.hubOff]));
  const nodes: MapNode[] = [];
  for (const machine of machinesOf(world)) {
    const facts = "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
    const mine = machine.here || logins.some((login) => owns(machine, login));
    const owner = mine ? null : (logins.map((login) => ownerOf(machine, login)).find(Boolean) ?? null);
    const reaching = !mine && logins.some((login) => machineActsFor(world, machine.name, login));
    const member = world.accounts.hubs.flatMap((hub) => hub.members).find((one) => one.name === machine.name && one.kind === "computer");
    const direct = member?.direct ?? [];
    const was = formerNames(world, machine);
    // Written with its hub where another hub has a computer of its name.
    const suffix = machine.written.startsWith(machine.name) ? machine.written.slice(machine.name.length) : "";
    const hubLink = machine.here ? world.accounts.hubs.map((hub) => hub.link).find((link) => link.kind === "online") : undefined;
    nodes.push({
      key: `m:${machine.written}`,
      name: machine.written,
      sub: owner ? words.ownedBy(owner) : facts ? osName(facts.os) : "",
      hubs: machine.on.map((on) => on.hub).filter((hub) => hubs.some((one) => one.id === hub)),
      here: machine.here,
      online: machine.here ? true : machine.online,
      hold: mine ? "mine" : "others",
      reaching,
      busy: busy.get(machine.name) ?? 0,
      title: [machine.written, owner ? words.ownedBy(owner) : "", reaching ? words.reachingWhat : "", direct.length ? words.takesDirect : "", was.length ? words.formerly(was) : ""].filter(Boolean).join(" · "),
      ...(was.length ? { was: was.map((name) => `m:${name}${suffix}`) } : {}),
      directAt: direct,
      through: machine.through ?? null,
      hubMs: (hubLink?.kind === "online" ? hubLink.ms : member?.paths?.hub_ms) ?? undefined,
      group: owner ? { key: `of:${owner}`, label: words.ownedBy(owner) } : { key: "mine", label: words.mineGroup },
    });
  }
  const assisting = new Set<string>();
  for (const one of assistantsOf(world)) {
    if (assisting.has(one.written)) continue;
    assisting.add(one.written);
    nodes.push({
      key: `assist:${one.written}`,
      name: one.written,
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

/** Everyone on a hub but the administrator looking, for the administrator
 *  alone: a member sees only the people whose machines they may use. */
function peopleOf(world: WorldNow, words: MapWords): MapPerson[] {
  const logins = loginsOf(world.accounts);
  return world.accounts.hubs.flatMap((hub) => {
    const admin = logins.find((login) => login.hub === hub.hub && isSuper(world, login));
    if (!admin) return [];
    return hub.accounts
      .filter((account) => !logins.some((login) => login.hub === hub.hub && login.account === account.name))
      .map((account) => ({ hub: hub.hub, account: account.name, label: words.ownedBy(account.name), locked: account.locked }));
  });
}

/** The names a machine had, as the hubs it is on and, for this computer,
 *  its node remember them. */
export function formerNames(world: WorldNow, machine: { name: string; here: boolean; on: { hub: string }[] }): string[] {
  const was = new Set<string>(machine.here ? (world.accounts.was ?? []) : []);
  for (const hub of world.accounts.hubs) {
    if (!machine.on.some((on) => on.hub === hub.hub)) continue;
    for (const member of hub.members) if (member.name === machine.name) for (const name of member.was ?? []) was.add(name);
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
  const pairs = new Map<string, MapLink>();
  const note = (from: string, seen: LinkSeen) => {
    const a = keys.get(from);
    const b = keys.get(seen.peer);
    if (!a || !b || a === b || !nodes.some((node) => node.key === a) || !nodes.some((node) => node.key === b)) return;
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
