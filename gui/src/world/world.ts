// The World as the front page reads it: the JSON forms of what `fastctx w`
// and `nodes` answer (`fastctx_types::town`, `::residents`, `::project`,
// `::world`). These are serialized replies, before any presentation model.

import type { Speaker, TurnEnd } from "./events";
import type { Address, AgentName, EntryId, JobId, JournalName, MachineName, ResidentId, Result, Timestamp, TownName } from "./wire";

export interface Host {
  agent: AgentName;
  client: string;
  version: string;
}

export type Origin = { kind: "user" } | { kind: "colleague"; asked_by: Address; label: string };

export interface About {
  id: ResidentId;
  host: Host;
  machine: MachineName;
  born_at: Timestamp;
  origin: Origin;
  session: string | null;
  world_started: boolean;
  model: string | null;
}

export type Activity = "busy" | "idle" | "asleep";

export interface Trouble {
  at: Timestamp;
  text: string;
}

export interface Now {
  activity: Activity;
  since: Timestamp;
  turn: string | null;
  owed: Record<string, Address[]>;
  answering: Address[];
  trouble: Trouble | null;
  desk: boolean;
  waiting_for_user: boolean;
}

export interface Action {
  at: Timestamp;
  machine: MachineName;
  hand: string;
  what: string;
  result: string;
}

export interface ResidentNow {
  about: About;
  town: TownName | null;
  here: boolean;
  /** When the user removed it from the World; its records are archived. */
  removed?: Timestamp | null;
  now: Now | null;
  last_action: Action | null;
  moved: Moved | null;
}

export interface Moved {
  to: TownName | null;
  at: Timestamp;
}

export interface RunningJob {
  job: JobId;
  machine: MachineName;
  owner: Address;
  started_at: Timestamp;
  pid: number | null;
  command: string;
  output: string;
}

export interface WhoReply {
  machine: MachineName;
  residents: ResidentNow[];
  jobs: RunningJob[];
}

export type MachineWho =
  | { kind: "answered"; reply: WhoReply }
  | { kind: "offline"; machine: MachineName; since: Timestamp | null }
  | { kind: "no_answer"; machine: MachineName; reason: string };

export type WhoScope =
  | { kind: "town"; town: TownName; record: ProjectRecord | null }
  | { kind: "world"; towns: TownSummary[] };

export interface WorldWho {
  machine: MachineName;
  scope: WhoScope;
  machines: MachineWho[];
}

export interface MachineFacts {
  name: MachineName;
  os: string;
  arch: string;
  home: string;
  temp: string;
  path_separator: string;
  case_sensitive: boolean;
  version: string;
}

export type Route =
  | { kind: "local" }
  | { kind: "card"; name: string }
  | { kind: "proxy"; proxy: string }
  | { kind: "system" };

export type LinkState =
  | { kind: "alone" }
  | { kind: "connecting"; since: Timestamp; last_error: string | null }
  | { kind: "online"; since: Timestamp; hub: string; route: Route }
  | { kind: "stopped"; reason: string };

export interface NodesReply {
  machine: MachineName;
  world: string | null;
  link: LinkState;
  machines: Node[];
}

export interface Node {
  name: MachineName;
  online: boolean | null;
  since: Timestamp | null;
  facts: Result<MachineFacts, string>;
  revoked: Revoked | null;
  /** A phone or another device: nothing runs on it, and it is not asked. */
  device?: boolean;
}

export interface Revoked {
  by: MachineName;
  at: Timestamp;
  reason: string;
}

export type Writer =
  | { kind: "user" }
  | { kind: "resident"; address: Address }
  | { kind: "session"; agent: AgentName };

export interface Location {
  machine: MachineName;
  path: string;
}

/** Listing.location is flattened by serde. */
export interface Listing extends Location {
  what: string;
}

/** fastctx_types::project::Record; named to avoid shadowing TS Record<K, V>. */
export interface ProjectRecord {
  version: number;
  name: string;
  what: string;
  machines: MachineName[];
  places: Listing[];
  canon: Location | null;
  by: Writer;
  on: MachineName;
  at: Timestamp;
}

export interface TownSummary {
  town: TownName;
  record: ProjectRecord;
  journals: number;
}

export interface Description {
  version: number;
  text: string;
  by: Writer;
  on: MachineName;
  at: Timestamp;
}

export interface Model {
  name: string;
  self_reported: boolean;
}

export type AttachmentKind = "text" | "image" | "pdf" | "binary";

export type Kept =
  | { kind: "in_entry" }
  | { kind: "on_machine"; location: Location };

export interface Attachment {
  name: string;
  kind: AttachmentKind;
  bytes: number;
  sha256: string;
  kept: Kept;
}

export interface EntryStamp {
  id: EntryId;
  at: Timestamp;
  machine: MachineName;
  by: Writer;
  agent: Host | null;
  model: Model | null;
  session: string | null;
  cites: EntryId[];
  attachments: Attachment[];
}

export interface JournalSummary {
  journal: JournalName;
  description: Description | null;
  entries: number;
  writers: number;
  latest: EntryStamp | null;
}

/** Who wrote a version of a record, on which machine, when. */
export interface Stamp {
  by: Writer;
  on: MachineName;
  at: Timestamp;
}

/** One document of a town's canon: its title and what it covers. */
export interface CanonHead {
  file: string;
  title: string;
  scope: string;
}

/** What a town's map is made of (`fastctx_types::project::MapView`). */
export interface MapView {
  town: TownName;
  ledger: string;
  record: ProjectRecord;
  opened: Stamp;
  /** Most recently added to first. */
  journals: JournalSummary[];
  /** The canon's documents, or why they could not be read. */
  canon: Result<CanonHead[], string> | null;
}

export interface EntryView {
  stamp: EntryStamp;
  text: string;
  /** Later entries that cite this one. */
  mentioned_by: EntryId[];
}

/** A part of a journal: the latest entries that fit, in order. */
export interface JournalView {
  town: TownName;
  journal: JournalName;
  directory: string;
  description: Description | null;
  /** Its position among all entries, from 1. */
  first: number;
  entries: EntryView[];
  total: number;
  writers: number;
}

/** A line of a colleague's messages (`fastctx_types::residents::MessageLine`). */
export type MessageLine = { at: Timestamp } & (
  | { kind: "asked"; from: Address; text: string }
  | { kind: "heard"; from: Speaker; turn: string | null; text: string }
  | { kind: "ended"; turn: string; end: TurnEnd; owed: Address[] }
  | { kind: "told"; to: Address; turn: string | null; outcome: { kind: string; why?: string | null } }
);

/** A job's record on the machine it runs on (`fastctx_types::run::JobRecord`),
 *  in the parts the interface shows. */
export interface JobRecord {
  id: JobId;
  machine: MachineName;
  owner: Address;
  town: TownName | null;
  script: string;
  cwd: string;
  output: string;
  started_at: Timestamp;
  pid: number | null;
  exit: { status: unknown; at: Timestamp } | null;
  output_ended_at: Timestamp | null;
  remaining: number | null;
  end: { at: Timestamp; how: unknown } | null;
}

/** One resident in detail, for `fastctx w <id>`. */
export interface ResidentDetail {
  resident: ResidentNow;
  records: string;
  actions: Action[];
  actions_total: number;
  asked: string | null;
  answers: MessageLine[];
  jobs: RunningJob[];
}

export type Looked = { kind: "resident" } & ResidentDetail | { kind: "job"; record: JobRecord };

/** One id looked up across the World. */
export interface LookedUp {
  id: string;
  found: Looked[];
  unasked: string[];
}
