import type { UsageCallsQuery, UsageCallsPage } from "./usage-calls";
// The way to the World from inside the shell: each function is one of the
// shell's commands (`src-tauri/src/lib.rs`), which is one call of the
// core's client. Outside the shell there is no World to reach, and
// `inShell` says so.

import { Channel, invoke } from "@tauri-apps/api/core";
import { closeDelivery, observe, retryDeliveries } from "./delivery";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { HomeChange, WorldAgents } from "./agents";
import type { ChangePreview, FolderChange, LoginChange, MachineLogins, WorldLogins } from "./logins";
import type { Control, Controlled, Offers } from "./control";
import type { WorldSince } from "./since";
import type { FileRange } from "./file-read";
import type { HeldRead } from "./reads";
import type { Attachment, FilePlace, SentUploadTarget, UploadTarget } from "./attachments";
import type { ConnectionState } from "./device";
import type { Focus } from "./focus";
import type { BytesRead, TextRead } from "../app/reader";
import type { Address, AgentActivity, MachineName, SessionEvent } from "./events";
import type { Cut, Edge, FindDoc, Found, PartRead, Window } from "../conversation/conversation";
import type { AccountChange, AccountsReply, HubAccount, TokenPeek } from "./accounts";
import type { UsageReport } from "./usage";
import type { HubMoveReply, HubMoveRequest } from "./hub-move";
import type { MachineStats, StatsSeen } from "./stats";
import type { Looked, RemoteHub, RemoteKey, RemoteServer, RemoteStep } from "../app/remote-world";
import type { LookedUp, MapView, NodesReply, ProjectRecord, Stamp, TownSummary, WorldWho } from "./world";

/** A development build asked for its specimen (`?specimen`) draws it even
 *  inside the app's shell, so a phone's own WebView can be tried with real
 *  fingers; a release build has no such door. */
const specimenAsked = import.meta.env.DEV && new URLSearchParams(location.search).has("specimen");

/** Whether the page runs in the app's shell, with the World behind it. */
export function inShell(): boolean {
  return "__TAURI_INTERNALS__" in window && !specimenAsked;
}

/** In the shell on a computer, where this machine can be set up; a phone
 *  carries none of that. */
export function onComputer(): boolean {
  return inShell() && !/Android|iPhone|iPad/i.test(navigator.userAgent);
}

/** Opens a web link in the system's browser, outside the app. */
export async function openOutside(url: string): Promise<void> {
  if (!/^https?:\/\//i.test(url)) throw new Error(`not a web link: ${url}`);
  if (inShell()) await openUrl(url);
  else window.open(url, "_blank", "noopener");
}

export interface WorldNow {
  here: MachineName;
  who: WorldWho;
  nodes: NodesReply;
  towns: TownSummary[];
  /** The accounts this computer or device is logged in to, hub by hub,
   *  with what each sees. */
  accounts: AccountsReply;
}

/** A piece of a watched conversation: the page holds a part of it near its
 *  reader, the shell the rest (`src-tauri/src/transcript.rs`). */
export type Piece =
  | { kind: "resumed" }
  /** Events after all that was given, from `start` on. */
  | { kind: "extended"; start: number; events: SessionEvent[] }
  /** One thing as it happened, the `at`th event of the conversation. */
  | { kind: "event"; at: number; event: SessionEvent }
  | { kind: "ended"; error: string | null }
  /** The conversation was deleted: nothing more of it comes. */
  | { kind: "deleted"; error: string }
  /** Its machine refused this account: nothing more comes until it may. */
  | { kind: "refused"; error: string }
  /** Whether it works now, on which turn, and which words its machine
   *  keeps on their way in. */
  | { kind: "now"; busy: boolean; turn: string | null; kept?: import("../conversation/model").KeptWords[] }
  /** The part the page holds is now this, in place of all it held. */
  | { kind: "window"; window: Window }
  /** What came before arrived: what the page holds stays, moved along. */
  | { kind: "rebased"; generation: number; offset: number; turns_added: number }
  /** The stream broke and is being read again: why, and since when
   *  (milliseconds since the epoch). */
  | { kind: "interrupted"; error: string | null; since: number };

export type Landed =
  | { kind: "uncertain"; why: string }
  | { kind: "new_turn"; turn: string }
  | { kind: "joined_turn"; turn: string }
  | { kind: "waiting"; why: string | null; answered: boolean }
  | { kind: "sent" }
  | { kind: "handed"; id: string };

export function worldNow(): Promise<WorldNow> {
  return invoke<WorldNow>("world_now");
}

export type WorldPiece =
  | { kind: "world"; world: WorldNow }
  /** What changed since the piece before (`world/change.ts`). */
  | { kind: "changed"; change: import("./change").Change }
  | { kind: "interrupted"; error: string; since: number };

/** Follows the World: first as it is, then after each change, nothing
 *  asked of any machine meanwhile (`world-network.md`, 近况). Resolves to
 *  the id `unwatch` takes. */
export function watchWorld(onPiece: (piece: WorldPiece) => void): Promise<number> {
  return observe<WorldPiece, number>("world_watch", {}, onPiece);
}

/** Watches a resident: the part it opens on, then each piece as it
 *  comes. */
export async function watch(
  who: Address,
  onPiece: (piece: Piece) => void,
): Promise<{ id: number; window: Window }> {
  return observe<Piece, { id: number; window: Window }>("watch", { residentId: who.resident, machineName: who.machine }, onPiece);
}

export function unwatch(id: number): Promise<void> {
  closeDelivery(id);
  return invoke("unwatch", { id });
}

/** A part of a watched conversation the page does not hold; null when the
 *  page asked about an earlier reading or there is nothing there. */
export function conversationPart(id: number, generation: number, edge: Edge): Promise<PartRead | null> {
  return invoke("conversation_part", { id, generation, edge });
}

/** Where a turn of a watched conversation first appears. */
export function conversationLocate(id: number, generation: number, turn: string): Promise<number | null> {
  return invoke("conversation_locate", { id, generation, turn });
}

/** Where the page can let go of what comes before its latest `events`. */
export function conversationCut(id: number, generation: number, events: number): Promise<Cut | null> {
  return invoke("conversation_cut", { id, generation, events });
}

/** Hands the shell the displayed text of a part, for finding. */
export function conversationIndex(id: number, generation: number, start: number, end: number, docs: FindDoc[]): Promise<boolean> {
  return invoke("conversation_index", { id, generation, start, end, docs });
}

/** Counts words in what the page does not hold and the shell has the text
 *  of, and says what it has not. */
export function conversationFind(id: number, generation: number, query: string, held: [number, number][]): Promise<Found | null> {
  return invoke("conversation_find", { id, generation, query, held });
}

/** What this device keeps of the conversations it viewed: every byte, the
 *  pictures and files shared by them, and each conversation, the largest
 *  first (`fastctx_client::cache::Usage`). */
export interface KeptUsage {
  bytes: number;
  pictures: number;
  conversations: { address: Address; bytes: number; updated: string }[];
}

export function keptUsage(): Promise<KeptUsage> {
  return invoke("kept_usage");
}

/** Lets go of everything this device keeps of conversations. */
export function keptClear(): Promise<void> {
  return invoke("kept_clear");
}

/** Lets go of one conversation this device keeps, and of the pictures only
 *  it showed. */
export function keptForget(address: Address): Promise<void> {
  // The cache knows a conversation by the name its machine has on its hub.
  return invoke("kept_forget", { address: { ...address, machine: address.machine.split(".")[0] } });
}

/** Why words were not said: `again` when no answer came, so saying them
 *  again under the same id is how they go on. */
export interface NotSaid {
  message: string;
  again: boolean;
}

/** The user's words to a conversation. Said again under the same `id`, as
 *  after an answer that never came, they go in once. */
export function talk(who: Address, text: string, attachments: Attachment[] = [], filesOn: string | null = null, id: string | null = null): Promise<{ to: Address; landed: Landed; delivery?: string | null }> {
  return invoke("talk", { residentId: who.resident, machineName: who.machine, text, attachments, filesOn, id });
}

/** The wait before try number `tries` of what goes to `machine`, as the
 *  core paces it: `done` runs once it is over, early when told to try now
 *  or when a way to the machine opens. `unwatch` with the id calls it off. */
export function pause(tries: number, machine: MachineName | null, done: () => void): Promise<number> {
  const over = new Channel<null>();
  over.onmessage = () => done();
  return invoke("pause", { tries, machineName: machine, done: over });
}

/** Everything that broke is tried again now: the window came to the front,
 *  or the system says the network is back. */
export function retryNow(): Promise<void> {
  retryDeliveries();
  return invoke("reconnect");
}

/** An upload as the shell takes it: where the file goes, when it goes to a
 *  machine of an assisted user's, as `place` says. */
function sentTarget(machine: MachineName, target: UploadTarget, place: FilePlace | null): SentUploadTarget {
  return { kind: "sent", machine, target, operation: crypto.randomUUID(), ...(place ? { account: place.account, files_on: place.filesOn } : {}) };
}

/** Stores a file on this computer and registers its location with the conversation. */
export function uploadPath(machine: MachineName, target: UploadTarget, path: string, sent: (bytes: number) => void, place: FilePlace | null = null): Promise<Attachment> {
  const pieces = new Channel<number>();
  pieces.onmessage = sent;
  const upload = sentTarget(machine, target, place);
  return invoke("upload_path", { target: upload, machineName: machine, path, sent: pieces });
}

/** Keeps pasted bytes on this computer, or at the conversation's home on a phone. */
export async function uploadBytes(machine: MachineName, target: UploadTarget, file: File, place: FilePlace | null = null): Promise<Attachment> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  const upload = sentTarget(machine, target, place);
  return invoke("upload_bytes", bytes, {
    headers: {
      target: encodeURIComponent(JSON.stringify(upload)),
      machine: encodeURIComponent(machine),
      name: encodeURIComponent(file.name),
      type: encodeURIComponent(file.type),
    },
  });
}

/** What the user chose for a conversation they open, beyond its agent,
 *  project and model (`fastctx_types::agents::Choices`). */
export interface Choices {
  /** The account it works for, where more than one may open it there. */
  account?: HubAccount;
  /** The machines and folders the user points it at. */
  focus?: Focus;
  /** Where its colleagues live; without it, it asks for none. */
  colleagues?: string;
  /** The agent's home, as the machine it lives on writes it, when not the
   *  default one there. */
  home?: string;
  /** The Codex home its colleagues use where they live, when not the
   *  default one there. */
  colleagues_home?: string;
  /** Where the files attached to its first words go, other than images,
   *  for an agent assisting the user. */
  files_on?: string;
  /** The account's MCP servers it starts without. */
  mcp_off?: string[];
}

/** Opens a conversation with an agent; the model and reasoning effort are
 *  the official program's own names, and its defaults where not given. */
export function open(
  agent: string,
  machine: MachineName,
  town: string | null,
  text: string,
  model: string | null = null,
  effort: string | null = null,
  attachments: Attachment[] = [],
  choices: Choices = {},
): Promise<{ to: Address; landed: Landed }> {
  return invoke("open", { agent, town, machineName: machine, text, model, effort, attachments, choices });
}

/** What happened in the World after a moment, as each machine's records
 *  say now. */
export function since(at: Date, town: string | null): Promise<WorldSince> {
  return invoke("since", { since: at.toISOString(), town });
}

/** The agents each machine can start, as their own programs say; one
 *  machine, or all of them. */
export function agents(machine: MachineName | null): Promise<WorldAgents> {
  return invoke("agents", { machineName: machine });
}

export function stop(who: Address): Promise<boolean> {
  return invoke("stop", { residentId: who.resident, machineName: who.machine });
}

/** A project's overview: its map, and who is in its town now. */
export function project(town: string): Promise<{ map: MapView; who: WorldWho }> {
  return invoke("project", { town });
}

/** A text file on a machine, or a piece of it, read for the user; `held`
 *  lets go of it and hears how it goes. */
export function readText(machine: MachineName, path: string, range: FileRange, held?: HeldRead): Promise<TextRead> {
  return invoke("read_text", { machineName: machine, path, range, ...held?.args });
}

/** A file on a machine, whole, as bytes for the page to show as it is. */
export function readBytes(machine: MachineName, path: string, held?: HeldRead): Promise<BytesRead> {
  return invoke("read_bytes", { machineName: machine, path, ...held?.args });
}

/** How the shell reaches the World now: its own node, or as a device. */
export function connectionState(): Promise<ConnectionState> {
  return invoke("connection_state");
}

/** The user's word on whether Claude Code on a machine, in one of its
 *  homes or its default one, may run the World's sessions without
 *  approvals; what it now records, and whether it changed. */
export function claudeConsent(
  machine: MachineName,
  accept: boolean,
  home: string | null = null,
): Promise<{ accepted: boolean; changed: boolean }> {
  return invoke("claude_consent", { machineName: machine, accept, home });
}

/** Adds an agent's home on a machine, or hides one or shows it again. */
export function agentHomes(machine: MachineName, change: HomeChange): Promise<void> {
  return invoke("agent_homes", { machineName: machine, change });
}

/** What one machine, or every machine the user may look at, keeps of its
 *  agents' logins; nothing is started to read it. */
export function agentLogins(machine: MachineName | null): Promise<WorldLogins> {
  return invoke("agent_logins", { machineName: machine });
}

/** Scans a machine for its agents, answered once the usual places are
 *  done while its chosen folders are searched again; with `stop`, stops
 *  those searches. */
export function scanAgents(machine: MachineName, stop: boolean): Promise<MachineLogins> {
  return invoke("scan_agents", { machineName: machine, stop });
}

/** Takes up a directory on a machine as a candidate login of `agent`,
 *  asking who is logged in there. */
export function addAgentLogin(machine: MachineName, agent: string, dir: string): Promise<MachineLogins> {
  return invoke("add_agent_login", { machineName: machine, agentName: agent, dir });
}

/** Chooses a folder on a machine for logins, stops its search, or no
 *  longer searches it. */
export function agentFolder(machine: MachineName, dir: string, change: FolderChange): Promise<MachineLogins> {
  return invoke("agent_folder", { machineName: machine, dir, change });
}

/** The system's own folder picker on this computer; null when cancelled. */
export function pickFolder(): Promise<string | null> {
  return invoke("pick_folder");
}

/** What putting the World's tools into a login (`into`), or taking them
 *  out, would change, with nothing written; with `takeBack`, rewriting what
 *  the user changed of them too. */
export function previewAgentChange(machine: MachineName, agent: string, dir: string, into: boolean, takeBack = false): Promise<ChangePreview> {
  return invoke("preview_agent_change", { machineName: machine, agentName: agent, dir, into, takeBack });
}

/** Puts the World's tools into a login, or takes them out, by its
 *  machine's node; answered when the change has ended. Its steps are in
 *  the record (`agentLogins`) as it goes. */
export function changeAgentLogin(
  machine: MachineName,
  agent: string,
  dir: string,
  into: boolean,
  raiseOutputLimit = false,
  takeBack = false,
): Promise<LoginChange> {
  return invoke("change_agent_login", { machineName: machine, agentName: agent, dir, into, raiseOutputLimit, takeBack });
}

/** Reads a login's limits afresh on its machine. */
export function readAgentLimits(machine: MachineName, agent: string, dir: string): Promise<MachineLogins> {
  return invoke("read_agent_limits", { machineName: machine, agentName: agent, dir });
}

/** Turns naming new Codex conversations in a login on or off. */
export function setAgentTitle(machine: MachineName, agent: string, dir: string, on: boolean): Promise<MachineLogins> {
  return invoke("set_agent_title", { machineName: machine, agentName: agent, dir, on });
}

/** What a login on a machine was used for, as this device's accounts may
 *  see it. */
export function agentUsage(machine: MachineName, agent: string, dir: string): Promise<UsageReport> {
  return invoke("agent_usage", { machineName: machine, agentName: agent, dir });
}

/** Request records, already filtered and redacted by the login's machine. */
export function agentCalls(machine: MachineName, agent: string, dir: string, query: UsageCallsQuery = {}): Promise<UsageCallsPage> {
  return invoke("agent_calls", { machineName: machine, agentName: agent, dir, query });
}

/** Residents or jobs in detail, from every machine that has them. */

export function look(ids: string[]): Promise<LookedUp[]> {
  return invoke("look", { ids });
}

/** What a version of a project's record says, before the World stamps it;
 *  what is left out stays as it was. */
export interface RecordFields {
  name?: string | null;
  what?: string | null;
  machines?: MachineName[] | null;
  places?: { what: string; machine: MachineName; path: string }[] | null;
  canon?: { machine: MachineName; path: string } | null;
}

export interface TownRecord {
  town: string;
  record: ProjectRecord;
  opened: Stamp;
}

/** Opens a project in `account` of the hub named `hub`, or in the one
 *  account this computer is in. */
export function projectNew(name: string, fields: RecordFields, account: string | null = null, hub: string | null = null): Promise<TownRecord> {
  return invoke("project_new", { name, fields, accountName: account, hubName: hub });
}

export function projectEdit(name: string, fields: RecordFields): Promise<TownRecord> {
  return invoke("project_edit", { name, fields });
}

/** A login, and on a computer what came of keeping it connected. */
export interface LoggedIn {
  hub: string;
  name: MachineName;
  account: string;
  kept?: KeptNow | null;
  /** Why keeping the node running from boot failed, the computer being in
   *  all the same. */
  unkept?: string | null;
}

/** Logs in to `account` at the hub written `hub` (host:port) with its
 *  password; a computer registers as one of the account's machines when
 *  `register`. */
export function login(hub: string, account: string, password: string, register: boolean, name: string | null): Promise<LoggedIn> {
  return invoke("login", { hub, accountName: account, password, register, name });
}

/** What is wrong with a machine or server name as typed, by the World's
 *  one rule (`fastctx_types::NameProblem`). */
export type NameProblem =
  | { problem: "empty" }
  | { problem: "too_long" }
  | { problem: "space" }
  | { problem: "not_allowed"; character: string }
  | { problem: "bad_start"; character: string }
  | { problem: "ends_with_hyphen" }
  | { problem: "mixed_scripts"; first: string; second: string }
  | { problem: "reserved" };

/** What is wrong with `text` as the name of a machine or of a server;
 *  null when nothing is. */
export function nameProblem(kind: "machine" | "hub", text: string): Promise<NameProblem | null> {
  return invoke("name_problem", { kind, text });
}

/** What a code lets this computer into, asked of its server before it is
 *  used; null from a server that says nothing of one before. */
export function peekToken(token: string): Promise<TokenPeek | null> {
  return invoke("peek_token", { token });
}

/** Logs in with a one-time token another device made. */
export function loginToken(token: string, name: string | null): Promise<LoggedIn> {
  return invoke("login_token", { token, name });
}

/** Opens an account on a hub that lets anyone, and logs in to it. */
export function openAccount(hub: string, account: string, password: string, register: boolean, name: string | null): Promise<LoggedIn> {
  return invoke("open_account", { hub, accountName: account, password, register, name });
}

/** How `machine` is doing, or this computer with none; its lines' history
 *  only after `after`, when given (`fastctx_types::stats`). */
export function machineStats(machine: MachineName | null, after: string | null = null, seen: StatsSeen | null = null): Promise<MachineStats> {
  return invoke("machine_stats", { machineName: machine, after, seen });
}

/** What `machine`, or this computer with none, keeps free in memory for its
 *  agents; sets it first when `bytes` is given. */
export function machineMemoryRoom(machine: MachineName | null, bytes: number | null = null): Promise<number> {
  return invoke("machine_memory_room", { machineName: machine, bytes });
}

/** Renames this computer `name`: on no hub, here alone; on hubs, through
 *  the one it is registered on, which the others follow. */
export function renameHere(name: string): Promise<void> {
  return invoke("rename_here", { name });
}

/** The HTTP proxy this computer reaches its hubs through: read it, or with
 *  `change`, set it, none clearing it. */
export function hubProxy(url: string | null, change: boolean): Promise<string | null> {
  return invoke("hub_proxy", { url, change });
}

/** This computer's addresses another might reach it at. */
export function hereAddresses(): Promise<{ interface: string; address: string; physical: boolean }[]> {
  return invoke("here_addresses");
}

/** Makes this computer the hub of a new World, runs it as a service and
 *  logs in to it as its super administrator. */
export function createWorld(name: string, admin: string, password: string, reach: string[], register: boolean, machine: string | null): Promise<LoggedIn> {
  return invoke("create_world", { name, admin, password, public: reach, register, machine });
}

export function accounts(): Promise<AccountsReply> {
  return invoke("accounts");
}

/** The networks said dissolved were seen: they are said no more. */
export function seenDissolved(): Promise<void> {
  return invoke("seen_dissolved");
}

/** Newest first; pass next_before to read the next page. */
export function machineLog(hub: string, account: string, before: number | null = null): Promise<import("./machine-log").MachineLogPage> {
  return invoke("machine_log", { hubName: hub, accountName: account, before });
}

/** Makes `change` as `account` at the hub named `hub`; resolves to a token
 *  when it made one. */
export function accountChange(hub: string | null, account: string, change: AccountChange): Promise<string | null> {
  return invoke("account_change", { hubName: hub, accountName: account, change });
}

/** The line that installs fastctx on Linux servers and registers each as a
 *  machine of `account`, valid until `expires`: for `uses` of them, one when
 *  null, any number at zero; its code called `label`. */
export function serverCommand(hub: string | null, account: string, name: string | null, expires: string, managed = false, uses: number | null = null, label: string | null = null, windows = false, mac = false): Promise<string> {
  return invoke("server_command", { hubName: hub, accountName: account, name, expires, managed, uses, label, windows, mac });
}

/** Where this computer stands with the World (`src-tauri/src/setup.rs`). */
export interface Setup {
  installed: string | null;
  carried: string | null;
  same: boolean;
  service: boolean;
  codex: "absent" | "out" | "in" | "old";
  claude: "absent" | "out" | "in" | "old";
  /** What was done with what an old fastctx left here. */
  legacy: Legacy | null;
}

export type Legacy =
  | { kind: "done"; record: string; archives: string[]; archived: number; tidied: number; waiting: number }
  | { kind: "unreadable"; why: string };

export type KeptNow =
  | { kind: "already" }
  | { kind: "installed" }
  | { kind: "declined" }
  | { kind: "unsupported" }
  | { kind: "command"; command: string }
  | { kind: "removed" }
  | { kind: "not_kept" };

export function setupState(): Promise<Setup> {
  return invoke("setup_state");
}

export function placeFastctx(): Promise<string> {
  return invoke("place_fastctx");
}

/** Has a system service keep this computer's node, asking once for
 *  administrator rights. */
export function keepHere(): Promise<KeptNow> {
  return invoke("keep_here");
}

/** Removes the service that keeps this computer's node. */
export function releaseHere(): Promise<KeptNow> {
  return invoke("release_here");
}

/** One thing removing fastctx from this computer takes away, or leaves,
 *  as `fastctx remove plan` lists it. `kind` names what it is, for the page
 *  to say in its own words; `what` is the kernel's words for a kind the
 *  page does not know; `where` is a path, a service's or a hub's name. */
export interface RemovalItem {
  id: string;
  kind: string;
  what: string;
  where: string | null;
  /** Taking it away needs administrator rights. */
  admin: boolean;
}

export interface RemovalPlan {
  everything: boolean;
  steps: RemovalItem[];
  /** What stays when the data is kept. */
  keeps: RemovalItem[];
  /** The World this computer is the hub of, if it is one. */
  hub: { name: string; dir: string } | null;
  /** Another fastctx on the PATH that is not the World's, such as one npm
   *  installed; it is not taken away. */
  other: { path: string; version: string | null } | null;
  /** The network this computer is a managed machine on, and its owner, while
   *  the owner keeps it: only an administrator of this computer removes
   *  Fastctx then. */
  managed?: { hub: string; owner: string };
}

/** What came of one step, and at the end what is left and why. */
export type RemovalEvent =
  | { step: string; result: "done" | "failed" | "skipped"; why: string | null }
  | { left: { kind: string; what: string; where: string | null; why: string }[] };

export function removalPlan(everything: boolean): Promise<RemovalPlan> {
  return invoke("removal_plan", { everything });
}

/** Removes fastctx from this computer, telling each step as it is done;
 *  the rights it needs are asked for once. */
export function removeHere(everything: boolean, onEvent: (event: RemovalEvent) => void): Promise<void> {
  const events = new Channel<RemovalEvent>();
  events.onmessage = (event) => {
    try {
      onEvent(event);
    } catch (error) {
      console.error("a step of the removal could not be shown", event, error);
    }
  };
  return invoke("removal_run", { everything, events });
}

/** Closes World and has its own uninstaller take it, and with everything
 *  its data, off this computer. */
export function removeApp(everything: boolean): Promise<void> {
  return invoke("removal_finish", { everything });
}

/** Opens a resident on this computer in its own official interface, in a
 *  terminal window of its own; resolves when that attach ends. */
export function attachHere(resident: string): Promise<void> {
  return invoke("attach_here", { residentId: resident });
}

export function applyHosts(codex: boolean, claude: boolean, raise: boolean, into: boolean, home: string | null = null): Promise<string[]> {
  return invoke("apply_hosts", { codex, claude, raise, into, home });
}

/** Does something to a resident's conversation, besides speaking into it. */
export function control(who: Address, what: Control): Promise<Controlled> {
  return invoke("control", { residentId: who.resident, machineName: who.machine, control: what });
}

/** What a resident's conversation offers now. */
export function offers(who: Address): Promise<Offers> {
  return invoke("offers", { residentId: who.resident, machineName: who.machine });
}

/** The host's original record of one item; null when it no longer exists. */
export function itemDetail(address: Address, turn: string | null, item: string): Promise<unknown | null> {
  return invoke("item_detail", { address, turn, item });
}

/** One of the host's own agents' own record, as its host keeps it, read
 *  from the machine the conversation that started it lives on. */
export function agentActivity(address: Address, agent: string): Promise<AgentActivity> {
  return invoke("agent_activity", { address, agent });
}

/** Whether this interface runs in the Android shell, which reads QR codes
 *  with the camera. */
export const canScan = (): boolean => inShell() && /Android/i.test(navigator.userAgent);

/** Reads a login token's QR code with the phone's camera; null when the
 *  person turned back. */
export async function scanToken(): Promise<string | null> {
  const read = await invoke<{ kind: "scanned"; code: string } | { kind: "cancelled" }>("plugin:android-world|scan");
  return read.kind === "scanned" ? read.code : null;
}

/** Stops a job the user chose on the machine it runs on; the agent that
 *  started it is told the user stopped it. */
export function stopJob(job: string, machine: string | null): Promise<void> {
  return invoke("stop_job", { job, machineName: machine });
}

/** How many days this computer keeps a command's output once its end was
 *  delivered (`src-tauri/src/retention.rs`); none before its node first
 *  ran. */
export function retentionRead(): Promise<number | null> {
  return invoke("retention_read");
}

export function retentionWrite(days: number): Promise<void> {
  return invoke("retention_write", { days });
}

/** Making a World on a server over SSH (`src-tauri/src/remote_world.rs`). */
export function remoteProbe(host: string, port: number): Promise<RemoteKey> {
  return invoke("remote_probe", { host, port });
}

/** Signs in to the server whose key was confirmed and looks at it: the
 *  name it would take, the ports taken, a World's hub kept there. */
export function remoteLook(server: RemoteServer): Promise<Looked> {
  return invoke("remote_look", { server });
}

/** Takes the World a server keeps off it, as for a hub made and not wanted. */
export function remoteWithdraw(server: RemoteServer, step: (step: RemoteStep) => void): Promise<void> {
  const steps = new Channel<RemoteStep>();
  steps.onmessage = step;
  return invoke("remote_withdraw", { server, steps });
}

export function remoteHub(request: RemoteHub, step: (step: RemoteStep) => void): Promise<{ session: number; address: string; machine: string }> {
  const steps = new Channel<RemoteStep>();
  steps.onmessage = step;
  return invoke("remote_hub", { request, steps });
}

export function remoteOpen(server: RemoteServer, step: (step: RemoteStep) => void): Promise<{ session: number; machine: string }> {
  const steps = new Channel<RemoteStep>();
  steps.onmessage = step;
  return invoke("remote_open", { server, steps });
}

export function remoteJoin(session: number, token: string, step: (step: RemoteStep) => void, keepOpen = false, proxy: string | null = null): Promise<void> {
  const steps = new Channel<RemoteStep>();
  steps.onmessage = step;
  return invoke("remote_join", { session, token, keepOpen, proxy, steps });
}

/** Installs the service of the hub moving to the server the session is open to. */
export function remoteArrive(session: number, step: (step: "arrive") => void): Promise<void> {
  const steps = new Channel<"arrive">();
  steps.onmessage = step;
  return invoke("remote_arrive", { session, steps });
}

/** `request` of a move of the hub named `hub`, as `account`, its super administrator. */
export function hubMove(hub: string | null, account: string, request: HubMoveRequest): Promise<HubMoveReply> {
  return invoke("hub_move", { hubName: hub, accountName: account, request });
}

/** Installs the service of the hub moving to this computer, asking for administrator rights here. */
export function arriveHub(): Promise<void> {
  return invoke("arrive_hub");
}

export function remoteClose(session: number): Promise<void> {
  return invoke("remote_close", { session });
}
