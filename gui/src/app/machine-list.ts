// The machines as a list (`world-gui.md`, 账号、机器、共享与协助): the same
// World the drawing shows, set out as rows. The hub stands first; then the
// machines grouped as the drawing groups them, the account's own first, this
// computer first among them, then each other person's, then those whose
// agents assist; under each group's name a hand-drawn line. The machines an
// owner put in groups (`world-network.md`, 机器的分组) follow the rest of
// theirs, each group under its name. A row says
// whether the machine is there, its name, what it is, how it reaches the
// hub, whom it is shared with and how many agents are at work on it. A
// machine's row goes to its own page; a hub's or an assisting agent's opens
// the details beside the list, as on the drawing. A visitor's machine, which
// the network's administrator only sees, is a faint row that goes nowhere.
// A machine the account works on has a small box at the start of its row,
// and each heading one for all its machines, half filled when some are
// chosen: the boxes show where pointed at, and on every row once any is
// chosen (`machine-picking.ts`).

import { glyph, type GlyphName } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { WorldNow } from "../world/bridge";
import { loginsOf } from "../world/accounts";
import { agentName, osName } from "../world/names";
import type { MachinesWords } from "./account-words";
import { compareBoxes } from "./machine-groups";
import { mapOf } from "./map-of";
import type { MapNode } from "./map-nodes";
import { pickable, pickableIn } from "./machine-picking";
import { link } from "./places";
import { assistantsOf, machinesOf, owns, type Machine } from "./reach";

export interface ListNow {
  world: WorldNow;
  now: Date;
  /** The spot whose details are open beside the list, by its key. */
  selected: string | null;
  /** The hubs a machine may be added on: those this device is logged in to. */
  adds: string[];
  /** When, as the page says it: a clock time today, a day before. */
  when(at: Date, now: Date): string;
  /** The machines chosen together, by their keys. */
  picked: ReadonlySet<string>;
  /** The names of a row's box and a heading's. */
  pick: { one(name: string): string; all(label: string): string };
}

/** Up to this many dots for the agents at work on a machine; more are a
 *  number. */
const DOTS = 4;

export function machineList(now: ListNow, words: MachinesWords): string {
  const { world } = now;
  const { hubs, nodes, hubLabels } = mapOf(world, words);
  const machines = new Map(machinesOf(world).map((machine) => [`m:${machine.written}`, machine]));
  const hubRows = hubs.map((hub) => {
    const seen = world.accounts.hubs.find((one) => one.hub === hub.id);
    const ms = seen?.link.kind === "online" ? seen.link.ms : null;
    const count = nodes.filter((node) => node.hubs.includes(hub.id) && node.hold !== "assist" && !node.device).length;
    const state = [hubLabels.get(hub.id) ?? "", ms ? words.listMs(ms) : ""].filter(Boolean).join(" · ");
    const key = `hub:${hub.id}`;
    return `<button class="ml-row ml-hub${hub.online ? " online" : " offline"}${now.selected === key ? " chosen" : ""}" type="button" data-ml-open="${escape(key)}">
      <span class="ml-pick none" aria-hidden="true"></span>
      <span class="ml-mark" aria-hidden="true">${knot()}</span>
      <span class="ml-name"><span class="ml-name-text">${escape(hub.name)}</span><span class="tag">${escape(words.listHub)}</span></span>
      <span class="ml-system">${escape(words.hubMachines(count))}</span>
      <span class="ml-link">${escape(state)}</span>
      <span class="ml-given"></span>
      <span class="ml-busy"></span>
      <span class="ml-go" aria-hidden="true">${glyph("chevron", 16)}</span>
    </button>`;
  });
  // Groups in the drawing's order: the account's own, each other person's
  // as they first come, then those whose agents assist; inside each, its
  // owner's groups by name.
  const groups: { key: string; label: string; nodes: MapNode[]; inside: { key: string; label: string; nodes: MapNode[] }[] }[] = [];
  for (const node of nodes) {
    const group = node.group ?? { key: node.hold, label: node.hold };
    const owner = group.within ?? group;
    let found = groups.find((one) => one.key === owner.key);
    if (!found) groups.push((found = { key: owner.key, label: owner.label, nodes: [], inside: [] }));
    if (!group.within) found.nodes.push(node);
    else {
      let band = found.inside.find((one) => one.key === group.key);
      if (!band) found.inside.push((band = { key: group.key, label: group.label, nodes: [] }));
      band.nodes.push(node);
    }
  }
  const rank = (owner: string) => (owner === "mine" ? 0 : owner.startsWith("assist:") ? 2 : 1);
  groups.sort((a, b) => compareBoxes(a.key, b.key, rank));
  for (const group of groups) group.inside.sort((a, b) => compareBoxes(a.key, b.key, rank));
  if (now.adds.length && !groups.some((group) => group.key === "mine")) groups.unshift({ key: "mine", label: words.mineGroup, nodes: [], inside: [] });
  // This computer first, then by name: the rows stay where they are as
  // machines come and go.
  const sorted = (nodes: MapNode[]) => [...nodes].sort((a, b) => Number(b.here) - Number(a.here) || a.name.localeCompare(b.name, undefined, { numeric: true }));
  const online = (nodes: MapNode[]) => nodes.filter((node) => node.online !== false).length;
  const rows = (nodes: MapNode[]) => sorted(nodes).map((node) => row(node, machines.get(node.key) ?? null, now, words)).join("");
  const body = groups
    .map((group) => {
      const all = [...group.nodes, ...group.inside.flatMap((band) => band.nodes)];
      const visitor = all.some((node) => node.hold === "visitor") ? `<span class="tag">${escape(words.visitor)}</span>` : "";
      const add =
        group.key === "mine" && now.adds.length
          ? `<button class="ml-row ml-add" type="button" data-ml-add="${escape(now.adds[0]!)}"><span class="ml-pick none" aria-hidden="true"></span><span class="ml-mark" aria-hidden="true">${glyph("plus", 14)}</span><span class="ml-name">${escape(words.addMachine)}</span></button>`
          : "";
      const inside = group.inside
        .map((band) => `<h3 class="ml-band-head">${box(`list:${band.key}`, band.label, nodes, now)}<span>${escape(band.label)}</span><span class="ml-group-sub">${escape(words.groupSub(band.nodes.length, online(band.nodes)))}</span></h3>${rows(band.nodes)}`)
        .join("");
      return `<section class="ml-group"><h2 class="ml-group-head">${box(`list:${group.key}`, group.label, nodes, now)}<span>${escape(group.label)}</span>${visitor}<span class="ml-group-sub">${escape(words.groupSub(all.length, online(all)))}</span></h2>${rows(group.nodes)}${add}${inside}</section>`;
    })
    .join("");
  return `<div class="ml-hubs">${hubRows.join("")}</div>${body}`;
}

/** A heading's box: all its machines that may be chosen, half filled when
 *  only some are; none where none may be. */
function box(key: string, label: string, nodes: readonly MapNode[], now: ListNow): string {
  const keys = pickableIn(nodes, key);
  if (!keys.length) return "";
  const chosen = keys.filter((one) => now.picked.has(one)).length;
  const state = chosen === 0 ? "false" : chosen === keys.length ? "true" : "mixed";
  return `<span class="ml-pick" role="checkbox" aria-checked="${state}" aria-label="${escape(now.pick.all(label))}" data-ml-pick-all="${escape(key)}"></span>`;
}

function row(node: MapNode, machine: Machine | null, now: ListNow, words: MachinesWords): string {
  const { world } = now;
  const assist = node.hold === "assist";
  const state = node.online === true ? "online" : node.online === false ? "offline" : "unknown";
  const facts = machine && "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
  const form: GlyphName = facts?.form === "laptop" ? "laptop" : facts?.form === "server" ? "server" : "desktop";
  const visitor = node.hold === "visitor";
  const managed = !assist && !visitor && isManaged(world, node.name);
  const tags = [
    node.here ? `<span class="tag">${escape(words.here)}</span>` : "",
    managed ? `<span class="tag" title="${escape(words.managedWhat)}">${escape(words.managed)}</span>` : "",
    node.directAt?.length ? `<span class="ml-direct" title="${escape(words.takesDirect)}">${glyph("link", 14)}</span>` : "",
  ].join("");
  const system = assist
    ? agentsOn(world, node.name)
    : visitor
      ? `<span>${escape(words.visitorMachine)}</span>`
      : node.device
        ? `${glyph("phone", 14)}<span>${escape(words.phone)}</span>`
        : facts
          ? `${glyph(form, 14)}<span>${escape(osName(facts.os))}</span>`
          : "";
  const link_ = linkSaid(node, machine, now, words);
  const given = assist ? "" : givenSaid(world, machine, words);
  const busy = node.busy > 0 ? busyDots(node.busy, words) : "";
  const title = node.title;
  const chooses = pickable(node);
  const chosen = chooses && now.picked.has(node.key);
  const pick = chooses
    ? `<span class="ml-pick" role="checkbox" aria-checked="${chosen}" aria-label="${escape(now.pick.one(node.name))}" data-ml-pick="${escape(node.key)}"></span>`
    : `<span class="ml-pick none" aria-hidden="true"></span>`;
  const inner = `${pick}<span class="ml-mark" aria-hidden="true"><span class="ml-dot"></span></span>
      <span class="ml-name"><span class="ml-name-text">${escape(node.name)}</span>${tags}</span>
      <span class="ml-system">${system}</span>
      <span class="ml-link">${link_}</span>
      <span class="ml-given">${given}</span>
      <span class="ml-busy">${busy}</span>
      <span class="ml-go" aria-hidden="true">${glyph("chevron", 16)}</span>`;
  const kind = `ml-row ${state}${assist ? " assist" : ""}${visitor ? " visitor" : ""}${now.selected === node.key || chosen ? " chosen" : ""}`;
  const keyed = chooses ? ` data-ml-key="${escape(node.key)}"` : "";
  // A machine one may work on has its own page; an assisting agent's and a
  // phone open their details here; a visitor's is only seen.
  if (visitor) return `<div class="${kind}" title="${escape(title)}">${inner.replace(glyph("chevron", 16), "")}</div>`;
  return assist || node.device
    ? `<button class="${kind}" type="button" data-ml-open="${escape(node.key)}" title="${escape(title)}">${inner}</button>`
    : `<a class="${kind}" href="${link({ kind: "machine", name: node.name })}" title="${escape(title)}"${keyed}>${inner}</a>`;
}

/** How a machine is there: through which machine it reaches the hub, or the
 *  round trip of its line; since when it is away. */
function linkSaid(node: MapNode, machine: Machine | null, now: ListNow, words: MachinesWords): string {
  if (node.online === false) {
    const since = machine?.since ?? node.since ?? null;
    return escape(words.offline(since ? now.when(new Date(since), now.now) : null));
  }
  if (node.online === null) return escape(words.unknown);
  if (node.through) return escape(words.viaShort(node.through));
  return node.hubMs !== undefined ? `<span class="ml-ms">${escape(words.listMs(node.hubMs))}</span>` : escape(words.onlineNow);
}

/** Whom a machine of the account's own is shared with. */
function givenSaid(world: WorldNow, machine: Machine | null, words: MachinesWords): string {
  if (!machine) return "";
  const logins = loginsOf(world.accounts);
  const mine = machine.here || logins.some((login) => owns(machine, login));
  if (!mine) return "";
  const to = [...new Set(world.accounts.hubs.flatMap((hub) => hub.shares.filter((share) => share.machine === machine.name).map((share) => share.account)))];
  if (!to.length) return "";
  return `${glyph("colleague", 14)}<span>${escape(to.join(words.separator))}</span>`;
}

/** The agents of another's assisting on their machine. */
function agentsOn(world: WorldNow, name: string): string {
  const agents = [...new Set(assistantsOf(world).filter((one) => one.written === name).map((one) => one.agent))];
  return agents.map((agent) => `${glyph(agent === "claude" ? "claude" : "codex", 14)}<span>${escape(agentName(agent))}</span>`).join("");
}

function busyDots(count: number, words: MachinesWords): string {
  const dots = Array.from({ length: Math.min(count, DOTS) }, () => `<i></i>`).join("");
  return `<span class="ml-busy-dots" title="${escape(words.busy(count))}">${dots}${count > DOTS ? `<span>+${count - DOTS}</span>` : ""}</span>`;
}

/** Whether a machine is a managed machine: registered under an account,
 *  logged in to none (`world-network.md`, 受管机器). */
export function isManaged(world: WorldNow, name: string): boolean {
  return world.accounts.hubs.some((hub) => hub.members.some((member) => member.name === name && !!member.managed));
}

/** The hub's small knot, as the drawing marks it. */
function knot(): string {
  return `<svg class="ml-knot" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="5" fill="none" stroke="currentColor" stroke-width="1.3"/><circle cx="8" cy="8" r="1.8" fill="currentColor"/></svg>`;
}
