// The account's World as the drawing shows it (`machine-map.ts`): each hub
// the account is on, each machine it may work on, and each machine of
// another an agent is lent on, with what the drawing marks on it. The front
// page and the machines page draw the same thing; a machine is keyed
// `m:<name>`, a machine an agent is lent on `lent:<name>`, a hub `hub:<id>`.
// Between two machines, the direct link each holds with the other, as the
// two say.

import { loginsOf, type LinkSeen } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { MapHub, MapLink, MapNode } from "./map-nodes";
import { osName } from "../world/names";
import { borrowedOf, machineActsFor, machinesOf, ownerOf, owns } from "./reach";
import { residentsOf } from "./store";

export interface MapWords {
  ownedBy(account: string): string;
  /** On a machine others may connect to directly, for a pointer resting on
   *  its ring. */
  takesDirect: string;
  mineGroup: string;
  lentGroup(owner: string): string;
  lentShort(owner: string): string;
  borrowedSaid(owner: string): string;
  reachingWhat: string;
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
}

export function mapOf(world: WorldNow, words: MapWords): MapNow {
  const residents = residentsOf(world).filter((r) => r.here || r.now?.waiting_for_user);
  const marks = (machine: string) =>
    residents
      .filter((r) => r.about.machine === machine)
      .map((r) => (r.now?.waiting_for_user ? "waiting" : r.now?.activity === "busy" ? "busy" : "idle") as MapNode["marks"][number]);
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
      marks: marks(machine.name),
      title: [machine.written, owner ? words.ownedBy(owner) : "", reaching ? words.reachingWhat : "", direct.length ? words.takesDirect : ""].filter(Boolean).join(" · "),
      directAt: direct,
      through: machine.through ?? null,
      hubMs: (hubLink?.kind === "online" ? hubLink.ms : member?.paths?.hub_ms) ?? undefined,
      group: owner ? { key: `of:${owner}`, label: words.ownedBy(owner) } : { key: "mine", label: words.mineGroup },
    });
  }
  const lent = new Set<string>();
  for (const one of borrowedOf(world)) {
    if (lent.has(one.written)) continue;
    lent.add(one.written);
    nodes.push({
      key: `lent:${one.written}`,
      name: one.written,
      sub: words.lentShort(one.owner ?? ""),
      hubs: [one.login.hub],
      here: false,
      online: one.online,
      hold: "lent",
      reaching: machineActsFor(world, one.machine, one.login),
      marks: marks(one.machine),
      title: words.borrowedSaid(one.owner ?? ""),
      group: { key: `lent:${one.owner ?? ""}`, label: words.lentGroup(one.owner ?? "") },
    });
  }
  return { hubs, nodes, hubLabels, links: linksOf(world, nodes) };
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
