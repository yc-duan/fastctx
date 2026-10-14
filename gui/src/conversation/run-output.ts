// What a command printed, as the model got it (`world-run.md`, 输出): its
// lines with their places in the whole output, and where something was left
// out, how much. What a write printed of itself is drawn as that write's
// row, not again here. The whole output stays on the machine that ran it
// and is read from there only when asked for, in the full-screen view.

import type { LineChange, LineCursor, LineExtent, LineMatch, LinePaint, LineSource } from "../longview";
import type { OutputNote, OutputSpan, WriteFact } from "../world/run-facts";

export interface OutputWords {
  /** Lines of the output the model was not given. */
  linesLeftOut(lines: number): string;
  /** Characters of one long line the model was not given. */
  charsLeftOut(chars: number): string;
  /** A line no known encoding reads. */
  unreadable: string;
}

/** Whether a stretch of output is a write's own note, which the write's row
 *  stands for. */
export function writeNote(note: OutputNote | undefined, writes: readonly WriteFact[]): boolean {
  if (!note?.write || (note.command !== "write" && note.command !== "edit" && note.command !== "apply_patch")) return false;
  const operation = `${note.write}-`;
  return writes.some((write) => write.id.startsWith(operation));
}

export interface ShownOutput {
  lines: string[];
  /** Each line's number in the whole output; null where a line stands for
   *  what was left out. */
  numbers: (number | null)[];
  /** Why a line reads as it does, where it was not readable. */
  titles: Map<number, string>;
  /** Whether anything was left out, so that the numbers say something. */
  cut: boolean;
}

/** The output the model got, as lines; null when it printed nothing of its
 *  own. */
export function shownOutput(spans: readonly OutputSpan[], writes: readonly WriteFact[], words: OutputWords): ShownOutput | null {
  const shown: ShownOutput = { lines: [], numbers: [], titles: new Map(), cut: false };
  let open = "";
  let number: number | null = null;
  const flush = () => {
    if (number === null) return;
    shown.lines.push(open);
    shown.numbers.push(number);
    open = "";
    number = null;
  };
  const gap = (said: string, title?: string) => {
    flush();
    if (title) shown.titles.set(shown.lines.length, title);
    shown.lines.push(said);
    shown.numbers.push(null);
    shown.cut = true;
  };
  // Where the parts of one long line were given, so the characters between
  // and around them can be counted.
  let part: { line: number; to: number; of: number } | null = null;
  const endPart = () => {
    if (part && part.to < part.of) gap(words.charsLeftOut(part.of - part.to));
    part = null;
  };
  for (const span of spans) {
    const piece = span.piece;
    if (writeNote(span.note, writes)) {
      // A write's note is drawn as the write's row. Where it is part of a
      // long line, what it holds was given, not left out.
      if (piece.kind === "part_of_line") {
        flush();
        if (part && part.line !== piece.line) endPart();
        const before = part ? part.to : 0;
        if (piece.from - 1 > before) gap(words.charsLeftOut(piece.from - 1 - before));
        part = { line: piece.line, to: piece.to, of: piece.of };
      }
      continue;
    }
    if (piece.kind !== "part_of_line") endPart();
    switch (piece.kind) {
      case "text": {
        // A stretch on a line already begun goes on with it; one on
        // another line ends it (its line's end was in a write's note).
        let line = piece.first_line;
        if (number !== null && line !== number) flush();
        for (const segment of piece.text.split(/(?<=\n)/)) {
          if (!segment) continue;
          if (number === null) number = line;
          if (segment.endsWith("\n")) {
            open += segment.slice(0, segment.endsWith("\r\n") ? -2 : -1);
            flush();
            line += 1;
          } else open += segment;
        }
        break;
      }
      case "part_of_line": {
        flush();
        const before = part && part.line === piece.line ? part.to : 0;
        if (part && part.line !== piece.line) endPart();
        if (piece.from - 1 > before) gap(words.charsLeftOut(piece.from - 1 - before));
        shown.lines.push(piece.text);
        shown.numbers.push(piece.line);
        shown.cut = true;
        part = { line: piece.line, to: piece.to, of: piece.of };
        break;
      }
      case "omitted":
        gap(words.linesLeftOut(piece.last_line - piece.first_line + 1));
        break;
      case "unreadable":
        flush();
        shown.titles.set(shown.lines.length, piece.reason);
        shown.lines.push(words.unreadable);
        shown.numbers.push(piece.line);
        shown.cut = true;
        break;
      case "image":
        break;
    }
  }
  endPart();
  flush();
  return shown.lines.length ? shown : null;
}

/** Draws an output line with its number in the whole output, and what was
 *  left out as a quiet line of its own; unnumbered, unless `numbered`. */
export function outputPaint(shown: ShownOutput, numbered = true): LinePaint {
  // Not spread into Math.max: an output may have more lines than a call
  // takes arguments.
  let most = 0;
  for (const kept of shown.numbers) if (kept !== null && kept > most) most = kept;
  const widest = String(most).length;
  return (row, line, index, match) => {
    const holder = row.parentElement;
    if (numbered && holder && widest > Number(holder.style.getPropertyValue("--line-digits") || 0)) holder.style.setProperty("--line-digits", String(widest));
    const number = document.createElement("span");
    number.className = "lv-number";
    number.setAttribute("aria-hidden", "true");
    const kept = shown.numbers[index];
    const text = document.createElement("span");
    text.className = "lv-text";
    const title = shown.titles.get(index);
    if (kept === null || kept === undefined || title !== undefined) row.classList.add("lv-gap");
    if (kept !== null && kept !== undefined) number.textContent = String(kept);
    if (title !== undefined) row.title = title;
    if (match) {
      const mark = document.createElement("mark");
      mark.textContent = line.slice(match.start, match.end);
      text.append(line.slice(0, match.start), mark, line.slice(match.end));
    } else text.textContent = line;
    if (numbered) row.append(number);
    row.append(text);
  };
}

/** Why lines could not be read, in the user's words; what the machine said
 *  is its detail, shown on hover. */
export class Said extends Error {
  constructor(
    message: string,
    readonly detail: string,
  ) {
    super(message);
  }

  override toString(): string {
    return this.detail || this.message;
  }
}

/** How lines are read from the machine holding them. */
export type OpenedLines = LineSource & { close(): Promise<void> };

/** A command's whole output on its machine, opened when first read and let
 *  go when the view showing it closes; what the run said of it stands for
 *  its extent until then. */
export class WholeOutput implements LineSource {
  private opened: Promise<OpenedLines> | null = null;
  private lines: OpenedLines | null = null;
  private unsubscribe: () => void = () => {};
  private readonly listeners = new Set<(change: LineChange) => void>();

  constructor(
    private readonly open: () => Promise<OpenedLines>,
    private readonly known: LineExtent,
    /** The error to show for a failure to open. */
    private readonly said: (error: unknown) => Error,
  ) {}

  get extent(): LineExtent {
    return this.lines?.extent ?? this.known;
  }

  private ready(): Promise<OpenedLines> {
    this.opened ??= this.open().then(
      (lines) => {
        this.lines = lines;
        this.unsubscribe = lines.subscribe((change) => this.tell(change));
        // The output went on after the run answered, or counts its last
        // line otherwise: what was asked for is asked again.
        if (lines.extent.length !== this.known.length || lines.extent.complete !== this.known.complete) {
          this.tell({ from: Math.min(lines.extent.length, this.known.length) });
        }
        return lines;
      },
      (error: unknown) => {
        this.opened = null;
        throw this.said(error);
      },
    );
    return this.opened;
  }

  private tell(change: LineChange): void {
    for (const listener of this.listeners) listener(change);
  }

  async read(start: number, count: number, signal: AbortSignal): Promise<readonly string[]> {
    const lines = await this.ready();
    return lines.read(start, count, signal);
  }

  async find(query: string, from: LineCursor, direction: 1 | -1, signal: AbortSignal): Promise<LineMatch | null> {
    const lines = await this.ready();
    return lines.find(query, from, direction, signal);
  }

  subscribe(changed: (change: LineChange) => void): () => void {
    this.listeners.add(changed);
    return () => this.listeners.delete(changed);
  }

  /** Lets the machine holding it forget it; read again, it opens again. */
  close(): void {
    const lines = this.lines;
    this.unsubscribe();
    this.unsubscribe = () => {};
    this.lines = null;
    this.opened = null;
    if (lines) void lines.close().catch((error: unknown) => console.warn("the whole output could not be let go", error));
  }
}
