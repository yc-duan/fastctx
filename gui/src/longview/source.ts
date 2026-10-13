/** Zero-based logical lines. Text excludes the line delimiter; no Unicode
 * normalization, shortening, or markup interpretation is performed. */
export interface LineExtent {
  /** Lines currently readable, including a command's unfinished last line. */
  length: number;
  complete: boolean;
}

export interface LineChange {
  /** First changed line. Earlier lines retain their identity and contents. */
  from: number;
}

export interface LineMatch {
  line: number;
  /** UTF-16 offsets, suitable for DOM selection. */
  start: number;
  end: number;
}

/** Why the way to a source's lines is broken, and since when (milliseconds
 *  since the epoch), while it is. */
export interface LineInterruption {
  why: string;
  since: number;
}

export interface LineCursor {
  line: number;
  /** Inclusive UTF-16 start; omitted means the directional end of the line. */
  offset?: number;
}

/** A source owns indexing, transport, and search. Reads return exactly the
 * requested available lines, in order. Cancellation must reach the transport.
 * Publish a change after updating extent, including changes to the last line.
 * Remote search should run beside the file, without downloading it to search. */
export interface LineSource {
  readonly extent: LineExtent;
  read(start: number, count: number, signal: AbortSignal): Promise<readonly string[]>;
  find(query: string, from: LineCursor, direction: 1 | -1, signal: AbortSignal): Promise<LineMatch | null>;
  subscribe(changed: (change: LineChange) => void): () => void;
  /** Hears the way to the lines break, and come back (null), for a source
   *  read from elsewhere; reads wait meanwhile rather than fail. */
  interruptions?(heard: (interrupted: LineInterruption | null) => void): () => void;
}

/** In-memory adapter. The caller already owns the text; the view only retains
 * its visible slice. Replace can describe an append or an edited final line. */
export class MemoryLines implements LineSource {
  private listeners = new Set<(change: LineChange) => void>();

  constructor(private lines: readonly string[], private complete = true) {}

  get extent(): LineExtent { return { length: this.lines.length, complete: this.complete }; }

  replace(lines: readonly string[], complete: boolean, from = 0): void {
    this.lines = lines;
    this.complete = complete;
    for (const listener of this.listeners) listener({ from });
  }

  async read(start: number, count: number, signal: AbortSignal): Promise<readonly string[]> {
    signal.throwIfAborted();
    return this.lines.slice(start, start + count);
  }

  async find(query: string, from: LineCursor, direction: 1 | -1, signal: AbortSignal): Promise<LineMatch | null> {
    for (let line = from.line; line >= 0 && line < this.lines.length; line += direction) {
      signal.throwIfAborted();
      const offset = line === from.line ? from.offset : undefined;
      const start = direction === 1 ? this.lines[line].indexOf(query, offset ?? 0)
        : this.lines[line].lastIndexOf(query, offset ?? Infinity);
      if (start !== -1) return { line, start, end: start + query.length };
      // Yield on each paint so a large local search remains cancellable.
      if (line % Math.max(1, Math.ceil(this.lines.length / 100)) === 0) {
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
      }
    }
    return null;
  }

  subscribe(changed: (change: LineChange) => void): () => void {
    this.listeners.add(changed);
    return () => this.listeners.delete(changed);
  }
}
