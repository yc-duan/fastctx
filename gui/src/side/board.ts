// A card of the side taken out into a window of its own (`world-gui.md`,
// 对话的侧面板; the user, 2026-11-17: "不是整个侧栏拉出去哦，是里面的卡片，
// 每个卡片都能拉出去"): the plan, the files, the colleagues, the host's
// agents. The window is a view of the desk's own side, not a second reader
// of the World: the page is opened there with `?board=<card>` on the same
// conversation, which loads a small entry of its own (`board/main.ts`), and
// everything the card shows and does is asked of the desk's window over a
// channel of this origin: calls answered once, and what is followed, told
// as it changes. So the conversation, its plan and its files are read
// once, by the desk, however many of its cards stand in windows.
//
// A board says it stands, and that it went; the desk keeps the card's
// place in its side, where it is called forward or back; the files pressed
// in the conversation go to the board that reads files, where they open on
// tabs; put back, a board hands over the files it has open and shuts. A
// conversation the desk shows no more takes its boards with it.

import { talkLink } from "../app/places";
import { invoke } from "@tauri-apps/api/core";
import { inShell, onComputer } from "../world/bridge";
import type { Address } from "../world/wire";
import type { Card } from "./shown";

/** What a board holds: one of the side's cards. The goal's is not one:
 *  its bar stands above the input. */
export type Scope = Exclude<Card, "goal">;
const SCOPES: readonly Scope[] = ["plan", "files", "colleagues", "agents"];

/** A file to open on a tab: at what it holds, or at its changes. */
export interface FileAsked {
  machine: string;
  path: string;
  tab: "content" | "changes" | null;
}

/** Why a call failed, as it crosses between the windows. */
interface Failure {
  message: string;
  /** The failure says the thing asked for is not there. */
  gone: boolean;
}

type Said =
  | { kind: "here" | "gone" | "got" | "return" | "front"; board: string }
  | { kind: "who" }
  /** The desk shows this conversation no more: its boards shut. */
  | { kind: "bye"; talk: string }
  | { kind: "file"; board: string; file: FileAsked }
  | { kind: "back"; board: string; files: FileAsked[] }
  | { kind: "call"; board: string; id: number; method: string; args: unknown[] }
  | { kind: "reply"; board: string; id: number; value?: unknown; failure?: Failure }
  | { kind: "watch"; board: string; id: number; topic: string; args: unknown[] }
  | { kind: "unwatch"; board: string; id: number }
  | { kind: "push"; board: string; id: number; args: unknown[] };

const asked = new URLSearchParams(location.search);

/** The one card this window holds of a conversation's side, where it is a
 *  board; none in the desk's own window. */
export const ONLY: Scope | null = SCOPES.find((scope) => scope === asked.get("board")) ?? null;
/** This window is a board, with nothing of the desk beside it. */
export const ALONE = ONLY !== null;

const channel = typeof BroadcastChannel === "function" ? new BroadcastChannel("fastctx-side-board") : null;

/** Whether a card can be taken out of this window's side: in a browser,
 *  and in the app's shell on a computer, which opens the window itself
 *  (`src-tauri/src/board.rs`); not on a phone. */
const SHELL = inShell();
export const PULLS = !ALONE && channel !== null && (!SHELL || onComputer());

/** How long a board has to say it got what was sent before it is taken to
 *  be gone; and how long a call waits for its answer. */
const ANSWER = 500;
const CALL = 60_000;

/** The boards known to stand, the windows of those opened from here, who
 *  hears what is said, and who is told as boards come and go. */
const standing = new Set<string>();
const windows = new Map<string, Window>();
const hearers = new Set<(said: Said) => void>();
const watchers = new Set<() => void>();

const talkKey = (address: Address) => `${address.resident}@${address.machine}`;
const boardKey = (address: Address, scope: Scope) => `${scope}:${talkKey(address)}`;
const talkOf = (board: string) => board.slice(board.indexOf(":") + 1);

// What is said crosses as text, so it is the same however the windows are
// joined: only what can be written down crosses, and nothing else is tried.
const say = (said: Said) => channel?.postMessage(JSON.stringify(said));

function stands(key: string, there: boolean): void {
  if (standing.has(key) === there) return;
  if (there) standing.add(key);
  else standing.delete(key);
  for (const told of [...watchers]) told();
}

channel?.addEventListener("message", (event: MessageEvent<string>) => {
  const said = JSON.parse(event.data) as Said;
  if (said.kind === "here") stands(said.board, true);
  // A board put back is gone before it says so: what it hands over opens
  // here, not in the window that is shutting.
  else if (said.kind === "gone" || said.kind === "back") stands(said.board, false);
  for (const hear of [...hearers]) hear(said);
});

// A desk opened anew asks which boards stand, so what was taken out before
// it was reloaded is still left out of its side.
if (!ALONE) say({ kind: "who" });

/** Whether this card is taken out of the conversation's side and stands in
 *  a window of its own. */
export function standsOut(address: Address, scope: Scope): boolean {
  return !ALONE && standing.has(boardKey(address, scope));
}

/** Told as boards come and go; the answer stops the telling. */
export function onBoards(told: () => void): () => void {
  watchers.add(told);
  return () => watchers.delete(told);
}

/** Takes a card out into a window of its own, where it stood on the
 *  screen, or was let go, and no smaller than reads well; the files' with
 *  the files that were open. False where the window could not be opened. */
export function pullOut(address: Address, scope: Scope, files: FileAsked[], from: { left: number; top: number; width: number; height: number }, title = "Fastctx"): boolean {
  const key = boardKey(address, scope);
  const url = new URL(location.href);
  url.searchParams.set("board", scope);
  if (files.length) url.searchParams.set("open", JSON.stringify(files));
  else url.searchParams.delete("open");
  url.hash = talkLink(address);
  const width = Math.max(440, Math.round(from.width));
  const height = Math.max(scope === "files" ? 560 : 420, Math.round(from.height));
  const left = Math.round(window.screenX + from.left);
  const top = Math.round(window.screenY + (window.outerHeight - window.innerHeight) + from.top);
  if (SHELL) {
    // The shell opens it, with the system's own frame: a page of the app.
    invoke("board_open", { key, path: `index.html${url.search}${url.hash}`, title, left, top: Math.round(window.screenY + from.top), width, height }).catch((error: unknown) => {
      console.error("taking a card out into a window of its own", error);
      stands(key, false);
    });
    stands(key, true);
    return true;
  }
  const opened = window.open(url, `fastctx-board-${key}`, `popup,width=${width},height=${height},left=${left},top=${top}`);
  if (!opened) return false;
  windows.set(key, opened);
  stands(key, true);
  return true;
}

/** Brings the window of the card `key` forward. */
function forward(key: string): void {
  if (SHELL) void invoke("board_front", { key }).catch(() => {});
  else windows.get(key)?.focus();
}

/** Shuts this card's own window. */
function shut(): void {
  if (SHELL) void invoke("board_close").catch(() => window.close());
  else window.close();
}

/** Sends a file pressed in the conversation to the board that reads its
 *  files, which opens it on a tab and comes forward: true where one stands.
 *  `lost` where it does not answer, and is then taken to be gone. */
export function sendFile(address: Address, file: FileAsked, lost: () => void): boolean {
  if (ALONE) return false;
  const key = boardKey(address, "files");
  if (!standing.has(key)) return false;
  let got = false;
  const hear = (said: Said) => {
    if (said.kind === "got" && said.board === key) got = true;
  };
  hearers.add(hear);
  say({ kind: "file", board: key, file });
  forward(key);
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
export function onBack(heard: (of: (address: Address) => boolean, files: FileAsked[]) => void): void {
  hearers.add((said) => {
    if (said.kind === "back") heard((address) => talkOf(said.board) === talkKey(address), said.files);
  });
}

/** Calls a card's window forward. */
export function bringFront(address: Address, scope: Scope): void {
  const key = boardKey(address, scope);
  forward(key);
  say({ kind: "front", board: key });
}

/** Asks a card back from its window into the desk. One that does not
 *  answer is taken to be gone, and its card is here again. */
export function askBack(address: Address, scope: Scope): void {
  const key = boardKey(address, scope);
  say({ kind: "return", board: key });
  window.setTimeout(() => stands(key, false), ANSWER);
}

/** What the desk's window answers and tells the boards of one
 *  conversation. Each is handed the board asking first. */
export interface Served {
  /** Asked once, answered once; what it throws is the call's failure. */
  calls: Record<string, (board: string, ...args: never[]) => unknown>;
  /** Followed: told at once and as it changes, until its answer is called. */
  topics: Record<string, (board: string, tell: (...args: unknown[]) => void, ...args: never[]) => () => void>;
  /** A board went: what was held for it is let go. */
  left?(board: string): void;
  /** Whether a failure says the thing asked for is not there. */
  gone?(error: unknown): boolean;
  /** A failure in words. */
  detail?(error: unknown): string;
}

/** Serves the boards of `address` from the desk's window, until its answer
 *  is called, which shuts them: the conversation is shown here no more. */
export function serve(address: Address, served: Served): () => void {
  const talk = talkKey(address);
  /** What each board follows, by the number it gave. */
  const following = new Map<string, Map<number, () => void>>();
  const leave = (board: string) => {
    for (const stop of following.get(board)?.values() ?? []) stop();
    following.delete(board);
    served.left?.(board);
  };
  const hear = (said: Said) => {
    if (!("board" in said) || talkOf(said.board) !== talk) return;
    const board = said.board;
    if (said.kind === "here" || said.kind === "gone" || said.kind === "back") leave(board);
    else if (said.kind === "call") {
      const { id, method, args } = said;
      const failed = (error: unknown) =>
        say({ kind: "reply", board, id, failure: { message: served.detail?.(error) ?? (error instanceof Error ? error.message : String(error)), gone: served.gone?.(error) ?? false } });
      const asked = served.calls[method] as ((board: string, ...args: unknown[]) => unknown) | undefined;
      if (!asked) return failed(new Error(`the desk does not answer ${method}`));
      try {
        Promise.resolve(asked(board, ...args)).then((value) => say({ kind: "reply", board, id, value }), failed);
      } catch (error) {
        failed(error);
      }
    } else if (said.kind === "watch") {
      const { id } = said;
      const begin = served.topics[said.topic] as ((board: string, tell: (...args: unknown[]) => void, ...args: unknown[]) => () => void) | undefined;
      if (!begin) return;
      const mine = following.get(board) ?? new Map<number, () => void>();
      following.set(board, mine);
      mine.get(id)?.();
      mine.set(
        id,
        begin(board, (...args) => say({ kind: "push", board, id, args }), ...said.args),
      );
    } else if (said.kind === "unwatch") {
      following.get(board)?.get(said.id)?.();
      following.get(board)?.delete(said.id);
    }
  };
  hearers.add(hear);
  const stop = () => {
    hearers.delete(hear);
    window.removeEventListener("pagehide", stop);
    for (const board of [...following.keys()]) leave(board);
    say({ kind: "bye", talk });
  };
  // A desk reloaded or shut takes its boards with it: they have nothing to
  // show without it.
  window.addEventListener("pagehide", stop);
  return stop;
}

/** A call the desk's window answered with a failure. */
export class RemoteFailure extends Error {
  constructor(
    message: string,
    /** The thing asked for is not there. */
    readonly gone: boolean,
  ) {
    super(message);
  }
}

/** A board's way to the desk's window: what it asks, and what it follows. */
export class Remote {
  private readonly board: string;
  private next = 0;
  private readonly waiting = new Map<number, { resolve(value: unknown): void; reject(error: unknown): void; timer: number }>();
  private readonly told = new Map<number, (...args: unknown[]) => void>();

  constructor(address: Address, scope: Scope) {
    this.board = boardKey(address, scope);
    hearers.add((said) => {
      if (!("board" in said) || said.board !== this.board) return;
      if (said.kind === "reply") {
        const call = this.waiting.get(said.id);
        if (!call) return;
        this.waiting.delete(said.id);
        window.clearTimeout(call.timer);
        if (said.failure) call.reject(new RemoteFailure(said.failure.message, said.failure.gone));
        else call.resolve(said.value);
      } else if (said.kind === "push") this.told.get(said.id)?.(...said.args);
    });
  }

  /** Asks the desk's window once. */
  call<T>(method: string, ...args: unknown[]): Promise<T> {
    const id = (this.next += 1);
    return new Promise<T>((resolve, reject) => {
      const timer = window.setTimeout(() => {
        this.waiting.delete(id);
        reject(new RemoteFailure(`the desk did not answer ${method}`, false));
      }, CALL);
      this.waiting.set(id, { resolve: resolve as (value: unknown) => void, reject, timer });
      say({ kind: "call", board: this.board, id, method, args });
    });
  }

  /** Follows what the desk's window tells of `topic`; the answer stops. */
  watch(topic: string, told: (...args: never[]) => void, ...args: unknown[]): () => void {
    const id = (this.next += 1);
    this.told.set(id, told as (...args: unknown[]) => void);
    say({ kind: "watch", board: this.board, id, topic, args });
    return () => {
      if (!this.told.delete(id)) return;
      say({ kind: "unwatch", board: this.board, id });
    };
  }
}

/** In a board's window: says it stands, to those who ask too, and that it
 *  went as the window shuts; takes the files the desk sends, comes forward
 *  when called, goes `back` when asked back, and shuts when the desk shows
 *  its conversation no more. */
export function standAs(address: Address, open: (file: FileAsked) => void, back: () => void): void {
  if (!ONLY) return;
  const key = boardKey(address, ONLY);
  say({ kind: "here", board: key });
  hearers.add((said) => {
    if (said.kind === "who") say({ kind: "here", board: key });
    else if (said.kind === "bye") {
      if (said.talk === talkKey(address)) shut();
    } else if (said.board !== key) return;
    else if (said.kind === "file") {
      say({ kind: "got", board: key });
      open(said.file);
      window.focus();
    } else if (said.kind === "front") window.focus();
    else if (said.kind === "return") back();
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
  if (!ONLY) return;
  say({ kind: "back", board: boardKey(address, ONLY), files });
  shut();
}
