// The rows of changes read whole, kept for finding in them and opening them
// again with no request: the latest used last, up to a budget of characters
// for each conversation page, so a long conversation does not hold every
// change it ever made in memory. Those let go are read again when opened.

/** A change's rows, with what the change was when they were read. */
export interface KeptRows {
  readonly signature: string;
  readonly lines: readonly string[];
}

export class RowsKept {
  private readonly kept = new Map<string, { rows: KeptRows; size: number }>();
  private size = 0;

  constructor(private readonly budget: number) {}

  /** The characters still free before the oldest would go. */
  get room(): number {
    return Math.max(0, this.budget - this.size);
  }

  get(key: string): KeptRows | undefined {
    const one = this.kept.get(key);
    if (!one) return undefined;
    this.kept.delete(key);
    this.kept.set(key, one);
    return one.rows;
  }

  /** Keeps `rows`, letting the least recently used go past the budget; rows
   *  larger than the whole budget are not kept. */
  set(key: string, rows: KeptRows): void {
    const before = this.kept.get(key);
    if (before) {
      this.kept.delete(key);
      this.size -= before.size;
    }
    const size = rows.lines.reduce((sum, line) => sum + line.length, 0);
    if (size > this.budget) return;
    this.kept.set(key, { rows, size });
    this.size += size;
    for (const [oldest, one] of this.kept) {
      if (this.size <= this.budget) break;
      this.kept.delete(oldest);
      this.size -= one.size;
    }
  }

  clear(): void {
    this.kept.clear();
    this.size = 0;
  }
}
