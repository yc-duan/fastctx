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
import type { JournalSummary, JournalView, LookedUp, MapView, NodesReply, ProjectRecord, Scope, Stamp, TownSummary, WorldWho } from "./world";

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
): Promise<{ to: Address; landed: Landed }> {
  return invoke("open", { agent, town, machineName: machine, text, model, effort, attachments });
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

/** Joins a World as a device with an invitation made for one; nothing is
 *  installed or started here. */
export function joinDevice(code: string, name: string | null): Promise<{ world: string; name: MachineName }> {
  return invoke("join_device", { code, name });
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

export function projectNew(name: string, fields: RecordFields): Promise<TownRecord> {
  return invoke("project_new", { name, fields });
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

/** An invitation for one more machine, or for a phone (`device`), valid
 *  for `hours`; what it may do until a grant says otherwise is `scope`,
 *  everything when null. */
export function invite(name: string | null, hours: number, device = false, scope: Scope | null = null): Promise<string> {
  return invoke("invite", { name, hours, device, scope });
}

/** Sets what `member` may do, and whether it holds management rights. */
export function grant(member: MachineName, scope: Scope, manage: boolean): Promise<void> {
  return invoke("grant", { member, scope, manage });
}

/** Where `member` connects from now on: through the relays `via`, in
 *  order, then the hub; straight to the hub when `via` is empty. */
export function route(member: MachineName, via: MachineName[]): Promise<void> {
  return invoke("route", { member, via });
}

/** Makes `machine` relay, dialed at `public` (IP:port each) and listening
 *  at `listen`, or stop relaying when `public` is empty. Resolves to where
 *  it relays now. */
export function setRelay(machine: MachineName, publicAt: string[], listen: string | null): Promise<string[]> {
  return invoke("set_relay", { machine, public: publicAt, listen });
}

/** The user has been shown that `member` came in with this machine's
 *  invitation. */
export function seen(member: MachineName): Promise<void> {
  return invoke("seen", { member });
}

export function revoke(name: MachineName, reason: string): Promise<void> {
  return invoke("revoke", { name, reason });
}

export function leave(): Promise<void> {
  return invoke("leave");
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

/** What joining comes to: it keeps the machine connected, never removes
 *  a service. */
export type JoinKept = Exclude<KeptNow, { kind: "removed" | "not_kept" }>;

export function joinWorld(code: string, name: string | null): Promise<{ world: string; name: MachineName; kept: JoinKept }> {
  return invoke("join_world", { code, name });
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

/** Founds a hub on this computer; resolves to the code that founds its World. */
export function hubHere(publicAddresses: string[]): Promise<string> {
  return invoke("hub_here", { public: publicAddresses });
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

/** Reads an invitation's QR code with the phone's camera; null when the
 *  person turned back. */
export async function scanInvitation(): Promise<string | null> {
  const read = await invoke<{ kind: "scanned"; code: string } | { kind: "cancelled" }>("plugin:android-world|scan");
  return read.kind === "scanned" ? read.code : null;
}
