// The file reads the page holds (`src-tauri/src/held_reads.rs`). A read is
// not over because its way broke: the core opens it again and goes on from
// the byte where it broke, for as long as it is wanted. So whatever asks
// for one holds it, hears how it goes, and lets go of it when it no longer
// wants it: a page left, a picture closed.

import { Channel, invoke } from "@tauri-apps/api/core";
import { inShell } from "./bridge";

/** How a read goes, as the shell tells it. */
export interface ReadProgress {
  received: number;
  /** The bytes asked of the file, once it opened. */
  length: number | null;
  /** While the read is broken off: why, and since when (milliseconds since
   *  the Unix epoch). */
  interrupted: { why: string; since: number } | null;
}

/** The size from which a read says how much of it has come: a megabyte
 *  takes about a second on a slow line, when a count starts to help. */
const SHOWN_FROM = 1024 * 1024;

/** How much of a read has come, in hundredths, where saying so helps. */
export function shareOf(progress: ReadProgress | null): number | null {
  if (!progress?.length || progress.length < SHOWN_FROM) return null;
  return Math.floor((progress.received * 100) / progress.length);
}

let made = 0;

/** What a read ends with when the page let go of it, which nothing says. */
export class LetGo extends Error {
  constructor() {
    super("the page let go of this read");
  }
}

/** One read the page holds. */
export class HeldRead {
  readonly id = `read-${++made}`;
  released = false;
  private readonly channel: Channel<ReadProgress> | null;

  /** `told` hears how the read goes, a few times a second at most; outside
   *  the shell, as in the specimen, nothing tells it. */
  constructor(told?: (progress: ReadProgress) => void) {
    this.channel = told && inShell() ? new Channel<ReadProgress>(told) : null;
  }

  /** What a read command of the shell takes besides its own arguments. */
  get args(): { read: string; progress: Channel<ReadProgress> | null } {
    return { read: this.id, progress: this.channel };
  }

  /** The read is not wanted any more: it stops where it is. */
  release(): void {
    this.released = true;
    void invoke("cancel_read", { read: this.id }).catch(() => undefined);
  }
}

/** The reads one part of the page holds, let go of together when it goes. */
export class HeldReads {
  private readonly held = new Set<HeldRead>();

  /** Runs `work` with a read held for it until the work is done, or until
   *  `release` lets go of every read held here, when it fails as `LetGo`. */
  async hold<T>(work: (read: HeldRead) => Promise<T>, told?: (progress: ReadProgress) => void): Promise<T> {
    const read = new HeldRead(told);
    this.held.add(read);
    try {
      return await work(read);
    } catch (error) {
      throw read.released ? new LetGo() : error;
    } finally {
      this.held.delete(read);
    }
  }

  release(): void {
    for (const read of this.held) read.release();
    this.held.clear();
  }
}
