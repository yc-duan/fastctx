// The one chosen on the machines page (`world-gui.md`, 账号、机器、共享与
// 协助): the bar beside it on the drawing, and what is known of a hub, a
// phone or the machine of another whose agent assists, opened when asked
// for. The bar holds the thing done to it most, written out, a few marks
// for the next, and "⋯" for the rest. Its name is on the paper beside its
// ring; the bar says it only where the drawing writes no names, or where
// the bar lies along a phone's bottom. A machine's details are its own
// page; the card of a hub, a phone or an assisting agent is a few facts,
// with the same acts at its foot.

import { glyph, type GlyphName } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { blot, circle, ruled, seeded } from "../pen/shapes";
import type { HubAccounts, Login, MemberSeen } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { LinkState } from "../world/world";
import { agentName } from "../world/names";
import type { AccessWords, MachinesWords } from "./account-words";
import type { MenuAct, MenuEntry } from "./acts-menu";
import { copyButton } from "./copy-button";
import type { PeopleWords } from "./people-words";
import { superOf, type Assistant, type Machine } from "./reach";

/** One thing on the drawing, as the page knows it. */
export type Spot =
  | { kind: "machine"; machine: Machine }
  | { kind: "assist"; assistants: Assistant[] }
  | { kind: "hub"; hub: HubAccounts }
  | { kind: "device"; hub: HubAccounts; member: MemberSeen; login: Login; owner: string | null; here: boolean };

/** What can be done to a hub or machine. */
export interface Act {
  act: string;
  label: string;
  icon: GlyphName;
  danger?: boolean;
  /** For a setting that is on or off, whether it is on. */
  on?: boolean;
}

/** A spot's acts as its bar holds them: the one done most, written out;
 *  the next few as marks; the rest behind "⋯". */
export interface SpotActs {
  lead: Act | null;
  marks: Act[];
  more: Act[];
}

/** The acts tried in turn for the one a bar writes out, and those it
 *  shows as marks beside it. */
const LEADS = ["talk", "details"];
const MARKS = ["agents", "share", "invite", "details"];

export function spotActs(acts: Act[]): SpotActs {
  const lead = LEADS.map((name) => acts.find((act) => act.act === name)).find((act) => !!act) ?? null;
  const marks = acts.filter((act) => act !== lead && MARKS.includes(act.act));
  const more = acts.filter((act) => act !== lead && !marks.includes(act));
  return { lead, marks, more };
}

/** What lies behind a spot's "⋯", as its menu lists it: a setting with its
 *  switch, and what cannot be taken back last, under a rule. */
export function moreEntries(more: Act[], words: MachinesWords, people: PeopleWords): MenuEntry[] {
  const entries: MenuEntry[] = [];
  for (const act of more) {
    const last = entries.at(-1);
    if (act.danger && last && "act" in last && !last.danger) entries.push({ rule: true });
    const entry: MenuAct = { act: act.act, icon: act.icon, label: settingOf(act, words, people) };
    if (act.danger) entry.danger = true;
    if (act.on !== undefined) entry.on = act.on;
    entries.push(entry);
  }
  return entries;
}

/** A setting is named for what it allows, whether it is on or off: its
 *  switch says which. */
function settingOf(act: Act, words: MachinesWords, people: PeopleWords): string {
  if (act.act === "direct") return words.allowDirect;
  if (act.act === "signup") return people.signUp;
  return act.label;
}

/** What a spot's bar and card ask of the page. */
export interface SpotHost {
  words: MachinesWords;
  access: AccessWords;
  /** When, as the front page says it. */
  when(at: Date, now: Date): string;
  /** Moving the hub elsewhere, while it is under way. */
  moving(hub: string, here: string | null): string;
}

export class SpotViews {
  constructor(private readonly host: SpotHost) {}

  /** The bar of `spot`, chosen as `key`: `named`, it says the name first;
   *  `note`, what last went wrong with it. */
  bar(spot: Spot, key: string, acts: SpotActs, named: boolean, note: { said: string; detail: string } | null): string {
    const { words } = this.host;
    const button = (act: Act, lead: boolean) => {
      const primary = lead && act.act === "talk";
      const named = lead ? "" : ` aria-label="${escape(act.label)}" title="${escape(act.label)}"`;
      return `<button class="spot-act${lead ? " lead" : ""}${primary ? " primary" : ""}" type="button" data-map-act="${act.act}" data-key="${escape(key)}"${named}>${glyph(act.icon, 18)}<span class="spot-label">${escape(act.label)}</span></button>`;
    };
    const more = acts.more.length
      ? `<button class="spot-act spot-more" type="button" data-spot-more="${escape(key)}" aria-haspopup="menu" aria-expanded="false" aria-label="${escape(words.moreActs)}" title="${escape(words.moreActs)}">${glyph("more", 18)}<span class="spot-label">${escape(words.moreActs)}</span></button>`
      : "";
    const count = (acts.lead ? 1 : 0) + acts.marks.length + (more ? 1 : 0);
    const name = named ? `<span class="spot-name${awayOf(spot) ? " away" : ""}">${escape(this.titleOf(spot))}</span>` : "";
    return `<div class="spot-head">${name}<button class="icon-button spot-close" type="button" data-spot="close" aria-label="${escape(words.closeSpot)}" title="${escape(words.closeSpot)}">${glyph("close", 16)}</button></div>
      <div class="spot-acts" style="--acts:${count}">${acts.lead ? button(acts.lead, true) : ""}${acts.marks.map((act) => button(act, false)).join("")}${more}</div>
      ${note ? `<p class="spot-note" title="${escape(note.detail)}">${escape(note.said)}</p>` : ""}`;
  }

  /** What is known of a hub, a phone, or the machine of another whose agent
   *  assists, with what can be done to it at the foot. */
  card(spot: Spot, key: string, world: WorldNow, now: Date, acts: SpotActs, note: { said: string; detail: string } | null): string {
    const body = spot.kind === "hub" ? this.hub(spot.hub, world) : spot.kind === "assist" ? this.assist(spot.assistants, now) : spot.kind === "device" ? this.phone(spot.member, now) : "";
    const mark = spot.kind === "hub" ? `<span class="spot-knot${spot.hub.link.kind === "online" ? " online" : ""}" aria-hidden="true"></span>` : spot.kind === "device" ? glyph("phone", 18) : "";
    const tag = spot.kind === "device" && spot.owner ? `<span class="tag">${escape(this.host.words.ownedBy(spot.owner))}</span>` : "";
    return `<article class="spot-card">
      <header class="spot-card-head">${mark}<h2 class="spot-card-title${awayOf(spot) ? " away" : ""}">${escape(this.titleOf(spot))}</h2>${tag}</header>
      ${body}
      ${note ? `<p class="spot-note" title="${escape(note.detail)}">${escape(note.said)}</p>` : ""}
      ${this.foot(key, acts)}
    </article>`;
  }

  /** What a spot is called on its bar and card: as the drawing names it,
   *  an assisting agent's machine by whose agents they are. */
  private titleOf(spot: Spot): string {
    return spot.kind === "assist" ? this.host.words.assistAgents(spot.assistants[0]!.owner ?? "") : nameOf(spot);
  }

  /** A card's acts: those its bar shows, written out, but the details it
   *  is; "⋯" for the rest, or the one left, written out at the end. */
  private foot(key: string, acts: SpotActs): string {
    const { words } = this.host;
    const shown = [acts.lead, ...acts.marks].filter((act): act is Act => !!act && act.act !== "details");
    if (!shown.length && !acts.more.length) return "";
    const button = (act: Act, last = false) =>
      `<button class="spot-card-act${act.act === "talk" ? " primary" : ""}${act.danger ? " danger" : ""}${last ? " last" : ""}" type="button" data-map-act="${act.act}" data-key="${escape(key)}">${glyph(act.icon, 16)}<span>${escape(act.label)}</span></button>`;
    const [only] = acts.more;
    const more =
      acts.more.length === 1 && only && only.on === undefined
        ? button(only, true)
        : acts.more.length
          ? `<button class="icon-button spot-card-more" type="button" data-spot-more="${escape(key)}" aria-haspopup="menu" aria-expanded="false" aria-label="${escape(words.moreActs)}" title="${escape(words.moreActs)}">${glyph("more", 18)}</button>`
          : "";
    return `<footer class="spot-card-acts">${shown.map((act) => button(act)).join("")}${more}</footer>`;
  }

  /** A hub: a small drawing and a line of what it is, then where it is,
   *  how this computer reaches it, how many machines it holds and who
   *  administers it; and its move, while one is under way. */
  private hub(hub: HubAccounts, world: WorldNow): string {
    const { words } = this.host;
    const logins = loginsOf(world.accounts).filter((login) => login.hub === hub.hub);
    const admin = logins.map((login) => superOf(world, login)).find((name) => !!name) ?? null;
    const machines = hub.members.filter((member) => member.kind === "computer" && member.home).length;
    const why = linkWhy(hub.link);
    const addresses = hub.addresses.map((address) => `<span class="spot-copy"><span>${escape(address)}</span>${copyButton(address, words.spot.copy)}</span>`).join("");
    const rows = [
      addresses ? [words.spot.address, addresses, ""] : null,
      [words.spot.link, escape(linkShort(hub.link, words)), why],
      [words.spot.machines, escape(words.groupCount(machines)), ""],
      admin ? [words.spot.admin, escape(admin), ""] : null,
    ].filter((row): row is string[] => !!row);
    return `<div class="spot-what">${hubPicture()}<p>${escape(words.hubIs)}</p></div>
      <dl class="spot-facts">${rows.map(([name, value, title]) => `<div><dt>${escape(name!)}</dt><dd${title ? ` title="${escape(title)}"` : ""}>${value}</dd></div>`).join("")}</dl>
      ${this.host.moving(hub.hub, world.nodes.machine ?? null)}`;
  }

  /** The agents of another's assisting the account on a machine of theirs:
   *  each with until when. */
  private assist(assistants: Assistant[], now: Date): string {
    const { access } = this.host;
    return `<ul class="spot-agents">${assistants
      .map((one) => {
        const until = one.until ? new Date(one.until) : null;
        const said = until ? access.moment.until(until, now) : access.when.none;
        return `<li title="${escape(said)}">${glyph(one.agent === "codex" ? "codex" : "claude", 16)}<span>${escape(agentName(one.agent))}</span><span class="spot-until">${escape(until ? access.moment.short(until, now) : access.when.none)}</span></li>`;
      })
      .join("")}</ul>`;
  }

  /** A phone signed in to the account: since when, and whether it is
   *  there. */
  private phone(member: MemberSeen, now: Date): string {
    const { words, when } = this.host;
    const since = member.since ? when(new Date(member.since), now) : null;
    const state = member.online ? words.onlineNow : words.offline(since);
    return `<dl class="spot-facts"><div><dt>${escape(words.spot.signedIn)}</dt><dd>${escape(when(new Date(member.added), now))}</dd></div><div><dt>${escape(words.spot.state)}</dt><dd>${escape(state)}</dd></div></dl>`;
  }
}

/** The name a spot goes by: its machine's, hub's or phone's. */
export function nameOf(spot: Spot): string {
  if (spot.kind === "machine") return spot.machine.written;
  if (spot.kind === "assist") return spot.assistants[0]!.written;
  if (spot.kind === "hub") return spot.hub.name;
  return spot.member.name;
}

/** Whether a spot is away now: its name goes grey. */
function awayOf(spot: Spot): boolean {
  if (spot.kind === "machine") return !spot.machine.here && spot.machine.online === false;
  if (spot.kind === "assist") return spot.assistants[0]!.online === false;
  if (spot.kind === "hub") return spot.hub.link.kind !== "online";
  return !spot.here && !spot.member.online;
}

/** How this computer reaches a hub, in a word or two: the round trip, and
 *  through which machine when its own line is down. */
function linkShort(link: LinkState, words: MachinesWords): string {
  if (link.kind === "online") return [link.through ? words.viaShort(link.through) : "", link.ms !== undefined && link.ms !== null ? words.listMs(link.ms) : words.hubOnline].filter(Boolean).join(" ");
  if (link.kind === "connecting") return words.hubConnecting;
  if (link.kind === "stopped") return link.dissolved ? words.linkDissolved : words.hubOff;
  return words.hubOff;
}

/** A hub drawn small: its knot, the line along the top, and three machines
 *  hung from it, this computer's ring with a drop of clay in it. */
function hubPicture(): string {
  const random = seeded("spot-hub-picture");
  const knot: [number, number] = [9, 8];
  const rings: [number, number][] = [
    [9, 31],
    [36, 31],
    [63, 31],
  ];
  const lines = [ruled([13, 8], [63, 8], 1.1, random), ruled([9, 12], [9, 24], 1.1, random), ruled([36, 8], [36, 24], 1.1, random), ruled([63, 8], [63, 24], 1.1, random)];
  const ink = [circle(knot, 3.6, 1.3, random), blot(knot, 1.6, random), ...lines, ...rings.map((at) => circle(at, 6.5, 1.3, random))];
  return `<svg class="spot-picture" viewBox="0 0 72 40" aria-hidden="true"><g class="graphite" filter="url(#pen-marker)">${ink.map((d) => `<path d="${d}"/>`).join("")}</g><g class="clay" filter="url(#pen-marker)"><path d="${blot(rings[0]!, 2.2, random)}"/></g></svg>`;
}

/** Why a hub is not reached, in the kernel's words, for the hover. */
export function linkWhy(link: LinkState): string {
  return link.kind === "connecting" ? (link.last_error ?? "") : link.kind === "stopped" ? link.reason : "";
}

/** How this computer reaches a hub, in words. */
export function linkSaid(link: LinkState, words: MachinesWords): string {
  switch (link.kind) {
    case "online":
      return link.through ? words.linkVia(link.through, link.hub) : words.linkOnline(link.hub);
    case "connecting":
      return words.linkConnecting(link.last_error);
    case "stopped":
      return link.dissolved ? words.linkDissolved : words.linkStopped(link.reason);
    case "alone":
      return "";
  }
}
