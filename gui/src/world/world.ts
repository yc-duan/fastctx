// The World as the front page reads it: the JSON forms of what `fastctx w`
// and `nodes` answer (`fastctx_types::town`, `::residents`, `::project`,
// `::world`). These are serialized replies, before any presentation model.

import type { Speaker, TurnEnd } from "./events";
import type { PageSource, PageSummary } from "./pages";
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
  desk_since?: Timestamp | null;
  waiting_for_user: boolean;
  waiting_since?: Timestamp | null;
  /** The conversation's name, as its host last gave it. */
  title: string | null;
  last_tool_at?: Timestamp | null;
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
  tools?: { turn: string; id: string; started_at: Timestamp | null; name: string; machine: MachineName | null; detail: string }[];
  moved: Moved | null;
}

export interface Moved {
  to: TownName | null;
  at: Timestamp;
}

export interface RunningJob {
  job: JobId;
  town?: TownName | null;
  machine: MachineName;
  owner: JobOwner;
  started_at: Timestamp;
  pid: number | null;
  command: string;
  output: string;
  stderr?: string;
}

export interface WhoReply {
  machine: MachineName;
  residents: ResidentNow[];
  jobs: RunningJob[];
  history?: import("./since").SinceCoverage & { events: import("./since").SinceEvent[]; undated: import("./since").UndatedFact[] };
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
  you?: Address;
  history?: import("./since").WorldSince;
  machines: MachineWho[];
}

export interface MachineFacts {
  node_lifetime?: {
    process: { pid: number; token: string };
    starter: { process: { pid: number; token: string }; session: Host | null } | null;
    supervised: boolean;
    windows_job: { kill_on_close: boolean; breakaway_allowed: boolean; silent_breakaway: boolean } | null;
  };
  shell: string;
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
  | { kind: "online"; since: Timestamp; hub: string; relay?: MachineName | null; route: Route }
  | { kind: "stopped"; reason: string };

export interface NodesReply {
  machine: MachineName;
  world: string | null;
  link: LinkState;
  machines: Node[];
  /** Members that came in with this machine's invitations, not yet shown
   *  to the user. */
  joined?: Joined[];
}

export interface Node {
  name: MachineName;
  online: boolean | null;
  since: Timestamp | null;
  facts: Result<MachineFacts, string>;
  revoked: Revoked | null;
  /** A phone or another device: nothing runs on it, and it is not asked. */
  device?: boolean;
  /** What it may do; absent when it is revoked or the World is one machine. */
  standing?: Standing | null;
  /** Where it hangs in the World's tree; absent as `standing` is. */
  place?: Place | null;
}

/** One upstream a machine connects to (`fastctx_types::world::Upstream`). */
export type Upstream = { kind: "hub" } | { kind: "relay"; machine: MachineName };

/** Where a member hangs in the tree (`fastctx_types::world::Place`). */
export interface Place {
  /** The upstreams it connects through, in order; the hub is the last of any. */
  via: Upstream[];
  /** Where machines below it dial it, when it relays. */
  relays_at?: string[];
}

/** Everything, or only these (`fastctx_types::scope::Set`). */
export type ScopeSet<T> = "all" | T[];

/** What a member does on a machine, named after the hand that does it. */
export type ScopeAction = "run" | "write" | "copy" | "ask_colleague" | "w" | "talk" | "read" | "consent" | "page";

export const scopeActions: ScopeAction[] = ["talk", "w", "read", "run", "write", "copy", "ask_colleague", "page", "consent"];

/** What one member may use and do (`fastctx_types::scope::Scope`), projects
 *  by their short names; one this machine cannot name, by its id. */
export interface Scope {
  machines: ScopeSet<MachineName>;
  agents: ScopeSet<AgentName>;
  projects: ScopeSet<string>;
  actions: ScopeSet<ScopeAction>;
  invite: boolean;
}

/** Where a member stands (`fastctx_types::world::Standing`). */
export interface Standing {
  scope: Scope;
  manage?: boolean;
  /** Who invited it; null for the machine that founded the World. */
  invited_by: MachineName | null;
  /** The branch of an inviter without management rights. */
  delegated?: boolean;
  /** Whether it has a key others can seal to yet. */
  sealing: boolean;
}

/** A member that came in with this machine's invitation. */
export interface Joined {
  name: MachineName;
  device?: boolean;
  noticed_at: Timestamp;
}

export interface Revoked {
  by: MachineName;
  at: Timestamp;
  reason: string;
}

export type Writer =
  | { kind: "user" }
  | { kind: "resident"; address: Address }
  | { kind: "session"; agent: AgentName }
  /** An open page, doing what the user agreed to (`pages.ts`). */
  | { kind: "page"; page: PageSource };

/** Who started a job: a resident, or an open page. A resident is its
 *  address as it always was. */
export type JobOwner = Address | { page: PageSource };

/** The resident that started a job, where a resident did. */
export function ownerResident(owner: JobOwner): Address | null {
  return "page" in owner ? null : owner;
}

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
  /** The town's pages, each at its current version. */
  pages: PageSummary[];
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
  owner: JobOwner;
  town: TownName | null;
  script: string;
  cwd: string;
  output: string;
  stderr?: string;
  started_at: Timestamp;
  pid: number | null;
  exit: { status: ExitStatus; at: Timestamp } | null;
  output_ended_at: Timestamp | null;
  output_drained?: { stdout: number; stderr: number };
  remaining: number | null;
  end: { at: Timestamp; how: EndedHow } | null;
}

/** How a program ended (`fastctx_types::run::ExitStatus`). */
export type ExitStatus = { code: number } | { signal: number };

/** How a job's supervisor accounted for its end (`fastctx_types::run::EndedHow`). */
export type EndedHow =
  | { kind: "finished" }
  | { kind: "stopped" }
  | { kind: "interrupted"; reason: string }
  | { kind: "failed"; error: string };

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
  jobs?: RunningJob[];
  id: string;
  found: Looked[];
  unasked: string[];
}
