// What is taken out of the side into a window of its own (`world-gui.md`,
// 对话的侧面板): the whole side of a conversation, its overview and the tabs
// open; or one card of the overview, the plan, the files, those who work
// with it. The page is opened again in the new window with `?board`, on
// the same conversation, and lays over the whole of it what was taken out
// and nothing else. That window reads the conversation itself, as any
// window does; what the windows tell each other goes over a channel of
// this origin. A board says it stands, and that it went; the desk leaves
// out of its side what stands in a window, and sends the files pressed in
// the conversation to the board that reads files, where they open on tabs;
// put back, a board hands over the files it has open and shuts.

import { talkLink } from "../app/places";
import { inShell } from "../world/bridge";
import type { Address } from "../world/wire";
import type { Card } from "./shown";

/** What a board holds: the whole side, or one of its cards. */
export type Scope = "side" | Exclude<Card, "goal">;
const SCOPES: readonly Scope[] = ["side", "plan", "files", "colleagues", "agents"];

/** A file to open on a tab: at what it holds, or at its changes. */
export interface FileAsked {
  machine: string;
  path: string;
  tab: "content" | "changes" | null;
}

type Said =
  | { kind: "here" | "gone" | "got"; board: string }
  | { kind: "who" }
  | { kind: "file"; board: string; file: FileAsked }
  | { kind: "back"; board: string; files: FileAsked[] };

const asked = new URLSearchParams(location.search);

/** What this window holds of one conversation's side, where it is a
 *  board; none in the desk's own window. */
export const SCOPE: Scope | null = asked.has("board") ? (SCOPES.find((scope) => scope === asked.get("board")) ?? "side") : null;
/** This window is a board, with nothing of the desk beside it. */
export const ALONE = SCOPE !== null;
/** The one card this window holds, where it holds one. */
export const ONLY: Exclude<Scope, "side"> | null = SCOPE !== null && SCOPE !== "side" ? SCOPE : null;
if (ALONE) document.documentElement.classList.add("side-board");

const channel = typeof BroadcastChannel === "function" ? new BroadcastChannel("fastctx-side-board") : null;

/** Whether anything can be taken out of this window's side. The app's
 *  shell opens no window of this kind yet. */
export const PULLS = !ALONE && channel !== null && !inShell();

/** How long a board has to say it got what was sent before it is taken to
 *  be gone. */
const ANSWER = 500;

/** The boards known to stand, the windows of those opened from here, who
 *  hears what is said, and who is told as boards come and go. */
const standing = new Set<string>();
const windows = new Map<string, Window>();
const hearers = new Set<(said: Said) => void>();
const watchers = new Set<() => void>();

const talkKey = (address: Address) => `${address.resident}@${address.machine}`;
export const boardKey = (address: Address, scope: Scope = "side") => `${scope}:${talkKey(address)}`;

const say = (said: Said) => channel?.postMessage(said);

function stands(key: string, there: boolean): void {
  if (standing.has(key) === there) return;
  if (there) standing.add(key);
  else standing.delete(key);
  for (const told of [...watchers]) told();
}

channel?.addEventListener("message", (event: MessageEvent<Said>) => {
  const said = event.data;
  if (said.kind === "here") stands(said.board, true);
  // A board put back is gone before it says so: what it hands over opens
  // here, not in the window that is shutting.
  else if (said.kind === "gone" || said.kind === "back") stands(said.board, false);
  for (const hear of [...hearers]) hear(said);
});

// A desk opened anew asks which boards stand, so what was taken out before
// it was reloaded is still left out of its side.
if (!ALONE) say({ kind: "who" });

/** Whether this is taken out of the conversation's side and stands in a
 *  window of its own. */
export function standsOut(address: Address, scope: Scope): boolean {
  return !ALONE && standing.has(boardKey(address, scope));
}

/** Told as boards come and go; the answer stops the telling. */
export function onBoards(told: () => void): () => void {
  watchers.add(told);
  return () => watchers.delete(told);
}

/** Takes the side, or one card of it, out into a window of its own, where
 *  it stood on the screen and no smaller than reads well, with the files it
 *  had open; false where the window could not be opened. */
export function pullOut(address: Address, scope: Scope, files: FileAsked[], from: { left: number; top: number; width: number; height: number }): boolean {
  const key = boardKey(address, scope);
  const url = new URL(location.href);
  url.searchParams.set("board", scope);
  if (files.length) url.searchParams.set("open", JSON.stringify(files));
  else url.searchParams.delete("open");
  url.hash = talkLink(address);
  const width = Math.max(440, Math.round(from.width));
  const height = Math.max(scope === "side" || scope === "files" ? 560 : 420, Math.round(from.height));
  const left = Math.round(window.screenX + from.left);
  const top = Math.round(window.screenY + (window.outerHeight - window.innerHeight) + from.top);
  const opened = window.open(url, `fastctx-board-${key}`, `popup,width=${width},height=${height},left=${left},top=${top}`);
  if (!opened) return false;
  windows.set(key, opened);
  stands(key, true);
  return true;
}

/** Sends a file pressed in the conversation to the board that reads its
 *  files, which opens it on a tab and comes forward: true where one stands.
 *  `lost` where it does not answer, and is then taken to be gone. */
export function sendFile(address: Address, file: FileAsked, lost: () => void): boolean {
  if (ALONE) return false;
  const key = [boardKey(address, "files"), boardKey(address, "side")].find((one) => standing.has(one));
  if (!key) return false;
  let got = false;
  const hear = (said: Said) => {
    if (said.kind === "got" && said.board === key) got = true;
  };
  hearers.add(hear);
  say({ kind: "file", board: key, file });
  windows.get(key)?.focus();
  window.setTimeout(() => {
    hearers.delete(hear);
    if (got) return;
    stands(key, false);
    lost();
  }, ANSWER);
  return true;
}

/** A board of this conversation was put back into the desk: the files it
 *  had open, to whoever shows the conversation. */
export function onBack(heard: (address: (address: Address) => boolean, files: FileAsked[]) => void): void {
  hearers.add((said) => {
    if (said.kind === "back") heard((address) => said.board.endsWith(`:${talkKey(address)}`), said.files);
  });
}

/** In a board's window: says it stands, to those who ask too, and that it
 *  went as the window shuts; takes the files the desk sends. */
export function standAs(address: Address, open: (file: FileAsked) => void): void {
  if (!SCOPE) return;
  const key = boardKey(address, SCOPE);
  say({ kind: "here", board: key });
  hearers.add((said) => {
    if (said.kind === "who") say({ kind: "here", board: key });
    else if (said.kind === "file" && said.board === key) {
      say({ kind: "got", board: key });
      open(said.file);
      window.focus();
    }
  });
  window.addEventListener("pagehide", () => say({ kind: "gone", board: key }));
}

/** The files a board was opened with. */
export function firstFiles(): FileAsked[] {
  try {
    const files: unknown = JSON.parse(asked.get("open") ?? "[]");
    return Array.isArray(files) ? files.filter((file): file is FileAsked => !!file && typeof (file as FileAsked).machine === "string" && typeof (file as FileAsked).path === "string") : [];
  } catch {
    return [];
  }
}

/** A board goes back into the desk: its files are handed over, and its
 *  window shuts. */
export function putBack(address: Address, files: FileAsked[]): void {
  if (!SCOPE) return;
  say({ kind: "back", board: boardKey(address, SCOPE), files });
  window.close();
}
