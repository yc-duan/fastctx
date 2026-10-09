// The way to the World from inside the shell: each function is one of the
// shell's commands (`src-tauri/src/lib.rs`), which is one call of the
// core's client. Outside the shell there is no World to reach, and
// `inShell` says so.

import { Channel, invoke, isTauri } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { WorldAgents } from "./agents";
import type { Control, Controlled, Offers } from "./control";
import type { WorldSince } from "./since";
import type { FileRange } from "./file-read";
import type { Attachment, UploadTarget } from "./attachments";
import type { ConnectionState } from "./device";
import type { BytesRead, TextRead } from "../app/reader";
import type { Address, MachineName, SessionEvent } from "./events";
import type { AccountChange, AccountsReply, HubAccount } from "./accounts";
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

export function talk(who: Address, text: string, attachments: Attachment[] = []): Promise<{ to: Address; landed: Landed }> {
  return invoke("talk", { residentId: who.resident, machineName: who.machine, text, attachments });
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

/** Residents or jobs in detail, from every machine that has them. */
/** The user's word on whether Claude Code on a machine may run the World's
 *  sessions without approvals; what it now records, and whether it changed. */
export function claudeConsent(machine: MachineName, accept: boolean): Promise<{ accepted: boolean; changed: boolean }> {
  return invoke("claude_consent", { machineName: machine, accept });
}

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

/** Logs in with a one-time token another device made. */
export function loginToken(token: string, name: string | null): Promise<LoggedIn> {
  return invoke("login_token", { token, name });
}

/** Opens an account on a hub that lets anyone, and logs in to it. */
export function openAccount(hub: string, account: string, password: string, register: boolean, name: string | null): Promise<LoggedIn> {
  return invoke("open_account", { hub, accountName: account, password, register, name });
}

export function accounts(): Promise<AccountsReply> {
  return invoke("accounts");
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
 *  they live on, or off with null. */
export function setColleagues(who: Address, livesOn: string | null): Promise<void> {
  return invoke("colleagues", { residentId: who.resident, machineName: who.machine, livesOn });
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

/** Opens a resident on this computer in its own official interface, in a
 *  terminal window of its own; resolves when that attach ends. */
export function attachHere(resident: string): Promise<void> {
  return invoke("attach_here", { residentId: resident });
}

export function applyHosts(codex: boolean, claude: boolean, raise: boolean, into: boolean): Promise<string[]> {
  return invoke("apply_hosts", { codex, claude, raise, into });
}

/** Does something to a resident's conversation, besides speaking into it. */
export function control(who: Address, what: Control): Promise<Controlled> {
  return invoke("control", { residentId: who.resident, machineName: who.machine, control: what });
}

/** What a resident's conversation offers now. */
export function offers(who: Address): Promise<Offers> {
  return invoke("offers", { residentId: who.resident, machineName: who.machine });
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
