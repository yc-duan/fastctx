// The machines the agents page can show, one at a time (`world-gui.md`,
// 智能体页): the head's machine is a menu of them, grouped as the user's
// own, those shared with them, other members' that an administrator may
// look at, and those whose agents assist them; each says whether it is
// online. Which one was shown last is kept on this device.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Login as AccountLogin } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { Timestamp } from "../world/wire";
import { assistantsOf, machinesOf, ownerOf, owns } from "./reach";

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
  /** The login of this device that owns it, which lets its agents assist
   *  others and sets their plans. */
  ownerLogin: AccountLogin | null;
}

export interface MachinePickWords {
  holds: Record<Hold, string>;
  here: string;
  ownedBy(owner: string): string;
  offline: string;
  pickMachine: string;
}

/** Every machine the page can show, this computer first. */
export function pickables(world: WorldNow | null): Pickable[] {
  const logins = loginsOf(world?.accounts);
  const picked: Pickable[] = [];
  for (const machine of machinesOf(world)) {
    const ownerLogin = machine.here
      ? (logins.find((login) => world?.nodes.hubs.some((hub) => hub.hub === login.hub && hub.home === login.account)) ?? null)
      : (logins.find((login) => owns(machine, login)) ?? null);
    const shared = logins.some((login) => world?.accounts.hubs.some((hub) => hub.hub === login.hub && hub.shares.some((share) => share.machine === machine.name && share.account === login.account)));
    const hold: Hold = machine.here || ownerLogin ? "mine" : shared ? "shared" : "others";
    picked.push({
      name: machine.name,
      written: machine.written,
      here: machine.here,
      online: machine.here ? true : machine.online,
      since: machine.since,
      hold,
      owner: hold === "mine" ? null : (logins.map((login) => ownerOf(machine, login)).find(Boolean) ?? null),
      ownerLogin,
    });
  }
  for (const assistant of assistantsOf(world)) {
    if (picked.some((one) => one.written === assistant.written)) continue;
    picked.push({ name: assistant.machine, written: assistant.written, here: false, online: assistant.online, since: assistant.since, hold: "assist", owner: assistant.owner, ownerLogin: null });
  }
  return picked.sort((a, b) => HOLDS.indexOf(a.hold) - HOLDS.indexOf(b.hold) || Number(b.here) - Number(a.here) || Number(b.online === true) - Number(a.online === true) || a.written.localeCompare(b.written));
}

/** The machine to show: the one named, else the one kept, else this
 *  computer, else the first. */
export function chosen(machines: Pickable[], named: string | null, kept: string | null): Pickable | null {
  const find = (name: string | null) => (name ? (machines.find((one) => one.name === name || one.written === name) ?? null) : null);
  return find(named) ?? find(kept) ?? machines.find((one) => one.here) ?? machines[0] ?? null;
}

function sub(machine: Pickable, words: MachinePickWords): string {
  if (machine.here) return words.here;
  if (machine.online === false) return words.offline;
  return machine.owner ? words.ownedBy(machine.owner) : "";
}

/** The head's machine: a pill that opens the menu. */
export function pickButton(machine: Pickable | null, words: MachinePickWords): string {
  if (!machine) return "";
  const dot = machine.online === true ? " online" : machine.online === false ? " offline" : "";
  return `<button class="agents-pick" type="button" data-pick-open aria-haspopup="menu" aria-label="${escape(words.pickMachine)}" title="${escape(words.pickMachine)}">
    <span class="agents-pick-dot${dot}" aria-hidden="true"></span><span class="agents-pick-name">${escape(machine.written)}</span>${machine.here ? `<span class="agents-pick-sub">${escape(words.here)}</span>` : ""}${glyph("chevron", 14)}</button>`;
}

/** The menu: a group for each hold, each machine with whether it is
 *  online and whose it is. */
export function pickMenu(machines: Pickable[], current: Pickable | null, words: MachinePickWords): string {
  return HOLDS.map((hold) => {
    const held = machines.filter((one) => one.hold === hold);
    if (!held.length) return "";
    const items = held
      .map((machine) => {
        const on = machine.written === current?.written;
        const dot = machine.online === true ? " online" : machine.online === false ? " offline" : "";
        return `<button class="agents-pick-item${on ? " current" : ""}" type="button" role="menuitemradio" aria-checked="${on}" data-pick="${escape(machine.written)}">
          <span class="agents-pick-dot${dot}" aria-hidden="true"></span><span class="agents-pick-name">${escape(machine.written)}</span><span class="agents-pick-sub">${escape(sub(machine, words))}</span></button>`;
      })
      .join("");
    return `<div class="agents-pick-group" role="group" aria-label="${escape(words.holds[hold])}"><span class="agents-pick-hold">${escape(words.holds[hold])}</span>${items}</div>`;
  }).join("");
}
