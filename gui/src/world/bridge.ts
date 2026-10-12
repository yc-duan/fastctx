// The way to the World from inside the shell: each function is one of the
// shell's commands (`src-tauri/src/lib.rs`), which is one call of the
// core's client. Outside the shell there is no World to reach, and
// `inShell` says so.

import { Channel, invoke, isTauri } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { HomeChange, WorldAgents } from "./agents";
import type { ChangePreview, LoginChange, MachineLogins, WorldLogins } from "./logins";
import type { Control, Controlled, Offers } from "./control";
import type { WorldSince } from "./since";
import type { FileRange } from "./file-read";
import type { Attachment, UploadTarget } from "./attachments";
import type { ConnectionState } from "./device";
import type { BytesRead, TextRead } from "../app/reader";
import type { Address, MachineName, SessionEvent } from "./events";
import type { AccountChange, AccountsReply, HubAccount } from "./accounts";
import type { RemoteHub, RemoteKey, RemoteServer, RemoteStep } from "../app/remote-world";
import type { JournalSummary, JournalView, LookedUp, MapView, NodesReply, ProjectRecord, Stamp, TownSummary, WorldWho } from "./world";

export function inShell(): boolean {
  return "__TAURI_INTERNALS__" in window;
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

export type Piece =
  | { kind: "resumed" }
  | { kind: "extended"; history: SessionEvent[] }
  | { kind: "event"; event: SessionEvent }
  | { kind: "ended"; error: string | null }
  /** On a phone: the stream was read again from the start after the
   *  connection came back; it replaces everything shown. */
  | { kind: "reset"; history: SessionEvent[] }
  /** On a phone: the stream broke, and waits for the connection or for the
   *  app to come to the front. */
  | { kind: "interrupted"; error: string | null };

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

/** Watches a resident: what came before, then each piece as it comes. */
export async function watch(
  who: Address,
  onPiece: (piece: Piece) => void,
): Promise<{ id: number; history: SessionEvent[] }> {
  const pieces = new Channel<Piece>();
  // A handler that throws stops the channel for good: Tauri delivers
  // pieces in order and never moves past one that failed. A piece the page
  // cannot show is reported and passed over, so the conversation goes on.
  pieces.onmessage = (piece) => {
    try {
      onPiece(piece);
    } catch (error) {
      console.error("a piece of the conversation could not be shown", piece, error);
    }
  };
  return invoke("watch", { residentId: who.resident, machineName: who.machine, pieces });
}

export function unwatch(id: number): Promise<void> {
  return invoke("unwatch", { id });
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
  return invoke("kept_forget", { address });
}

/** The user's words to a resident, with the files uploaded for them;
 *  those other than images go to `filesOn` when the user chose a machine
 *  (an agent lent to them works on none of its own). */
export function talk(who: Address, text: string, attachments: Attachment[] = [], filesOn: string | null = null): Promise<{ to: Address; landed: Landed }> {
  return invoke("talk", { residentId: who.resident, machineName: who.machine, text, attachments, filesOn });
}

/** Carries a file on this computer to a machine, for an agent there or
 *  one about to be asked in; `sent` hears how many bytes have gone. */
export function uploadPath(machine: MachineName, target: UploadTarget, path: string, sent: (bytes: number) => void): Promise<Attachment> {
  const pieces = new Channel<number>();
  pieces.onmessage = sent;
  return invoke("upload_path", { target, machineName: machine, path, sent: pieces });
}

/** Carries bytes the page holds (a pasted picture) to a machine. They go
 *  as the call's body; where and what as headers. */
export async function uploadBytes(machine: MachineName, target: UploadTarget, file: File): Promise<Attachment> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  return invoke("upload_bytes", bytes, {
    headers: {
      target: encodeURIComponent(JSON.stringify(target)),
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
  /** The machine it works on, as written, when not the one it lives on. */
  works_on?: string;
  /** Where its colleagues live; without it, it asks for none. */
  colleagues?: string;
  /** The agent's home, as the machine it lives on writes it, when not the
   *  default one there. */
  home?: string;
  /** The Codex home its colleagues use where they live, when not the
   *  default one there. */
  colleagues_home?: string;
  /** Where the files attached to its first words go, other than images,
   *  for an agent lent to the user. */
  files_on?: string;
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

/** A journal's latest entries that fit, or those before an entry. */
export function journal(town: string, name: string, before: string | null): Promise<JournalView> {
  return invoke("journal", { town, journal: name, before });
}

/** A text file on a machine, or a piece of it, read for the user. */
export function readText(machine: MachineName, path: string, range: FileRange): Promise<TextRead> {
  return invoke("read_text", { machineName: machine, path, range });
}

/** A file on a machine, whole, as bytes for the page to show as it is. */
export function readBytes(machine: MachineName, path: string): Promise<BytesRead> {
  return invoke("read_bytes", { machineName: machine, path });
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
 *  done while the walk of its disks goes on; with `stop`, stops the walk. */
export function scanAgents(machine: MachineName, stop = false): Promise<MachineLogins> {
  return invoke("scan_agents", { machineName: machine, stop });
}

/** Takes up a directory on a machine as a candidate login of `agent`. */
export function addAgentLogin(machine: MachineName, agent: string, dir: string): Promise<MachineLogins> {
  return invoke("add_agent_login", { machineName: machine, agentName: agent, dir });
}

/** What putting the World's tools into a login (`into`), or taking them
 *  out, would change, with nothing written. */
export function previewAgentChange(machine: MachineName, agent: string, dir: string, into: boolean): Promise<ChangePreview> {
  return invoke("preview_agent_change", { machineName: machine, agentName: agent, dir, into });
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
): Promise<LoginChange> {
  return invoke("change_agent_login", { machineName: machine, agentName: agent, dir, into, raiseOutputLimit });
}

/** Reads a login's limits afresh on its machine. */
export function readAgentLimits(machine: MachineName, agent: string, dir: string): Promise<MachineLogins> {
  return invoke("read_agent_limits", { machineName: machine, agentName: agent, dir });
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

export function journals(name: string): Promise<JournalSummary[]> {
  return invoke("journals", { name });
}

/** An entry the user writes; returns its id. */
export function journalAdd(name: string, journal: string, text: string, description: string | null): Promise<string> {
  return invoke("journal_add", { name, journal, text, description });
}

/** A login, and on a computer what came of keeping it connected. */
export interface LoggedIn {
  hub: string;
  name: MachineName;
  account: string;
  kept?: KeptNow | null;
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

/** Logs in with a one-time token another device made. */
export function loginToken(token: string, name: string | null): Promise<LoggedIn> {
  return invoke("login_token", { token, name });
}

/** Opens an account on a hub that lets anyone, and logs in to it. */
export function openAccount(hub: string, account: string, password: string, register: boolean, name: string | null): Promise<LoggedIn> {
  return invoke("open_account", { hub, accountName: account, password, register, name });
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

/** Newest first; pass next_before to read the next page. */
export function machineLog(hub: string, account: string, before: number | null = null): Promise<import("./machine-log").MachineLogPage> {
  return invoke("machine_log", { hubName: hub, accountName: account, before });
}

/** Makes `change` as `account` at the hub named `hub`; resolves to a token
 *  when it made one. */
export function accountChange(hub: string | null, account: string, change: AccountChange): Promise<string | null> {
  return invoke("account_change", { hubName: hub, accountName: account, change });
}

/** The line that installs fastctx on a Linux server and registers it as a
 *  machine of `account`. */
export function serverCommand(hub: string | null, account: string, name: string | null, hours: number): Promise<string> {
  return invoke("server_command", { hubName: hub, accountName: account, name, hours });
}

/** Sets where a conversation works: a machine as written, or with null
 *  the one it lives on. */
export function setWorksOn(who: Address, worksOn: string | null): Promise<void> {
  return invoke("works_on", { residentId: who.resident, machineName: who.machine, worksOn });
}

/** Turns asking for colleagues on for a conversation, with the machine
 *  they live on and the Codex home they use there, or off with null. */
export function setColleagues(who: Address, livesOn: string | null, home: string | null = null): Promise<void> {
  return invoke("colleagues", { residentId: who.resident, machineName: who.machine, livesOn, home });
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

/** Whether this interface runs in the Android shell, which reads QR codes
 *  with the camera. */
export const canScan = (): boolean => isTauri() && /Android/i.test(navigator.userAgent);

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

export function remoteJoin(session: number, token: string, step: (step: RemoteStep) => void): Promise<void> {
  const steps = new Channel<RemoteStep>();
  steps.onmessage = step;
  return invoke("remote_join", { session, token, steps });
}

export function remoteClose(session: number): Promise<void> {
  return invoke("remote_close", { session });
}
