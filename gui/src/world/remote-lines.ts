// A file or a command's output on some machine, read a few lines at a time
// by the viewer that shows it (`longview`): the machine holding it indexes
// its lines and searches it where it is; this device keeps only what is
// read (`src-tauri/src/line_reading.rs`).
//
// The index lives as long as the connection that opened it, so a way that
// broke takes it along. The lines are not over for that (`world-network.md`,
// 断开不是结束): they are opened again as everything that tries again is
// paced, a read or search the break cut off is asked again of the lines
// opened anew, and the viewer hears they may all have changed. Only the
// machine saying the file is not there any more ends them.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { LineChange, LineCursor, LineExtent, LineInterruption, LineMatch, LineSource } from "../longview";
import { after, whyOf } from "./again";
import type { MachineName } from "./events";

/** How the lines are read: a file, in the encoding given or the one its
 *  bytes show; or the output of a job, as its terminal drew it. */
export type Decoding = { kind: "file"; encoding?: string | null } | { kind: "output"; job: string };

interface Extent {
  length: number;
  complete: boolean;
  version: number;
}

type Change = { kind: "changed"; extent: Extent; first_changed_line: number } | { kind: "error"; message: string };

interface Opened {
  machine: MachineName;
  source: string;
  path: string;
  extent: Extent;
  watch: number;
}

/** What was asked to open: the same is asked again when the way breaks. */
interface Asked {
  machine: MachineName | null;
  path: string;
  decoding: Decoding;
}

/** Whether opening failed because the machine says the lines are gone: the
 *  file is not there, or the job's output was not kept. */
const gone = (error: unknown): boolean =>
  typeof error === "object" && error !== null && ["source_missing", "output_not_retained"].includes(String((error as { code?: unknown }).code));

/** `promise`, unless `signal` gives up waiting for it first. */
function until<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    const aborted = () => reject(signal.reason);
    signal.addEventListener("abort", aborted, { once: true });
    promise.then(resolve, reject).finally(() => signal.removeEventListener("abort", aborted));
  });
}

export class RemoteLines implements LineSource {
  private known: Extent;
  private readonly listeners = new Set<(change: LineChange) => void>();
  private readonly breaks = new Set<(interrupted: LineInterruption | null) => void>();
  /** Why the way is broken, and since when, while it is. */
  private interrupted: LineInterruption | null = null;
  /** Opening again, while the way is broken. */
  private reopening: Promise<void> | null = null;
  /** Calls off the wait before the next try. */
  private waiting: (() => void) | null = null;
  /** The machine said the lines are gone: nothing is asked any more. */
  private lost: unknown = null;
  private closed = false;
  /** What went wrong after opening, as the machine said it; reading on
   *  says so again. */
  trouble: string | null = null;

  private constructor(
    private readonly asked: Asked,
    private opened: Opened,
    channel: Channel<Change>,
    private readonly troubled: (message: string) => void,
  ) {
    this.known = opened.extent;
    this.hear(opened, channel);
  }

  /** Opens the lines of `path` on `machine` (null: this computer). */
  static async open(machine: MachineName | null, path: string, decoding: Decoding, troubled: (message: string) => void = () => {}): Promise<RemoteLines> {
    const channel = new Channel<Change>();
    const opened = await invoke<Opened>("lines_open", { machineName: machine, path, decoding, channel });
    return new RemoteLines({ machine, path, decoding }, opened, channel, troubled);
  }

  /** Where the lines are, as the machine holding them names it. */
  get machine(): MachineName {
    return this.opened.machine;
  }

  get path(): string {
    return this.opened.path;
  }

  get extent(): LineExtent {
    return { length: this.known.length, complete: this.known.complete };
  }

  read(start: number, count: number, signal: AbortSignal): Promise<readonly string[]> {
    return this.ask(signal, (opened, requestId) =>
      invoke<string[]>("lines_read", { machineName: opened.machine, source: opened.source, start, count, requestId }),
    );
  }

  async find(query: string, from: LineCursor, direction: 1 | -1, signal: AbortSignal): Promise<LineMatch | null> {
    const found = await this.ask(signal, (opened, requestId) =>
      invoke<{ line: number; offset: number; length: number } | null>("lines_find", {
        machineName: opened.machine,
        source: opened.source,
        query,
        position: from.offset === undefined ? { line: from.line } : { line: from.line, offset: from.offset },
        direction: direction === 1 ? "forward" : "backward",
        requestId,
      }),
    );
    return found && { line: found.line, start: found.offset, end: found.offset + found.length };
  }

  subscribe(changed: (change: LineChange) => void): () => void {
    this.listeners.add(changed);
    return () => this.listeners.delete(changed);
  }

  interruptions(heard: (interrupted: LineInterruption | null) => void): () => void {
    this.breaks.add(heard);
    if (this.interrupted) heard(this.interrupted);
    return () => this.breaks.delete(heard);
  }

  /** Lets the machine holding the lines forget them. */
  close(): Promise<void> {
    this.closed = true;
    this.waiting?.();
    this.listeners.clear();
    this.breaks.clear();
    return this.letGo(this.opened);
  }

  private letGo(opened: Opened): Promise<void> {
    return invoke("lines_close", { machineName: opened.machine, source: opened.source, watch: opened.watch });
  }

  /** What the machine says of the lines `opened`, while they are the ones
   *  read: a change, or that they broke. */
  private hear(opened: Opened, channel: Channel<Change>): void {
    channel.onmessage = (change) => {
      if (opened !== this.opened) return;
      if (change.kind === "error") {
        this.trouble = change.message;
        this.troubled(change.message);
        this.broke(change.message);
        return;
      }
      if (change.extent.version < this.known.version) return;
      this.known = change.extent;
      for (const listener of this.listeners) listener({ from: change.first_changed_line });
    };
  }

  /** Asks `call` of the lines as they are opened now. One the break cut
   *  off is asked again once they are opened anew; failing on those too,
   *  it is the asking that fails, and it says so. */
  private async ask<T>(signal: AbortSignal, call: (opened: Opened, requestId: string) => Promise<T>): Promise<T> {
    let again = false;
    for (;;) {
      signal.throwIfAborted();
      if (this.reopening) await until(this.reopening, signal);
      if (this.lost) throw this.lost;
      const opened = this.opened;
      try {
        return await this.asking(signal, (requestId) => call(opened, requestId));
      } catch (error) {
        if (signal.aborted || again || this.closed) throw error;
        if (opened === this.opened) this.broke(whyOf(error));
        again = true;
      }
    }
  }

  /** The way to the lines broke: they are opened again until they are, or
   *  the machine says they are gone. */
  private broke(why: string): void {
    if (this.reopening || this.closed || this.lost) return;
    this.interrupt({ why, since: Date.now() });
    this.reopening = this.reopen().finally(() => {
      this.reopening = null;
    });
  }

  private async reopen(): Promise<void> {
    const broken = this.opened;
    void this.letGo(broken).catch(() => undefined);
    for (let tries = 1; !this.closed; tries++) {
      await new Promise<void>((resolve) => {
        this.waiting = after(tries, resolve, broken.machine);
      });
      this.waiting = null;
      if (this.closed) return;
      const channel = new Channel<Change>();
      let opened: Opened;
      try {
        opened = await invoke<Opened>("lines_open", { machineName: this.asked.machine, path: this.asked.path, decoding: this.asked.decoding, channel });
      } catch (error) {
        if (gone(error)) {
          this.lost = error;
          this.interrupt(null);
          return;
        }
        this.interrupt({ why: whyOf(error), since: this.interrupted?.since ?? Date.now() });
        continue;
      }
      if (this.closed) {
        void this.letGo(opened).catch(() => undefined);
        return;
      }
      this.opened = opened;
      this.known = opened.extent;
      this.trouble = null;
      this.hear(opened, channel);
      this.interrupt(null);
      // Lines read before may have changed meanwhile: all are read anew.
      for (const listener of this.listeners) listener({ from: 0 });
      return;
    }
  }

  private interrupt(interrupted: LineInterruption | null): void {
    this.interrupted = interrupted;
    for (const heard of this.breaks) heard(interrupted);
  }

  /** A request the viewer may give up on: giving up reaches the machine
   *  holding the lines, which stops reading or searching. */
  private async asking<T>(signal: AbortSignal, ask: (requestId: string) => Promise<T>): Promise<T> {
    signal.throwIfAborted();
    const requestId = crypto.randomUUID();
    const cancel = () => void invoke("cancel_line_request", { requestId }).catch((error: unknown) => console.warn("a line request could not be given up", error));
    signal.addEventListener("abort", cancel, { once: true });
    try {
      return await ask(requestId);
    } finally {
      signal.removeEventListener("abort", cancel);
    }
  }
}
