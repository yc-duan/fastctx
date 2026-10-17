// What happened in the World after a moment, as each machine's records say
// when asked: the JSON form of `fastctx_types::since::WorldSince`. Times are
// those the records kept; facts with no time come apart from the events.

import type { TurnEnd } from "./events";
import type { Address, JobId, JournalName, MachineName, NoticeId, Timestamp, TownName } from "./wire";
import type { Host, JobOwner, Location, Origin, Writer } from "./world";
import type { HubAccount } from "./accounts";

export interface WorldSince {
  machine: MachineName;
  since: Timestamp;
  town: TownName | null;
  machines: MachineSince[];
  events: SinceEvent[];
  undated: UndatedFact[];
  issues: SinceIssue[];
  limitations: string[];
}

export type MachineSince =
  | { kind: "answered"; reply: SinceCoverage }
  | { kind: "offline"; machine: MachineName; since: Timestamp | null }
  | { kind: "no_answer"; machine: MachineName; reason: string };

export interface SinceCoverage {
  machine: MachineName;
  read_started_at: Timestamp;
  read_finished_at: Timestamp;
  issues: SinceIssue[];
}

export interface SinceIssue {
  source: string;
  reason: string;
}

export type SinceTown = { kind: "known"; town: TownName | null } | { kind: "unrecorded"; recorded_in: (TownName | null)[] };

export type ExitStatus = { code: number } | { signal: number };

export type EndedHow =
  | { kind: "finished" }
  | { kind: "stopped" }
  | { kind: "interrupted"; reason: string }
  | { kind: "failed"; error: string };

export type Outcome = { kind: "delivered" } | { kind: "waiting"; why: string | null } | { kind: "sent" } | { kind: "failed"; reason: string };

export type SinceKind =
  | { kind: "file_action"; resident: Address; hand: string; paths: string; result: string }
  | { kind: "resident_born"; resident: Address; host: Host; origin: Origin }
  | { kind: "turn_ended"; resident: Address; turn: string; end: TurnEnd; readers: Address[] | null }
  | {
      kind: "job_ended";
      job: JobId;
      owner: JobOwner;
      started_at: Timestamp;
      elapsed_ms: number;
      how: EndedHow;
      exit: { status: ExitStatus; at: Timestamp } | null;
      script: string;
      output: string;
      /** The account a job run here for a resident of another machine
       *  works for. */
      account?: HubAccount;
    }
  | { kind: "colleague_asked"; asker: Address; colleague: Address; label: string }
  | { kind: "answer_delivery"; colleague: Address; to: Address; turn: string | null; outcome: Outcome }
  | { kind: "resident_moved"; resident: Address; from: TownName | null; to: TownName | null }
  | { kind: "claude_registered"; resident: Address; session: string; instance: string | null }
  | { kind: "submission_uncertain"; resident: Address; delivery: NoticeId; why: string; end: TurnEnd | null; readers: Address[] | null }
  | { kind: "journal_entry"; journal: JournalName; entry: string; by: Writer }
  | { kind: "journal_description"; journal: JournalName; version: number; by: Writer }
  | { kind: "project_version"; version: number; name: string; by: Writer }
  | { kind: "machine_connected" }
  | { kind: "machine_offline" };

export type SinceEvent = {
  id: string;
  at: Timestamp;
  machine: MachineName;
  town: SinceTown;
  sources: Location[];
} & SinceKind;

export type UndatedFact = {
  resident: Address;
  town: SinceTown;
  source: Location;
} & (
  | { kind: "asleep" }
  | { kind: "claude_closed"; session: string; instance: string }
  | { kind: "claude_registered"; session: string; instance: string | null }
  | { kind: "submission_uncertain"; delivery: NoticeId; why: string; end: TurnEnd | null; readers: Address[] | null }
);
