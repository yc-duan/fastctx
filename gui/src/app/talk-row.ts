// One conversation as a line in a list: which agent, its name, whether it
// works or waits, and on what kind of device it lives. The marks stand in
// fixed columns, so a long name never moves them; the name is cut short and
// whole on hover, and the device says its name and system on hover or a
// long press (`world-gui.md`, 用词).

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Address } from "../world/events";
import type { WorldNow } from "../world/bridge";
import { agentName, osName, titleOf } from "../world/names";
import type { MachineFacts, ResidentNow } from "../world/world";
import { link, type Place } from "./places";
import { machinesOf } from "./reach";
import { isUnreached, residentsOf, unreachedOf } from "./store";

export interface TalkRowWords {
  /** "Claude · laptop". */
  someone(agent: string, machine: string): string;
  busy(lasted: string): string;
  waiting: string;
  /** Its machine is away: the conversation is listed as it last was. */
  offline: string;
  span(ms: number): string;
  /** The kind of device, for a screen reader: "laptop". */
  device(form: "desktop" | "laptop" | "server"): string;
}

/** The conversations of the World a list shows: every one not removed,
 *  those on a machine away as it last answered, the ones asked in to help
 *  another left out, since they hang under the one that asked them. Most
 *  recently stirred first. */
export function conversationsOf(world: WorldNow | null): ResidentNow[] {
  return byRecent([...residentsOf(world), ...unreachedOf(world)].filter((r) => r.about.origin.kind !== "colleague"));
}

export function byRecent(residents: ResidentNow[]): ResidentNow[] {
  const stirred = (r: ResidentNow) => Date.parse(r.now?.since ?? r.about.born_at);
  return [...residents].sort((a, b) => stirred(b) - stirred(a));
}

/** Each machine's facts by name, for the device marks. */
export function factsByMachine(world: WorldNow | null): Map<string, MachineFacts> {
  const facts = new Map<string, MachineFacts>();
  for (const machine of machinesOf(world)) if ("Ok" in machine.facts) facts.set(machine.name, machine.facts.Ok);
  return facts;
}

export function talkRow(r: ResidentNow, place: Place, now: Date, facts: Map<string, MachineFacts>, words: TalkRowWords): string {
  const address: Address = { resident: r.about.id, machine: r.about.machine };
  const current = place.kind === "talk" && place.address.resident === address.resident && place.address.machine === address.machine;
  const agent = agentName(r.about.host.agent);
  const someone = words.someone(agent, r.about.machine);
  const name = titleOf(r) ?? someone;
  // What a machine away last said of a conversation is not said as now.
  const away = isUnreached(r);
  const waiting = !away && !!r.now?.waiting_for_user;
  const busy = !away && r.now?.activity === "busy";
  const state = away ? words.offline : waiting ? words.waiting : busy ? words.busy(words.span(now.getTime() - Date.parse(r.now!.since))) : "";
  const kind = away ? "mark-away" : waiting ? "mark-ask" : busy ? "mark-busy" : "";
  const host = r.about.host.agent === "claude" || r.about.host.agent === "codex" ? glyph(r.about.host.agent, 16) : glyph("agents", 16);
  const known = facts.get(r.about.machine);
  // A node too old to say what it is runs on a computer more often than not.
  const form = known?.form ?? "desktop";
  const device = [r.about.machine, known ? osName(known.os) : ""].filter(Boolean).join(" · ");
  return `<li><a class="talk-row${kind ? ` ${kind}` : ""}${current ? " current" : ""}" href="${link({ kind: "talk", address })}"${current ? ' aria-current="page"' : ""}>
    <span class="talk-host" title="${escape(agent)}">${host}</span>
    <span class="talk-name" title="${escape([name, name === someone ? "" : someone].filter(Boolean).join("\n"))}">${escape(name)}</span>
    <span class="talk-state" title="${escape(state)}"><span class="mark${kind ? ` ${kind}` : ""}" aria-hidden="true"></span><span class="spoken">${escape(state)}</span></span>
    <span class="talk-device" title="${escape(device)}" aria-label="${escape(`${words.device(form)} ${r.about.machine}`)}">${glyph(form, 16)}</span>
  </a></li>`;
}
