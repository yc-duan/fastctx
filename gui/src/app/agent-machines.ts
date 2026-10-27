// The machines the agents page can show, one at a time (`world-gui.md`,
// 智能体页): the head's machine is a menu of them, grouped as the user's
// own, those shared with them, other members' that an administrator may
// look at, and those whose agents assist them, each group's machines their
// owner put in groups (`world-network.md`, 机器的分组) after the rest under
// the group's name; each says whether it is online. Which one was shown
// last is kept on this device.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Login as AccountLogin } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { Timestamp } from "../world/wire";
import { isManaged } from "./machine-list";
import { assistantsOf, machinesOf, ownerOf, owns } from "./reach";
import { indexOf, pair } from "./world-index";

export type Hold = "mine" | "shared" | "others" | "assist";
const HOLDS: Hold[] = ["mine", "shared", "others", "assist"];

/** A machine the page can show. */
export interface Pickable {
  /** As its node is asked by. */
  name: string;
  written: string;
  here: boolean;
  online: boolean | null;
  since: Timestamp | null;
  hold: Hold;
  /** Whose it is, where it is not the user's. */
  owner: string | null;
  /** The group its owner put it in. */
  group: string | null;
  /** The login of this device that holds it, its owner's or one it is
   *  shared with whole, which lets its agents assist others and sets their
   *  plans. */
  ownerLogin: AccountLogin | null;
  /** What kind of computer it is, for its icon. */
  form: "desktop" | "laptop" | "server";
}

export interface MachinePickWords {
  /** Whose agents assist the user, where the menu would name their machine. */
  assistAgents(owner: string): string;
  holds: Record<Hold, string>;
  here: string;
  ownedBy(owner: string): string;
  offline: string;
  pickMachine: string;
}

/** Every machine the page can show, this computer first; made once for
 *  each World, and nothing changes what it returns. */
export function pickables(world: WorldNow | null): Pickable[] {
  if (!world) return [];
  const kept = made.get(world);
  if (kept) return kept;
  const picked = pickablesOf(world);
  made.set(world, picked);
  return picked;
}

const made = new WeakMap<WorldNow, Pickable[]>();

function pickablesOf(world: WorldNow): Pickable[] {
  const logins = loginsOf(world.accounts);
  const index = indexOf(world);
  const picked: Pickable[] = [];
  for (const machine of machinesOf(world)) {
    // A managed machine's agents work for no one: the page has nothing of
    // it (`world-network.md`, 受管机器).
    if (isManaged(world, machine.name)) continue;
    const ownerLogin = machine.here
      ? (logins.find((login) => world.nodes.hubs.some((hub) => hub.hub === login.hub && hub.home === login.account)) ?? null)
      : (logins.find((login) => owns(machine, login)) ?? null);
    const sharedLogin = logins.find((login) => index.hubs.get(login.hub)?.shares.has(pair(machine.name, login.account))) ?? null;
    const hold: Hold = machine.here || ownerLogin ? "mine" : sharedLogin ? "shared" : "others";
    const group = index.grouped.get(machine.name) ?? null;
    const facts = "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
    picked.push({
      name: machine.name,
      written: machine.written,
      here: machine.here,
      online: machine.here ? true : machine.online,
      since: machine.since,
      hold,
      owner: hold === "mine" ? null : (logins.map((login) => ownerOf(machine, login)).find(Boolean) ?? null),
      group,
      // Shared whole, it is managed by the login it is shared with as by
      // its owner (`world-network.md`, 共享机器与智能体协助).
      ownerLogin: ownerLogin ?? sharedLogin,
      form: facts?.form === "laptop" ? "laptop" : facts?.form === "server" ? "server" : "desktop",
    });
  }
  const written = new Set(picked.map((one) => one.written));
  for (const assistant of assistantsOf(world)) {
    if (written.has(assistant.written)) continue;
    picked.push({ name: assistant.machine, written: assistant.written, here: false, online: assistant.online, since: assistant.since, hold: "assist", owner: assistant.owner, group: null, ownerLogin: null, form: "desktop" });
  }
  // Within a hold, those in no group first, then each group by its name.
  const grouped = (a: Pickable, b: Pickable) => Number(a.group !== null) - Number(b.group !== null) || (a.group ?? "").localeCompare(b.group ?? "");
  return picked.sort((a, b) => HOLDS.indexOf(a.hold) - HOLDS.indexOf(b.hold) || grouped(a, b) || Number(b.here) - Number(a.here) || Number(b.online === true) - Number(a.online === true) || a.written.localeCompare(b.written));
}

/** The machine to show: the one named, else the one kept, else this
 *  computer, else the first. */
export function chosen(machines: Pickable[], named: string | null, kept: string | null): Pickable | null {
  const find = (name: string | null) => (name ? (machines.find((one) => one.name === name || one.written === name) ?? null) : null);
  return find(named) ?? find(kept) ?? machines.find((one) => one.here) ?? machines[0] ?? null;
}

/** What the menu and its pill call a machine: its name, or for one whose
 *  agents assist the user, whose agents they are, never the computer. */
function named(machine: Pickable, words: MachinePickWords): string {
  return machine.hold === "assist" ? words.assistAgents(machine.owner ?? "") : machine.written;
}

/** Its mark: the computer's, or for agents assisting the user, a person's. */
function mark(machine: Pickable): string {
  return glyph(machine.hold === "assist" ? "colleague" : machine.form, 15);
}

function sub(machine: Pickable, words: MachinePickWords): string {
  if (machine.here) return words.here;
  if (machine.hold === "assist") return machine.online === false ? words.offline : "";
  if (machine.online === false) return words.offline;
  return machine.owner ? words.ownedBy(machine.owner) : "";
}

/** The head's machine: a pill that opens the menu. */
export function pickButton(machine: Pickable | null, words: MachinePickWords): string {
  if (!machine) return "";
  const dot = machine.online === true ? " online" : machine.online === false ? " offline" : "";
  return `<button class="agents-pick" type="button" data-pick-open aria-haspopup="menu" aria-label="${escape(words.pickMachine)}" title="${escape(words.pickMachine)}">
    <span class="agents-pick-icon${dot}" aria-hidden="true">${mark(machine)}</span><span class="agents-pick-name">${escape(named(machine, words))}</span>${machine.here ? `<span class="agents-pick-sub">${escape(words.here)}</span>` : ""}${glyph("chevron", 14)}</button>`;
}

/** The menu: a group for each hold, each machine with whether it is
 *  online and whose it is, those in a group of their owner's under its
 *  name. */
export function pickMenu(machines: Pickable[], current: Pickable | null, words: MachinePickWords): string {
  return HOLDS.map((hold) => {
    const held = machines.filter((one) => one.hold === hold);
    if (!held.length) return "";
    const items = held
      .map((machine, at) => {
        const on = machine.written === current?.written;
        const dot = machine.online === true ? " online" : machine.online === false ? " offline" : "";
        const band = machine.group !== null && machine.group !== held[at - 1]?.group ? `<span class="agents-pick-band">${escape(machine.group)}</span>` : "";
        return `${band}<button class="agents-pick-item${on ? " current" : ""}" type="button" role="menuitemradio" aria-checked="${on}" data-pick="${escape(machine.written)}">
          <span class="agents-pick-icon${dot}" aria-hidden="true">${mark(machine)}</span><span class="agents-pick-name">${escape(named(machine, words))}</span><span class="agents-pick-sub">${escape(sub(machine, words))}</span></button>`;
      })
      .join("");
    return `<div class="agents-pick-group" role="group" aria-label="${escape(words.holds[hold])}"><span class="agents-pick-hold">${escape(words.holds[hold])}</span>${items}</div>`;
  }).join("");
}
