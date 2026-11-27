// What a conversation's page was given, kept for as long as the page holds
// the conversation: every piece since the part it last read whole, with
// when it came. A page that finds itself showing what its records do not
// (`self-check.ts`) keeps a copy on this computer, which the bench replays
// (`scripts/conversation-bench`). Nothing of it leaves this computer.

/** The most of it kept, counted in characters of its JSON. */
const KEPT_AT_MOST = 2_000_000;

export class Tape {
  private lines: string[] = [];
  private size = 0;
  private began = performance.now();
  /** It holds every piece since the part read whole; one that would pass
   *  the limit is not kept, nor any after it until the next such part. */
  private whole = true;

  /** A piece the page was given. `opens` for a part read whole in place of
   *  all it held, where what came before no longer counts. */
  put(piece: object, opens = false): void {
    let line: string;
    try {
      line = JSON.stringify({ t: Math.round(performance.now() - this.began), piece });
    } catch {
      this.whole = false;
      return;
    }
    if (opens) {
      this.lines = [];
      this.size = 0;
      this.whole = true;
    }
    if (!this.whole) return;
    if (this.size + line.length > KEPT_AT_MOST) {
      this.whole = false;
      return;
    }
    this.lines.push(line);
    this.size += line.length;
  }

  /** What it holds, one JSON line a piece, and whether it is all of it. */
  read(): { text: string; whole: boolean } {
    return { text: this.lines.join("\n") + "\n", whole: this.whole };
  }
}
