// A file or a command's output on some machine, read a few lines at a time
// by the viewer that shows it (`longview`): the machine holding it indexes
// its lines and searches it where it is; this device keeps only what is
// read (`src-tauri/src/line_reading.rs`).

import { Channel, invoke } from "@tauri-apps/api/core";
import type { LineChange, LineCursor, LineExtent, LineMatch, LineSource } from "../longview";
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

export class RemoteLines implements LineSource {
  private known: Extent;
  private readonly listeners = new Set<(change: LineChange) => void>();
  /** What went wrong after opening, as the machine said it; reading on
   *  says so again. */
  trouble: string | null = null;

  private constructor(
    private readonly opened: Opened,
    channel: Channel<Change>,
    private readonly troubled: (message: string) => void,
  ) {
    this.known = opened.extent;
    channel.onmessage = (change) => {
      if (change.kind === "error") {
        this.trouble = change.message;
        this.troubled(change.message);
        return;
      }
      if (change.extent.version < this.known.version) return;
      this.known = change.extent;
      for (const listener of this.listeners) listener({ from: change.first_changed_line });
    };
  }

  /** Opens the lines of `path` on `machine` (null: this computer). */
  static async open(machine: MachineName | null, path: string, decoding: Decoding, troubled: (message: string) => void = () => {}): Promise<RemoteLines> {
    const channel = new Channel<Change>();
    const opened = await invoke<Opened>("lines_open", { machineName: machine, path, decoding, channel });
    return new RemoteLines(opened, channel, troubled);
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
    return this.asking(signal, (requestId) =>
      invoke<string[]>("lines_read", { machineName: this.opened.machine, source: this.opened.source, start, count, requestId }),
    );
  }

  async find(query: string, from: LineCursor, direction: 1 | -1, signal: AbortSignal): Promise<LineMatch | null> {
    const found = await this.asking(signal, (requestId) =>
      invoke<{ line: number; offset: number; length: number } | null>("lines_find", {
        machineName: this.opened.machine,
        source: this.opened.source,
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

  /** Lets the machine holding the lines forget them. */
  close(): Promise<void> {
    this.listeners.clear();
    return invoke("lines_close", { machineName: this.opened.machine, source: this.opened.source, watch: this.opened.watch });
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
