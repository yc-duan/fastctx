// What a project's page draws of the work in it (`world-gui.md`, 项目主页):
// each conversation the user began there as a route of its plan's stops,
// with the colleagues it asked, the senses it keeps and what it waits on the
// user for; the machines the work stands on as the ground beneath.

import { stops } from "../plan/flow";
import { readFlow } from "../plan/source";
import type { SentinelSummary } from "../world/sentinels";
import { ownerResident, type ResidentNow, type RunningJob } from "../world/world";

/** A stop on a conversation's way, as its plan's first flow walks it. */
export interface Station {
  label: string;
  where: "walked" | "here" | "ahead";
  /** Every way into it is dashed: only maybe. */
  maybe: boolean;
}

export interface HubSession {
  r: ResidentNow;
  key: string;
  /** The colleagues it asked, in the order they came. */
  colleagues: ResidentNow[];
  /** The senses it keeps. */
  senses: SentinelSummary[];
  /** Its plan's way, or none where it wrote no flow. */
  route: Station[] | null;
  /** What it asks the user, while it waits on them. */
  asked: string | null;
  /** What it told the user that this device has not shown them, unless it
   *  only answered them. */
  unseen: { status: "urgent" | "info"; text: string } | null;
  /** Every machine its work stands on: its own, its colleagues', where its
   *  senses watch and its jobs run. */
  machines: string[];
}

/** Something that waits for the user: a question a conversation asked, or
 *  what it told them that they have not seen. */
export interface HubNeed {
  kind: "asked" | "urgent" | "info";
  r: ResidentNow;
  text: string;
  at: string;
}

export interface HubPlace {
  machine: string;
  /** Conversations and colleagues that live on it. */
  agents: ResidentNow[];
  /** Senses that watch it. */
  senses: SentinelSummary[];
  jobs: RunningJob[];
}

export interface Hub {
  sessions: HubSession[];
  needs: HubNeed[];
  ground: HubPlace[];
}

export const keyOf = (r: ResidentNow): string => `${r.about.id}@${r.about.machine}`;

/** The plan's first flow, in the order it is walked, or none. */
export function routeOf(markdown: string | null): Station[] | null {
  if (!markdown) return null;
  const fence = /^```flow[^\n]*\n([\s\S]*?)^```/m.exec(markdown);
  if (!fence) return null;
  const walked = stops(readFlow(fence[1]!));
  if (!walked.length) return null;
  // A flow with no step marked is drawn as all ahead: nothing is walked yet.
  return walked.map((stop) => ({ label: stop.label, where: stop.where || "ahead", maybe: stop.maybe }));
}

const sameAddress = (a: { resident: string; machine: string }, r: ResidentNow) => a.resident === r.about.id && a.machine === r.about.machine;

/** The hub of a project from what its machines answered, the plans read
 *  for its conversations (keyed by `keyOf`), the senses of the accounts
 *  here, and what this device has seen of what was told. */
export function hubOf(
  residents: ResidentNow[],
  jobs: RunningJob[],
  machines: string[],
  plans: ReadonlyMap<string, string | null>,
  senses: SentinelSummary[],
  seenTold: (key: string) => string | null,
): Hub {
  const present = residents.filter((r) => !r.removed);
  // The conversations begun here, and any colleague whose asker is not here
  // to be drawn beside.
  const own = present.filter((r) => {
    const origin = r.about.origin;
    return origin.kind === "user" || !present.some((one) => sameAddress(origin.asked_by, one));
  });
  const sessions: HubSession[] = own.map((r) => {
    const colleagues = present.filter((one) => one.about.origin.kind === "colleague" && sameAddress(one.about.origin.asked_by, r));
    const kept = senses.filter((one) => one.phase !== "withdrawn" && sameAddress(one.definition.owner, r));
    const workers = [r, ...colleagues];
    const theirs = jobs.filter((job) => {
      const owner = ownerResident(job.owner);
      return !!owner && workers.some((one) => sameAddress(owner, one));
    });
    const told = r.now?.told;
    const asked = r.now?.asked?.text ?? (r.now?.waiting_for_user ? "" : null);
    return {
      r,
      key: keyOf(r),
      colleagues,
      senses: kept,
      route: routeOf(plans.get(keyOf(r)) ?? null),
      asked,
      unseen: told && told.status !== "reply" && seenTold(keyOf(r)) !== told.id ? { status: told.status, text: told.text } : null,
      machines: [...new Set([...workers.map((one) => one.about.machine), ...kept.map((one) => one.definition.home), ...theirs.map((job) => job.machine)])],
    };
  });
  // Those that wait for the user first, then those at work, then the rest by
  // how lately each was heard from.
  const rank = (one: HubSession) => (one.asked !== null ? 0 : one.r.now?.activity === "busy" ? 1 : 2);
  const lately = (one: HubSession) => Date.parse(one.r.now?.since ?? one.r.about.born_at);
  sessions.sort((a, b) => rank(a) - rank(b) || lately(b) - lately(a));

  const needs: HubNeed[] = [];
  for (const { r, asked, unseen } of sessions) {
    if (asked !== null) needs.push({ kind: "asked", r, text: asked, at: r.now?.asked?.at ?? r.now?.waiting_since ?? r.now?.since ?? r.about.born_at });
    if (unseen) needs.push({ kind: unseen.status, r, text: unseen.text, at: r.now?.told?.at ?? r.about.born_at });
  }
  const order = { asked: 0, urgent: 1, info: 2 } as const;
  needs.sort((a, b) => order[a.kind] - order[b.kind] || Date.parse(b.at) - Date.parse(a.at));

  // The project's own machines first, then any other its work stands on.
  const names: string[] = [...machines];
  for (const session of sessions) for (const machine of session.machines) if (!names.includes(machine)) names.push(machine);
  for (const job of jobs) if (!names.includes(job.machine)) names.push(job.machine);
  const agents = [...sessions.map((one) => one.r), ...sessions.flatMap((one) => one.colleagues)];
  const watching = sessions.flatMap((one) => one.senses);
  const ground = names.map((machine) => ({
    machine,
    agents: agents.filter((r) => r.about.machine === machine),
    senses: watching.filter((one) => one.definition.home === machine),
    jobs: jobs.filter((job) => job.machine === machine),
  }));
  return { sessions, needs, ground };
}
