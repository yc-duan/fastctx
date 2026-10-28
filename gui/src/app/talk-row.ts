// One conversation as a line in a list: which agent, its name, whether it
// works or waits, and on what kind of device it lives. The marks stand in
// fixed columns, so a long name never moves them; the name is cut short and
// whole on hover, and the device says its name and system on hover or a
// long press (`world-gui.md`, 用词). On hover the marks give way to archiving
// it, or bringing it back, at once and a "⋯" for all that can be done to it
// (`talk-menu.ts`); a touch screen presses long for the same.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Address } from "../world/events";
import type { WorldNow } from "../world/bridge";
import { agentName, osName, titleOf } from "../world/names";
import type { MachineFacts, ResidentNow } from "../world/world";
import { link, type Place } from "./places";
import { machinesOf } from "./reach";
import { isUnreached, residentsOf, unreachedOf } from "./store";
import type { TalkActs } from "./talk-acts";

export interface TalkRowWords {
  /** "Claude laptop". */
  someone(agent: string, machine: string): string;
  /** An agent assisting the user, as whose it is, never its machine. */
  someoneAssisting(agent: string, owner: string): string;
  busy(lasted: string): string;
  waiting: string;
  /** Its machine is away: the conversation is listed as it last was. */
  offline: string;
  span(ms: number): string;
  /** The kind of device, for a screen reader: "laptop". */
  device(form: "desktop" | "laptop" | "server"): string;
  /** It was forked from another conversation. */
  forked: string;
}

/** What a row says of the user's own acts on it. */
export interface RowActs {
  more: string;
  archive: string;
  restore: string;
  /** Its machine is there to be asked now: archiving goes ahead. */
  reachable: boolean;
  deleting: string;
  /** It is being deleted. */
  isDeleting: boolean;
  /** Its name is a field now, holding this. */
  renaming: string | null;
}

/** The conversations of the World a list shows, as the user's acts on them
 *  show them: those on a machine away as it last answered, the ones asked in
 *  to help another left out, since they hang under the one that asked them,
 *  and the sessions the user opened on their own and never brought in, which
 *  are theirs and show in the activity (`world-layers.md`). Those listed
 *  apart from those archived, most recently stirred first. */
export function talksOf(world: WorldNow | null, acts: TalkActs): { listed: ResidentNow[]; archived: ResidentNow[] } {
  const every = byRecent(
    [...residentsOf(world, true), ...unreachedOf(world, true)]
      .filter((r) => r.about.origin.kind !== "colleague" && !r.about.side && (r.about.world_started || r.about.native_resumed))
      .map((r) => acts.view(r)),
  );
  return { listed: every.filter((r) => !r.removed), archived: every.filter((r) => r.removed) };
}

/** What a conversation dragged from a list carries: its address, for the
 *  page it is dropped on to open it beside its own (`side/mount.ts`). */
export const TALK_DRAG = "application/x-fastctx-talk";

export function byRecent(residents: ResidentNow[]): ResidentNow[] {
  // Each one's moment is read once, not at every comparison.
  const stirred = residents.map((r) => ({ r, at: Date.parse(r.now?.since ?? r.about.born_at) }));
  return stirred.sort((a, b) => b.at - a.at).map(({ r }) => r);
}

/** Each machine's facts by name, for the device marks. */
export function factsByMachine(world: WorldNow | null): Map<string, MachineFacts> {
  const facts = new Map<string, MachineFacts>();
  for (const machine of machinesOf(world)) if ("Ok" in machine.facts) facts.set(machine.written, machine.facts.Ok);
  return facts;
}

export function talkRow(r: ResidentNow, place: Place, now: Date, facts: Map<string, MachineFacts>, words: TalkRowWords, acts?: RowActs, assisting: string | null = null): string {
  const address: Address = { resident: r.about.id, machine: r.about.machine };
  const current = place.kind === "talk" && place.address.resident === address.resident && place.address.machine === address.machine;
  const agent = agentName(r.about.host.agent);
  // An agent assisting the user is said as whose it is: its machine is not the user's to see.
  const someone = assisting !== null ? words.someoneAssisting(agent, assisting) : words.someone(agent, r.about.machine);
  const name = titleOf(r) ?? someone;
  // What a machine away last said of a conversation is not said as now.
  const away = isUnreached(r);
  const archived = !!r.removed;
  const deleting = !!acts?.isDeleting;
  const waiting = !away && !archived && !!r.now?.waiting_for_user;
  const busy = !away && !archived && r.now?.activity === "busy";
  const state = deleting ? acts.deleting : away ? words.offline : waiting ? words.waiting : busy ? words.busy(words.span(now.getTime() - Date.parse(r.now!.since))) : "";
  const kind = deleting ? "deleting" : away ? "mark-away" : waiting ? "mark-ask" : busy ? "mark-busy" : "";
  const host = r.about.host.agent === "claude" || r.about.host.agent === "codex" ? glyph(r.about.host.agent, 16) : glyph("agents", 16);
  const known = facts.get(r.about.machine);
  // A node too old to say what it is runs on a computer more often than not.
  const form = known?.form ?? "desktop";
  const device = assisting !== null ? someone : [r.about.machine, known ? osName(known.os) : ""].filter(Boolean).join(" ");
  const named =
    acts?.renaming !== null && acts?.renaming !== undefined
      ? `<input class="talk-rename" value="${escape(acts.renaming)}" aria-label="${escape(name)}" spellcheck="false" autocomplete="off">`
      : `<span class="talk-name" title="${escape([name, name === someone ? "" : someone].filter(Boolean).join("\n"))}">${escape(name)}</span>`;
  // A fork says so beside its name, quietly.
  const titled = r.about.forked_from && !acts?.renaming
    ? `<span class="talk-named">${named}<span class="talk-forked" title="${escape(words.forked)}" aria-label="${escape(words.forked)}">${glyph("fork", 13)}</span></span>`
    : named;
  const stateCell = deleting
    ? `<span class="talk-state">${escape(state)}</span>`
    : `<span class="talk-state" title="${escape(state)}"><span class="mark${kind ? ` ${kind}` : ""}" aria-hidden="true"></span><span class="spoken">${escape(state)}</span></span>`;
  return `<li class="talk-item"${acts ? ` data-own-press data-machine="${escape(address.machine)}" data-resident="${escape(address.resident)}"` : ""}><a class="talk-row${kind ? ` ${kind}` : ""}${archived ? " archived" : ""}${current ? " current" : ""}" href="${link({ kind: "talk", address })}"${current ? ' aria-current="page"' : ""}>
    <span class="talk-host" title="${escape(agent)}">${host}</span>
    ${titled}
    ${stateCell}
    <span class="talk-device" title="${escape(device)}" aria-label="${escape(assisting !== null ? device : `${words.device(form)} ${r.about.machine}`)}">${glyph(assisting !== null ? "colleague" : form, 16)}</span>
  </a>${acts && !deleting ? quick(acts, archived, name) : ""}</li>`;
}

/** What shows at the row's end on hover: archiving it at once, or bringing
 *  it back, where its machine is there; and its "⋯". */
function quick(acts: RowActs, archived: boolean, name: string): string {
  const now = archived ? acts.restore : acts.archive;
  const act = acts.reachable
    ? `<button class="talk-quick-act" type="button" data-quick="${archived ? "restore" : "archive"}" aria-label="${escape(`${now} ${name}`)}" title="${escape(now)}">${glyph(archived ? "unarchive" : "archive", 16)}</button>`
    : "";
  return `<span class="talk-quick">${act}<button class="talk-more" type="button" aria-haspopup="menu" aria-expanded="false" aria-label="${escape(`${acts.more} ${name}`)}">${glyph("more", 18)}</button></span>`;
}
