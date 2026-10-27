// The machines as a list (`world-gui.md`, 账号、机器、共享与协助): the same
// World the drawing shows, set out as a tree. The hub stands first; then a
// section for each way a machine is the account's to see: its own, those
// shared with it, the members' its administrator sees, and those whose
// agents assist it; under each section's name a hand-drawn line. Every row
// is a twisty, an icon and a name. The account's own machines come first,
// this computer first among them, then the groups its owner made
// (`world-network.md`, 机器的分组), each a row that opens onto its machines.
// Another person is a row that opens onto theirs, their own groups one level
// further in. A leaf keeps the twisty's room, so the icons of one level stand
// in a column and a child's icon stands under its parent's name. Whether a
// machine is there is told by its icon and name in ink or in grey, with no
// dot of its own. A machine's row goes to its own page; a hub's, a phone's or
// an assisting agent's opens the details beside the list. A visitor's
// machine, which the network's administrator only sees, is a faint row that
// goes nowhere. A machine the account works on has a small box at the start
// of its row, and a group, a person and the account's own section one for
// all their machines, half filled when some are chosen: the boxes show where
// pointed at, and on every row once any is chosen (`machine-picking.ts`).
// A list of thousands is drawn as far as it is read, the rest coming as its
// end comes near.
//
// The update view is this same tree, narrowed to the machines this device's
// accounts may update, with what each runs, a mark of where its update
// stands and its own updating by itself in place of its facts; the version
// ruler and the network's policy stand over it (`update-ruler.ts`).

import { glyph, type GlyphName } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { WorldNow } from "../world/bridge";
import { loginsOf } from "../world/accounts";
import { agentName, osName } from "../world/names";
import type { MachinesWords } from "./account-words";
import { compareBoxes, naturally } from "./machine-groups";
import { mapOf } from "./map-of";
import type { MapNode } from "./map-nodes";
import { pickable, pickableIn } from "./machine-picking";
import { readOn } from "./read-on";
import { link } from "./places";
import { assistantsOf, machinesOf, owns, type Machine } from "./reach";
import { indexOf } from "./world-index";
import { compareVersions, type MachineUpdate } from "../world/updates";
import { groupMark, markOf, runningOf, shortVersion, type Mark } from "./update-marks";
import type { UpdateViewWords } from "./update-view-words";

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
  /** While machines are looked for, those that match, by key: the list
   *  holds them alone, every row that stands over them open. */
  found?: ReadonlySet<string> | null | undefined;
  /** The names of a row's box and a heading's. */
  pick: { one(name: string): string; all(label: string): string };
  /** Whether a group's or a person's row is open: as the viewer last left
   *  it, else `byDefault`. */
  open(key: string, byDefault: boolean): boolean;
  /** The update view, when it is the one shown. */
  updates?: UpdatesNow | undefined;
  /** At most how many rows are drawn, a fold's or a machine's alike; the
   *  rest wait under a mark at the end, drawn as it comes near. Every row
   *  when absent. */
  reach?: number | undefined;
}

/** The list drawn, and where its machines stand in it. */
export interface ListDrawn {
  html: string;
  /** Every machine's row under a fold that stands open, in order, drawn or
   *  not yet, by key: the row it is, counting every row before it. */
  rows: Map<string, number>;
  /** Rows wait to be drawn. */
  more: boolean;
}

/** The update view: the machines that may be updated, by name, and what
 *  stands over the tree. */
export interface UpdatesNow {
  byName: ReadonlyMap<string, MachineUpdate>;
  /** The release line every version shown shares. */
  line: string | null;
  words: UpdateViewWords;
  /** The ruler and the policy, drawn. */
  head: string;
  /** What scheduling a machine's update just said, by its name. */
  said: ReadonlyMap<string, string>;
}

/** Up to this many dots for the agents at work on a machine; more are a
 *  number. */
const DOTS = 4;

/** A group of the account's own opens by itself up to this many machines; a
 *  lab of eighty waits to be opened. */
const OPEN_GROUP = 12;

/** A person whose machines are shared with the account opens by itself up
 *  to this many; a member the administrator only oversees waits closed. */
const OPEN_PERSON = 6;

/** Machines of one owner put together: the ones in no group, then each
 *  group of theirs. */
interface Holding {
  key: string;
  label: string;
  loose: MapNode[];
  bands: { key: string; label: string; nodes: MapNode[] }[];
}

export function machineList(now: ListNow, words: MachinesWords): ListDrawn {
  const { world, updates } = now;
  const { hubs, nodes: all, hubLabels } = mapOf(world, words);
  const machines = new Map(machinesOf(world).map((machine) => [`m:${machine.written}`, machine]));
  const listed = updates ? all.filter((node) => updateOf(node, machines, updates)) : all;
  const { found } = now;
  const nodes = found ? listed.filter((node) => found.has(node.key)) : listed;
  const hubRows = hubs.map((hub) => {
    const seen = world.accounts.hubs.find((one) => one.hub === hub.id);
    const ms = seen?.link.kind === "online" ? seen.link.ms : null;
    const count = listed.filter((node) => node.hubs.includes(hub.id) && node.hold !== "assist" && !node.device).length;
    const state = [hubLabels.get(hub.id) ?? "", ms ? words.listMs(ms) : ""].filter(Boolean).join("　");
    // Since when the line is down, and why, for a pointer resting on it.
    const down = seen?.link.kind === "connecting" ? words.hubDownSince(now.when(new Date(seen.link.since), now.now), seen.link.last_error ?? null) : "";
    const key = `hub:${hub.id}`;
    return `<button class="ml-row ml-hub${hub.online ? " online" : " offline"}${now.selected === key ? " chosen" : ""}" type="button" data-ml-open="${escape(key)}"${down ? ` title="${escape(down)}"` : ""}>
      <span class="ml-pick none" aria-hidden="true"></span>
      ${nameCell(knot(), `<span class="ml-name-text">${escape(hub.name)}</span><span class="tag">${escape(words.listHub)}</span>`, false)}
      <span class="ml-system">${escape(words.hubMachines(count))}</span>
      <span class="ml-link">${escape(state)}</span>
      <span class="ml-given"></span>
      <span class="ml-busy"></span>
      <span class="ml-go" aria-hidden="true">${glyph("chevron", 16)}</span>
    </button>`;
  });

  // Each owner's machines, loose and in groups, in the drawing's order.
  const holdings: Holding[] = [];
  const held = new Map<string, Holding>();
  const banded = new Map<string, Holding["bands"][number]>();
  for (const node of nodes) {
    const group = node.group ?? { key: node.hold, label: node.hold };
    const owner = group.within ?? group;
    let found = held.get(owner.key);
    if (!found) {
      found = { key: owner.key, label: owner.label, loose: [], bands: [] };
      holdings.push(found);
      held.set(owner.key, found);
    }
    if (!group.within) found.loose.push(node);
    else {
      // A group's key holds its owner's (`boxOf`).
      let band = banded.get(group.key);
      if (!band) {
        band = { key: group.key, label: group.label, nodes: [] };
        found.bands.push(band);
        banded.set(group.key, band);
      }
      band.nodes.push(node);
    }
  }
  const rank = (owner: string) => (owner === "mine" ? 0 : owner.startsWith("assist:") ? 2 : 1);
  holdings.sort((a, b) => compareBoxes(a.key, b.key, rank));
  for (const holding of holdings) holding.bands.sort((a, b) => compareBoxes(a.key, b.key, rank));

  const draw = new Drawing(now, words, machines);
  const mine = holdings.find((one) => one.key === "mine") ?? (now.adds.length && !found ? { key: "mine", label: words.mineGroup, loose: [], bands: [] } : null);
  const people = holdings.filter((one) => one.key.startsWith("of:"));
  const shared = new Set(people.filter((one) => sharedWith(world, machines, one)));
  const members = people.filter((one) => !shared.has(one));
  const assisting = holdings.filter((one) => one.key.startsWith("assist:")).flatMap((one) => one.loose);

  // A section stands with its first row: one wholly beyond what is drawn
  // waits with its rows.
  const reach = now.reach ?? Infinity;
  const sections: string[] = [];
  if (mine) {
    const all = everyNode(mine);
    const first = draw.count;
    const body = draw.leaves(mine.loose, 0) + mine.bands.map((band) => draw.band(band, 0, band.nodes.length <= OPEN_GROUP)).join("");
    const add = now.adds.length && !updates && !found && draw.row()
      ? `<button class="ml-row ml-add" type="button" data-ml-add="${escape(now.adds[0]!)}"><span class="ml-pick none" aria-hidden="true"></span>${nameCell(glyph("plus", 15), `<span class="ml-name-text">${escape(words.addMachine)}</span>`, false)}</button>`
      : "";
    if (first < reach) sections.push(section(words.mineGroup, words.groupSub(all.length, online(all)), box(`list:mine`, words.mineGroup, nodes, now), body + add));
  }
  for (const [label, holders, opens] of [
    [words.sharedSection, [...shared], true],
    [words.membersSection, members, false],
  ] as const) {
    if (!holders.length) continue;
    const first = draw.count;
    const body = holders.map((holder) => draw.person(holder, opens && everyNode(holder).length <= OPEN_PERSON)).join("");
    if (first >= reach) continue;
    const all = holders.flatMap(everyNode);
    sections.push(section(label, words.groupSub(all.length, online(all)), "", body));
  }
  if (assisting.length && !updates) {
    const first = draw.count;
    const body = draw.leaves(assisting, 0);
    if (first < reach) sections.push(section(words.assistSection, words.groupSub(assisting.length, online(assisting)), "", body));
  }
  if (found && !nodes.length) sections.push(`<p class="ml-none">${escape(words.noneFound)}</p>`);
  const more = draw.count > reach;
  const end = more ? readOn : "";
  // The ruler and the policy stay put; the machines under them scroll.
  const html = updates
    ? `<div class="uv-top">${updates.head}</div><div class="uv-rows scroller">${sections.join("")}${end}</div>`
    : `<div class="ml-hubs">${hubRows.join("")}</div>${sections.join("")}${end}`;
  return { html, rows: draw.rows, more };
}

/** The machines found, in the order the list sets them out, whichever view
 *  is shown. */
export function listOrder(world: WorldNow, found: ReadonlySet<string>, words: MachinesWords): string[] {
  const walked = machineList({ world, now: new Date(0), selected: null, adds: [], when: () => "", picked: new Set(), pick: { one: () => "", all: () => "" }, open: () => true, found, reach: 0 }, words);
  return [...walked.rows.keys()];
}

/** What the update view knows of the machine a node is: a phone by its
 *  name, a machine by the name the network gives it. */
function updateOf(node: MapNode, machines: Map<string, Machine>, updates: UpdatesNow): MachineUpdate | undefined {
  return updates.byName.get(machines.get(node.key)?.written ?? node.name);
}

/** A mark as a row's cell: what it says, said when pointed at. */
function markCell(mark: Mark): string {
  const title = [mark.said, mark.why].filter(Boolean).join("\n");
  return `<span class="ml-link uv-mark ${mark.kind}"${title ? ` title="${escape(title)}"` : ""}>${mark.html}</span>`;
}

/** A section: its name, how many and how many online, a hand-drawn line;
 *  then its rows. */
function section(label: string, sub: string, box: string, body: string): string {
  return `<section class="ml-group"><h2 class="ml-group-head">${box}<span>${escape(label)}</span><span class="ml-group-sub">${escape(sub)}</span></h2>${body}</section>`;
}

/** The name cell of every row: the twisty, or its room on a leaf; the
 *  icon; the name and what follows it. */
function nameCell(icon: string, name: string, folds: boolean): string {
  return `<span class="ml-name"><span class="ml-twisty" aria-hidden="true">${folds ? glyph("chevron", 14) : ""}</span><span class="ml-icon" aria-hidden="true">${icon}</span>${name}</span>`;
}

const everyNode = (holding: Holding) => [...holding.loose, ...holding.bands.flatMap((band) => band.nodes)];
const online = (nodes: MapNode[]) => nodes.filter((node) => node.online !== false).length;

/** Whether another person's machines are here because they were shared
 *  with one of this device's logins, not because its administrator sees
 *  every member's. */
function sharedWith(world: WorldNow, machines: Map<string, Machine>, holding: Holding): boolean {
  const logins = loginsOf(world.accounts);
  return everyNode(holding).some((node) => {
    const machine = machines.get(node.key);
    if (!machine || node.hold !== "others") return false;
    return world.accounts.hubs.some((hub) => hub.shares.some((share) => share.machine === machine.name && logins.some((login) => login.hub === hub.hub && login.account === share.account)));
  });
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

/** The rows of the tree, drawn with what every row needs to hand, as many as
 *  the list draws; those after are only walked, for where they stand. */
class Drawing {
  /** Rows walked so far, a fold's or a machine's, drawn or not. */
  count = 0;
  /** Each machine's row walked, in order, by key. */
  readonly rows = new Map<string, number>();
  private readonly reach: number;

  constructor(
    private readonly now: ListNow,
    private readonly words: MachinesWords,
    private readonly machines: Map<string, Machine>,
  ) {
    this.reach = now.reach ?? Infinity;
  }

  /** Takes the next row: whether it is drawn. */
  row(): boolean {
    this.count += 1;
    return this.count <= this.reach;
  }

  /** Machines, this computer first, then by name: the rows stay where they
   *  are as machines come and go. */
  leaves(nodes: MapNode[], depth: number): string {
    return [...nodes]
      .sort((a, b) => Number(b.here) - Number(a.here) || naturally(a.name, b.name))
      .map((node) => this.leaf(node, depth))
      .join("");
  }

  /** A group of an owner's: one row that opens onto its machines. */
  band(band: Holding["bands"][number], depth: number, byDefault: boolean): string {
    const { words } = this;
    const managed = band.nodes.length > 0 && band.nodes.every((node) => isManaged(this.now.world, node.name));
    const tags = managed ? `<span class="tag" title="${escape(words.managedWhat)}">${escape(words.managed)}</span>` : "";
    return this.fold(band.key, glyph("group", 15), band.label, tags, band.nodes, depth, byDefault, () => this.leaves(band.nodes, depth + 1));
  }

  /** Another person: one row that opens onto their machines, their own
   *  groups a level further in. A visitor's is faint and tagged. */
  person(holding: Holding, byDefault: boolean): string {
    const all = everyNode(holding);
    const owner = holding.key.slice("of:".length);
    const visitor = all.some((node) => node.hold === "visitor");
    const tags = visitor ? `<span class="tag">${escape(this.words.visitor)}</span>` : "";
    const face = `<span class="ml-face">${escape(initial(owner))}</span>`;
    return this.fold(holding.key, face, owner, tags, all, 0, byDefault, () => this.leaves(holding.loose, 1) + holding.bands.map((band) => this.band(band, 1, band.nodes.length <= OPEN_GROUP)).join(""), visitor);
  }

  /** A row that opens onto `nodes`, which are every machine its box holds:
   *  its box ticks them all. */
  private fold(key: string, icon: string, label: string, tags: string, nodes: MapNode[], depth: number, byDefault: boolean, inside: () => string, faint = false): string {
    const { now, words } = this;
    const open = now.found ? true : now.open(key, byDefault);
    if (!this.row()) {
      if (open) inside();
      return "";
    }
    if (now.updates) {
      const updates = now.updates;
      const known = nodes.flatMap((node) => updateOf(node, this.machines, updates) ?? []);
      const versions = [...new Set(known.flatMap((machine) => runningOf(machine) ?? []))].sort(compareVersions);
      const span = versions.length ? (versions.length === 1 ? versions[0]! : `${versions[0]}–${versions[versions.length - 1]}`) : "";
      const shown = versions.length ? (versions.length === 1 ? shortVersion(versions[0]!, updates.line) : `${shortVersion(versions[0]!, updates.line)}–${shortVersion(versions[versions.length - 1]!, updates.line)}`) : "";
      const row = `<button class="ml-row ml-fold${open ? " open" : ""}${faint ? " visitor" : ""}" type="button" data-ml-fold="${escape(key)}" aria-expanded="${open}"${indent(depth)}>
      ${box(`list:${key}`, label, nodes, now) || `<span class="ml-pick none" aria-hidden="true"></span>`}
      ${nameCell(icon, `<span class="ml-name-text">${escape(label)}</span>${tags}<span class="ml-count">${escape(words.groupSub(nodes.length, online(nodes)))}</span>`, true)}
      <span class="ml-system uv-version"${span ? ` title="${escape(updates.words.runs(span))}"` : ""}>${escape(shown)}</span>
      ${markCell(groupMark(known, updates.line, updates.words))}
      <span class="ml-given"></span>
      <span class="ml-busy"></span>
      <span class="ml-go" aria-hidden="true"></span>
    </button>`;
      return open ? row + inside() : row;
    }
    const busy = nodes.reduce((sum, node) => sum + node.busy, 0);
    const count = `<span class="ml-count">${escape(words.groupSub(nodes.length, online(nodes)))}</span>`;
    const row = `<button class="ml-row ml-fold${open ? " open" : ""}${faint ? " visitor" : ""}" type="button" data-ml-fold="${escape(key)}" aria-expanded="${open}"${indent(depth)}>
      ${box(`list:${key}`, label, nodes, now) || `<span class="ml-pick none" aria-hidden="true"></span>`}
      ${nameCell(icon, `<span class="ml-name-text">${escape(label)}</span>${tags}`, true)}
      ${count}
      <span class="ml-given"></span>
      <span class="ml-busy">${busy ? `<span class="ml-busy-dots" title="${escape(words.busy(busy))}"><i></i><span>${busy}</span></span>` : ""}</span>
      <span class="ml-go" aria-hidden="true"></span>
    </button>`;
    return open ? row + inside() : row;
  }

  private leaf(node: MapNode, depth: number): string {
    this.rows.set(node.key, this.count);
    if (!this.row()) return "";
    const { now, words } = this;
    const { world } = now;
    const machine = this.machines.get(node.key) ?? null;
    const assist = node.hold === "assist";
    const state = node.online === true ? "online" : node.online === false ? "offline" : "unknown";
    const facts = machine && "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
    const visitor = node.hold === "visitor";
    const managed = !assist && !visitor && isManaged(world, node.name);
    const tags = [
      node.here ? `<span class="tag">${escape(words.here)}</span>` : "",
      managed ? `<span class="tag" title="${escape(words.managedWhat)}">${escape(words.managed)}</span>` : "",
      node.directAt?.length ? `<span class="ml-direct" title="${escape(words.takesDirect)}">${glyph("link", 14)}</span>` : "",
    ].join("");
    // An assisting machine is named by whose agents; its key keeps the machine.
    const agents = assist ? agentsOn(world, node.key.slice("assist:".length)) : [];
    const icon: GlyphName = node.device
      ? "phone"
      : assist
        ? (agents[0] ?? "codex")
        : facts?.form === "laptop"
          ? "laptop"
          : facts?.form === "server"
            ? "server"
            : "desktop";
    const system = assist
      ? escape(node.sub)
      : visitor
        ? escape(words.visitorMachine)
        : node.device
          ? escape(words.phone)
          : facts
            ? escape(osName(facts.os))
            : "";
    const given = assist ? "" : givenSaid(world, machine, words);
    const busy = node.busy > 0 ? busyDots(node.busy, words) : "";
    const chooses = pickable(node);
    const chosen = chooses && now.picked.has(node.key);
    const pick = chooses
      ? `<span class="ml-pick" role="checkbox" aria-checked="${chosen}" aria-label="${escape(now.pick.one(node.name))}" data-ml-pick="${escape(node.key)}"></span>`
      : `<span class="ml-pick none" aria-hidden="true"></span>`;
    const title = assist && agents.length ? `${node.title}　${agents.map(agentName).join("、")}` : node.title;
    const go = visitor ? "" : glyph("chevron", 16);
    const update = now.updates && !assist ? updateOf(node, this.machines, now.updates) : undefined;
    const inner = now.updates && update
      ? `${node.device ? `<span class="ml-pick none" aria-hidden="true"></span>` : pick}
      ${nameCell(glyph(icon, 15), `<span class="ml-name-text">${escape(node.name)}</span>${tags}`, false)}
      ${versionCell(update, now.updates)}
      ${markCell(now.updates.said.has(update.machine) ? { kind: "failed", html: glyph("cross", 16), said: now.updates.said.get(update.machine)! } : markOf(update, now.updates.line, now.updates.words))}
      <span class="ml-given"></span>
      <span class="ml-busy">${autoSwitch(update, now.updates)}</span>
      <span class="ml-go" aria-hidden="true">${go}</span>`
      : `${pick}
      ${nameCell(glyph(icon, 15), `<span class="ml-name-text">${escape(node.name)}</span>${tags}`, false)}
      <span class="ml-system">${system}</span>
      <span class="ml-link">${linkSaid(node, machine, now, words)}</span>
      <span class="ml-given">${given}</span>
      <span class="ml-busy">${busy}</span>
      <span class="ml-go" aria-hidden="true">${go}</span>`;
    const kind = `ml-row ${state}${assist ? " assist" : ""}${visitor ? " visitor" : ""}${now.selected === node.key || chosen ? " chosen" : ""}`;
    // Every machine's row is found by its key, as one looked for is gone to.
    const at = `${indent(depth)} data-ml-row="${escape(node.key)}"`;
    const keyed = chooses ? ` data-ml-key="${escape(node.key)}"` : "";
    // A machine one may work on has its own page; an assisting agent's and a
    // phone open their details here; a visitor's is only seen.
    if (visitor) return `<div class="${kind}" title="${escape(title)}"${at}>${inner}</div>`;
    return assist || node.device
      ? `<button class="${kind}" type="button" data-ml-open="${escape(node.key)}" title="${escape(title)}"${at}>${inner}</button>`
      : `<a class="${kind}" href="${link({ kind: "machine", name: node.name })}" title="${escape(title)}"${keyed}${at}>${inner}</a>`;
  }
}

/** What a machine runs, short, the whole version said when pointed at. */
function versionCell(update: MachineUpdate, updates: UpdatesNow): string {
  const running = runningOf(update);
  return `<span class="ml-system uv-version"${running ? ` title="${escape(updates.words.runs(running))}"` : ""}>${escape(running ? shortVersion(running, updates.line) : "")}</span>`;
}

/** A machine's own updating by itself, a switch where it may be set from
 *  here; none for a phone, or one that never said. */
function autoSwitch(update: MachineUpdate, updates: UpdatesNow): string {
  if (update.kind === "phone" || update.auto === null) return "";
  const settable = update.actions.settings.allowed;
  return `<span class="uv-auto${update.auto ? " on" : ""}${settable ? "" : " fixed"}" role="switch" aria-checked="${update.auto}" aria-label="${escape(updates.words.auto)}" title="${escape(settable ? updates.words.auto : updates.words.reasons[update.actions.settings.reason ?? "other"])}"${settable ? ` data-uv-auto="${escape(update.machine)}"` : ""}><span class="switch-track"><span class="switch-knob"></span></span></span>`;
}

/** How far in a row stands: nothing at the first level. */
function indent(depth: number): string {
  return depth ? ` data-depth="${depth}" style="--depth:${depth}"` : "";
}

/** The first letter of a person's name, for their small round mark. */
function initial(name: string): string {
  return [...name][0]?.toUpperCase() ?? "";
}

/** How a machine is there: through which machine it reaches the hub, or the
 *  round trip of its line; since when it is away. */
function linkSaid(node: MapNode, machine: Machine | null, now: ListNow, words: MachinesWords): string {
  if (node.online === false) {
    const since = machine?.since ?? node.since ?? null;
    return escape(words.offline(since ? now.when(new Date(since), now.now) : null));
  }
  if (node.online === null) return escape(words.unknown);
  // While no hub it is on is connected, how its line goes now is not known:
  // this computer says it is reconnecting, the others stand as they were.
  const heard = node.hubs.some((id) => now.world.accounts.hubs.find((hub) => hub.hub === id)?.link.kind === "online");
  if (!heard) return node.here ? escape(words.hubConnecting) : "";
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

/** The agents of another's assisting on their machine, by their marks. */
function agentsOn(world: WorldNow, name: string): ("claude" | "codex")[] {
  return [...new Set(assistantsOf(world).filter((one) => one.written === name).map((one) => (one.agent === "claude" ? "claude" : "codex")))];
}

function busyDots(count: number, words: MachinesWords): string {
  const dots = Array.from({ length: Math.min(count, DOTS) }, () => `<i></i>`).join("");
  return `<span class="ml-busy-dots" title="${escape(words.busy(count))}">${dots}${count > DOTS ? `<span>+${count - DOTS}</span>` : ""}</span>`;
}

/** Whether a machine is a managed machine: registered under an account,
 *  logged in to none (`world-network.md`, 受管机器). */
export function isManaged(world: WorldNow, name: string): boolean {
  return indexOf(world).managed.has(name);
}

/** The hub's small knot, as the drawing marks it. */
function knot(): string {
  return `<svg class="ml-knot" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="5" fill="none" stroke="currentColor" stroke-width="1.3"/><circle cx="8" cy="8" r="1.8" fill="currentColor"/></svg>`;
}
