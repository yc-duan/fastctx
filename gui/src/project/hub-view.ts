// The project page's picture of now (`world-gui.md`, 项目主页): each
// conversation begun there a line of its own, its mark and its name, its
// way as its plan walks it, and at the end the colleagues it asked, the
// senses it keeps and what waits for the user; beneath them, the machines
// the work stands on. Names are the only words; everything else is said
// where the pointer rests, or a click away.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { HomeWords } from "../home/words";
import type { Address } from "../world/events";
import { titleOf } from "../world/names";
import type { SentinelPhase, SentinelSummary } from "../world/sentinels";
import type { ResidentNow } from "../world/world";
import { groundDrawing, wayDrawing } from "./hub-draw";
import type { Hub, HubPlace, HubSession } from "./hub-model";
import type { ProjectWords } from "./words";

export interface HubViewOptions {
  words: ProjectWords;
  home: HomeWords;
  /** "Claude laptop". */
  someone(r: ResidentNow): string;
  link(address: Address): string;
  machineLink(machine: string): string;
  phase(phase: SentinelPhase): string;
  now: Date;
}

const markOf = (r: ResidentNow) => (r.about.host.agent === "codex" ? "codex" : "claude");
/** A state as the hub's classes name it. */
const stateOf = (r: ResidentNow) => (r.now?.activity === "busy" ? "ph-busy" : r.now?.activity === "asleep" || !r.now ? "ph-asleep" : "ph-idle");
const addressOf = (r: ResidentNow): Address => ({ resident: r.about.id, machine: r.about.machine });
const oneLine = (text: string) => text.replace(/\s+/g, " ").trim();

/** The most marks beside a conversation of each kind; the rest are a count. */
const BESIDE = 3;

/** How long it has been so, as a conversation's line says on hover. */
function lasted(r: ResidentNow, options: HubViewOptions): string {
  const { home, now } = options;
  const since = now.getTime() - Date.parse(r.now?.since ?? r.about.born_at);
  return r.now?.activity === "busy" ? home.for(home.lasted(since)) : home.ago(since);
}

/** The picture: a line for each conversation, then the ground. */
export function hubPicture(hub: Hub, seed: string, options: HubViewOptions): string {
  const rows = hub.sessions.length
    ? `<ul class="ph-sessions">${hub.sessions.map((session) => sessionRow(session, options)).join("")}</ul>`
    : `<p class="quiet ph-empty">${escape(options.words.nobodyHere)}</p>`;
  return `<div class="ph-picture">${rows}${groundStrip(hub, seed, options)}</div>`;
}

function sessionRow(session: HubSession, options: HubViewOptions): string {
  const { r } = session;
  const { words, home } = options;
  const here = session.route?.find((station) => station.where === "here");
  const said = session.asked !== null ? home.waitingFor : lasted(r, options);
  const title = [options.someone(r), here ? `${here.label}　${said}` : said].join("\n");
  const way = session.route
    ? `<span class="ph-way" title="${escape(words.way(session.route.map((station) => station.label)))}">${wayDrawing(session.route, session.key, session.asked !== null)}</span>`
    : `<span class="ph-way"></span>`;
  return `<li><a class="ph-row" href="${escape(options.link(addressOf(r)))}" title="${escape(title)}" data-machines="${escape(session.machines.join(" "))}">
    <span class="ph-mark ${stateOf(r)}">${glyph(markOf(r), 20)}</span>
    <span class="ph-name">${escape(titleOf(r) ?? options.someone(r))}</span>
    ${way}
    <span class="ph-company">${needMarks(session, options)}${company(session, options)}</span>
  </a></li>`;
}

/** What waits for the user, a mark each: a question in clay, something
 *  urgent a clay ※, something that can wait a drop of ink. */
function needMarks(session: HubSession, options: HubViewOptions): string {
  const { words } = options;
  const marks: string[] = [];
  if (session.asked !== null) {
    marks.push(`<span class="ph-need ph-need-ask" title="${escape([words.needAsked, oneLine(session.asked)].filter(Boolean).join("\n"))}">${glyph("ask", 16)}</span>`);
  }
  if (session.unseen?.status === "urgent") {
    marks.push(`<span class="ph-need ph-need-urgent" title="${escape(`${words.needUrgent}\n${oneLine(session.unseen.text)}`)}"><span aria-hidden="true">※</span></span>`);
  } else if (session.unseen) {
    marks.push(`<span class="ph-need ph-need-info" title="${escape(`${words.needInfo}\n${oneLine(session.unseen.text)}`)}"><span class="ph-drop"></span></span>`);
  }
  return marks.join("");
}

/** Its colleagues and its senses, a small mark each. */
function company(session: HubSession, options: HubViewOptions): string {
  const { words } = options;
  const colleagues = session.colleagues.slice(0, BESIDE).map((one) => {
    const label = one.about.origin.kind === "colleague" ? one.about.origin.label : "";
    return `<span class="ph-colleague ${stateOf(one)}" title="${escape(words.colleague(label, options.someone(one), lasted(one, options)))}">${glyph(markOf(one), 16)}</span>`;
  });
  const more = session.colleagues.length - BESIDE;
  const senses = session.senses.slice(0, BESIDE).map((one) => sense(one, options));
  const moreSenses = session.senses.length - BESIDE;
  return [
    ...colleagues,
    more > 0 ? `<span class="ph-more">+${more}</span>` : "",
    ...senses,
    moreSenses > 0 ? `<span class="ph-more">+${moreSenses}</span>` : "",
  ].join("");
}

function sense(one: SentinelSummary, options: HubViewOptions): string {
  const tone = one.phase === "satisfied" ? " found" : one.phase === "unable" || one.phase === "unavailable" ? " trouble" : "";
  const seen = one.latest?.text ?? one.reason ?? "";
  return `<span class="ph-sense${tone}" title="${escape(options.words.sense(one.definition.condition, one.definition.home, options.phase(one.phase), seen))}">${glyph("sentinel", 16)}</span>`;
}

/** The machines the work stands on, along one ruled line. */
function groundStrip(hub: Hub, seed: string, options: HubViewOptions): string {
  if (!hub.ground.length) return "";
  return `<div class="ph-ground"><div class="ph-ground-inner"><div class="ph-ground-line" aria-hidden="true">${groundDrawing(seed)}</div><ul class="ph-places">${hub.ground
    .map((place) => placeCell(place, options))
    .join("")}</ul></div></div>`;
}

/** The most marks above a machine; the rest are a count. */
const ABOVE = 4;

function placeCell(place: HubPlace, options: HubViewOptions): string {
  const all = [
    ...place.agents.map((r) => `<span class="ph-place-agent ${stateOf(r)}">${glyph(markOf(r), 14)}</span>`),
    ...place.senses.map(() => `<span class="ph-place-sense">${glyph("sentinel", 14)}</span>`),
    ...place.jobs.map(() => `<span class="ph-place-job"></span>`),
  ];
  const marks = all.slice(0, ABOVE).join("") + (all.length > ABOVE ? `<span class="ph-more">+${all.length - ABOVE}</span>` : "");
  return `<li><a class="ph-place" href="${escape(options.machineLink(place.machine))}" data-machine="${escape(place.machine)}" title="${escape(options.words.ground(place.agents.length, place.senses.length, place.jobs.length))}"><span class="ph-place-marks">${marks}</span><span class="ph-place-name">${escape(place.machine)}</span></a></li>`;
}
