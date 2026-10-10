// What the World answered to "what happened after this moment", read as
// the front page's chronicle. Each line is a fact a machine's records hold;
// what a machine could not be asked is said, never left out.

import type { MachineName } from "../world/wire";
import type { EndedHow, ExitStatus, SinceEvent, SinceTown, WorldSince } from "../world/since";
import type { Happening, Unasked } from "./model";

export function happeningsOf(answer: WorldSince): { happenings: Happening[]; unasked: Unasked[] } {
  const happenings = answer.events.flatMap((event) => happening(event));
  const unasked: Unasked[] = answer.machines.flatMap((machine): Unasked[] => {
    switch (machine.kind) {
      case "answered":
        return machine.reply.issues.length ? [{ machine: machine.reply.machine, offline: false, why: machine.reply.issues.map((issue) => issue.reason).join("; ") }] : [];
      case "offline":
        return [{ machine: machine.machine, offline: true, why: null }];
      case "no_answer":
        return [{ machine: machine.machine, offline: false, why: machine.reason }];
    }
  });
  return { happenings, unasked };
}

function townOf(town: SinceTown): string | null {
  return town.kind === "known" ? town.town : null;
}

function happening(event: SinceEvent): Happening[] {
  const at = event.at;
  const town = townOf(event.town);
  switch (event.kind) {
    case "file_action":
      return [{ kind: "file_action", at, who: event.resident, town, hand: event.hand, paths: event.paths, result: event.result }];
    case "turn_ended":
      return [{ kind: "turn_ended", at, who: event.resident, town, how: event.end.how, reason: event.end.reason, said: event.end.last_message }];
    case "job_ended":
      return [{ kind: "job_ended", at, job: event.job, machine: event.machine, owner: event.owner, town, how: jobHow(event.exit?.status ?? null, event.how) }];
    case "colleague_asked":
      return [{ kind: "asked", at, who: event.asker, colleague: event.colleague, town, label: event.label }];
    case "resident_born":
      // A colleague's birth is the asking, said once.
      return event.origin.kind === "user" ? [{ kind: "arrived", at, who: event.resident, town, agent: event.host.agent }] : [];
    case "resident_moved":
      return [{ kind: "moved", at, who: event.resident, town, from: event.from, to: event.to }];
    case "project_version":
      return town ? [{ kind: "project", at, town, version: event.version, name: event.name, by: event.by }] : [];
    case "submission_uncertain":
      return [{ kind: "uncertain", at, who: event.resident, town, why: event.why }];
    case "answer_delivery":
      return event.outcome.kind === "failed"
        ? [{ kind: "undelivered", at, who: event.colleague, to: event.to, town, why: event.outcome.reason }]
        : [];
    case "machine_connected":
    case "machine_offline":
      return [{ kind: "machine", at, machine: event.machine as MachineName, online: event.kind === "machine_connected" }];
    // Kept by the records but not something that happened to the reader's
    // work: a session registering again. Journals are project memory, not
    // shown until it is finished (`world-journals.md`).
    case "journal_entry":
    case "journal_description":
    case "claude_registered":
      return [];
  }
}

/** A job's end in the World's own wording, as `run` says it. */
function jobHow(status: ExitStatus | null, how: EndedHow): string {
  if (status) return "code" in status ? `exited ${status.code}` : `ended by signal ${status.signal}`;
  switch (how.kind) {
    case "finished":
      return "finished";
    case "stopped":
      return "stopped";
    case "interrupted":
      return `interrupted: ${how.reason}`;
    case "failed":
      return `could not run: ${how.error}`;
  }
}
