// The World as the front page reads it: the JSON forms of what `fastctx w`
// and `nodes` answer (`fastctx_types::town`, `::residents`, `::project`,
// `::world`). These are serialized replies, before any presentation model.

import type { AccountName, Acting, HubId, HubName, Rights } from "./accounts";
import type { Speaker, TurnEnd } from "./events";
import type { PageSource, PageSummary } from "./pages";
import type { Focus } from "./focus";
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
  /** A Codex session the user opened on their own and brought in
   *  (`world-layers.md`, 接进来). */
  native_resumed?: boolean;
  model: string | null;
  /** Whom it works for: this computer's logins, or these accounts. */
  acting: Acting;
  /** The machines and folders the user points it at. */
  focus?: Focus;
  /** The MCP servers of the account it works for that it starts without
   *  (`world-user-config.md`). */
  mcp_off?: string[];
  /** The agent's home it works in, as its machine writes it, when not the
   *  default one there: which of that machine's logins it uses. */
  home?: string;
  /** The conversation it was forked from, even after that one is deleted. */
  forked_from?: Address;
  /** A side conversation, kept in its source's side panel rather than in the list. */
  side?: boolean;
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
  /** The latest turn that ended. */
  last_turn?: string;
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
  /** The start of the user's first words here: the conversation's name
   *  where its host gave it none. */
  first_words?: string;
  /** The start of the last message of the latest turn that ended: what a
   *  notification of that turn says. */
  last_said?: string;
  /** The latest turn that ended compacted the context and said nothing
   *  after it: what a notification of that turn says. */
  last_compacted?: boolean;
  /** Whether the latest turn that ended took words the user sent; absent
   *  from a node that does not say (`world-voice.md`). */
  last_heard_user?: boolean;
  /** What the agent asked the user and has not had answered. */
  asked?: Asked;
  /** What the agent last told the user through `message_user`
   *  (`world-voice.md`). */
  told?: Told;
}

/** Something the agent told the user through `message_user`. */
export interface Told {
  /** A new one is something new said. */
  id: string;
  at: Timestamp;
  /** An answer to the user's words, or the agent's own that can wait
   *  until the user looks, or that they should know soon. */
  status: "reply" | "info" | "urgent";
  /** Its start, as the agent wrote it. */
  text: string;
  /** The turn it was said in. */
  turn?: string;
}

/** Something the agent asked the user: a request its turn waits on, or a
 *  question asked without waiting. */
export interface Asked {
  /** A new one is a new question. */
  id: string;
  at: Timestamp;
  waits: boolean;
  what: "answer" | "command" | "change" | "tool" | "plan" | "form" | "other";
  /** Its questions, one a line; the command, files or tool it asks leave
   *  for; a form's message. */
  text: string;
  /** The answers offered to a single question. */
  options?: string[];
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
  /** Which computer of which hub answered for it, as `fastctx_types::observation::MachineSource`. */
  source?: MachineSource | null;
}

/** A machine in one network, and how this computer writes it: `name.hub`
 *  where another of its hubs has a different computer of that name. */
export interface MachineSource {
  hub: string;
  key: string;
  written: string;
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
  /** The account it runs for, when another machine sent it. */
  account?: { hub: string; account: string };
  source?: MachineSource | null;
}

export interface WhoReply {
  machine: MachineName;
  residents: ResidentNow[];
  jobs: RunningJob[];
  history?: import("./since").SinceCoverage & { events: import("./since").SinceEvent[]; undated: import("./since").UndatedFact[] };
  /** How many agents are at work or waiting on the machine, whosever they
   *  are: told to its owner alone (`world-gui.md`, 机器页). */
  busy?: number;
}

export type MachineWho = (
  | { kind: "answered"; reply: WhoReply }
  /** `last`: what was afoot on it as it last told, when that is known. */
  | { kind: "offline"; machine: MachineName; since: Timestamp | null; last?: WhoReply }
  | { kind: "no_answer"; machine: MachineName; reason: string }
) & { observation?: { source: MachineSource } | null };

export type WhoScope =
  | { kind: "town"; town: TownName; record: ProjectRecord | null }
  | { kind: "world"; towns: TownSummary[] };

export interface WorldWho {
  machine: MachineName;
  scope: WhoScope;
  you?: Address;
  history?: import("./since").WorldSince;
  machines: MachineWho[];
  /** The residents and jobs shown that work for none of the asker's accounts. */
  others?: WhoFor[];
}

/** A resident or a job, by its id, that works for other accounts than the
 *  asker's, with whom it works for. */
export interface WhoFor {
  machine: MachineName;
  id: string;
  accounts: string[];
}

export interface MachineFacts {
  control_letters?: boolean;
  run_stops?: boolean;
  node_lifetime?: {
    process: { pid: number; token: string };
    starter: { process: { pid: number; token: string }; session: Host | null } | null;
    supervised: boolean;
    windows_job: { kill_on_close: boolean; breakaway_allowed: boolean; silent_breakaway: boolean } | null;
  };
  shell: string;
  /** What kind of device it is; older nodes do not say. */
  form?: "desktop" | "laptop" | "server";
  /** It tells how it is doing (`machine_stats`); older nodes do not. */
  stats?: boolean;
  /** It keeps and runs an account's command MCP servers; older nodes do
   *  not. */
  mcp?: boolean;
  /** It lists what can be imported by name and kind alone, and reads a
   *  chosen server again to import it. */
  mcp_import?: boolean;
  /** It keeps what a page is agreed to do, and does it, for one account. */
  page_accounts?: boolean;
  /** It says where an attached file may be kept before any of it comes. */
  attachment_places?: boolean;
  /** It takes the attachments a script is sent with into its scratch. */
  attachment_runs?: boolean;
  /** It takes a colleague's question without being woken by it. */
  goal_questions?: boolean;
  /** It reads such a question carried on a colleague's words. */
  goal_question_words?: boolean;
  /** It opens a colleague's goal before its first input. */
  goal_colleagues?: boolean;
  /** It says which colleague goals a conversation still waits on. */
  goal_waits?: boolean;
  /** It takes goal commands from runs elsewhere, and a requester's pause
   *  and resume of its colleague. */
  goal_commands?: boolean;
  /** It takes a colleague's goal report and wakes a closed requester. */
  goal_reports?: boolean;
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
  /** `through`, the machine it reaches the hub through when its own line
   *  is down; `ms`, the round trip to the hub as last measured. */
  | { kind: "online"; since: Timestamp; hub: string; through?: string | null; route: Route; ms?: number | null }
  /** `dissolved`, the whole network was, not only this machine removed. */
  | { kind: "stopped"; reason: string; dissolved?: boolean };

/** How this computer's calls to a machine go: over a direct link between
 *  the two, or through the hub, with why there is no direct link when it
 *  allows direct connections (`fastctx_types::world::Path`). */
export type NodePath = { kind: "direct"; ms?: number } | { kind: "hub"; why?: string };

/** This computer, and hub by hub the machines its logins may use
 *  (`fastctx_types::world::NodesReply`). */
export interface NodesReply {
  machine: MachineName;
  /** What this computer is like; a device has nothing to say. */
  facts?: MachineFacts;
  hubs: HubNodes[];
}

export interface HubNodes {
  hub: HubId;
  name: HubName;
  link: LinkState;
  /** The accounts this computer is logged in to there. */
  logins: AccountName[];
  /** The account this computer is registered under there. */
  home: AccountName | null;
  /** What each login may do on this computer itself, as the hub's
   *  directory decides it; absent from a node before 1.0.1-beta.10. */
  here?: [AccountName, Rights][];
  machines: Node[];
}

export interface Node {
  name: MachineName;
  /** How it is written: `name.hub` where another hub has a different
   *  computer of that name. */
  written: string;
  online: boolean | null;
  since: Timestamp | null;
  /** The machine it reaches the hub through, when its own line is down. */
  through?: string | null;
  /** Whether the network's administrator allowed direct connections to it. */
  allows_direct?: boolean;
  /** How this computer's calls to it go now. */
  path?: NodePath;
  facts: Result<MachineFacts, string>;
  /** The account it is registered under. */
  owner: AccountName | null;
  /** The group its owner put it in (`world-network.md`, 机器的分组). */
  group?: string;
  /** What each of this computer's accounts may do on it. */
  rights: [AccountName, Rights][];
}

export type Writer =
  | { kind: "user" }
  | { kind: "resident"; address: Address }
  | { kind: "session"; agent: AgentName }
  /** An open page, doing what the user agreed to (`pages.ts`). */
  | { kind: "page"; page: PageSource }
  /** A conversation, or a page, on a machine this device's account may not
   *  work on: no more is told (`world-network.md`, 共享机器与智能体协助). */
  | { kind: "elsewhere"; page: boolean };

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
  /** Left out where whoever reads it may not work on the machine it was
   *  written on. */
  on?: MachineName;
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

/** Who wrote a version of a record, on which machine, when; the machine
 *  left out as a record's is. */
export interface Stamp {
  by: Writer;
  on?: MachineName;
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
  /** Where its colleagues live, when the user let it ask for them. */
  colleagues?: string;
  /** The Codex home they use there, when not the default one. */
  colleagues_home?: string;
}

export type Looked = { kind: "resident" } & ResidentDetail | { kind: "job"; record: JobRecord };

/** One id looked up across the World. */
export interface LookedUp {
  jobs?: RunningJob[];
  id: string;
  found: Looked[];
  unasked: string[];
}
