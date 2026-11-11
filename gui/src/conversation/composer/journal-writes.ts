export class JournalFailure extends Error {
  constructor(readonly document: string, cause: unknown) {
    super(cause instanceof Error ? cause.message : String(cause), { cause });
  }
}

/** A successful write proves only that document's journal is healthy. */
export class JournalWrites {
  save: (document: string) => Promise<void> = async () => {};
  delete: (document: string) => Promise<void> = async () => {};
  private readonly failures = new Map<string, unknown>();
  constructor(private readonly active: () => boolean, private readonly changed: () => void) {}
  error(document: string): unknown { return this.failures.get(document) ?? null; }
  failed(document: string, error: unknown): void {
    const previous = this.failures.get(document);
    this.failures.set(document, error);
    if (this.active() && String(previous) !== String(error)) this.changed();
  }
  succeeded(document: string): void {
    if (this.failures.delete(document) && this.active()) this.changed();
  }
  persist(document: string): Promise<void> { return this.write(document, this.save); }
  forget(document: string): Promise<void> { return this.write(document, this.delete); }
  private async write(document: string, write: (document: string) => Promise<void>): Promise<void> {
    if (!this.active()) return;
    try { await write(document); this.succeeded(document); }
    catch (error) { this.failed(document, error); throw new JournalFailure(document, error); }
  }
}
