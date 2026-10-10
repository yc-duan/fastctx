// The account's World as the drawing shows it (`network-map.ts`): each hub
// the account is on, each machine it may work on, and each machine of
// another an agent is lent on, with what the drawing marks on it. The front
// page and the machines page draw the same thing; a machine is keyed
// `m:<name>`, a machine an agent is lent on `lent:<name>`, a hub `hub:<id>`.

import { loginsOf } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { MapHub, MapNode } from "./network-map";
import { osName } from "../world/names";
import { borrowedOf, machineActsFor, machinesOf, ownerOf, owns } from "./reach";
import { residentsOf } from "./store";

export interface MapWords {
  ownedBy(account: string): string;
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
      title: [machine.written, owner ? words.ownedBy(owner) : "", reaching ? words.reachingWhat : ""].filter(Boolean).join(" · "),
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
    });
  }
  return { hubs, nodes, hubLabels };
}
