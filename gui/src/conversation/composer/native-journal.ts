import type { ComposerJournal } from "./document";
import type { NativeJournal } from "./native";

/** Native tails share the existing database but have a private key namespace.
 * Older outbox readers cannot mistake a binary update row for a draft journal. */
export class NativeJournalStorage {
  private readonly bases = new Map<string, { checkpoint: string; confirmed: string | undefined }>();
  private readonly rows = new Map<string, Map<number, string>>();
  private readonly metadata = new Map<string, string>();
  private readonly writes = new Map<string, Promise<void>>();
  constructor(private readonly db: IDBDatabase, private readonly prefix: string) {}
  private tail(key: string): string { return `\u0001${this.prefix}${encodeURIComponent(key)}\u0000`; }
  private range(key: string): IDBKeyRange { const prefix = this.tail(key); return IDBKeyRange.bound(prefix, `${prefix}\uffff`); }

  async hydrate(entries: Map<string, ComposerJournal>): Promise<ReadonlyMap<string, ComposerJournal>> {
    for (const [key, journal] of entries) {
      if (!journal.native) continue;
      const retained = journal.native;
      const values = await new Promise<{ key: string; value: unknown }[]>((resolve, reject) => {
        const transaction = this.db.transaction("edits", "readonly");
        const request = transaction.objectStore("edits").openCursor(this.range(key));
        const rows: { key: string; value: unknown }[] = [];
        request.onsuccess = () => { const cursor = request.result; if (cursor) { rows.push({ key: String(cursor.key), value: cursor.value }); cursor.continue(); } };
        transaction.oncomplete = () => resolve(rows);
        transaction.onabort = () => reject(transaction.error ?? new Error("the composer update journal read was aborted"));
      });
      let metadata = journal;
      const updates: NonNullable<NativeJournal["updates"]>[number][] = [];
      const rows = new Map<number, string>();
      for (const row of values) {
        if (row.key.endsWith("\u0000metadata")) { metadata = row.value as ComposerJournal; this.metadata.set(key, JSON.stringify(metadata)); }
        else { const update = row.value as NonNullable<NativeJournal["updates"]>[number]; updates.push(update); rows.set(update.input.number, JSON.stringify(update)); }
      }
      updates.sort((a, b) => a.input.number - b.input.number);
      this.rows.set(key, rows); this.bases.set(key, { checkpoint: retained.checkpoint, confirmed: retained.confirmed });
      // Pre-tail journals remain readable until the first new checkpoint replaces
      // them. Their original exact outbox is retained during that conversion.
      entries.set(key, { ...metadata, native: { ...retained, ...metadata.native, checkpoint: retained.checkpoint, confirmed: retained.confirmed ?? retained.checkpoint,
        updates, outbox: updates.length ? updates.filter((row) => row.pending).map((row) => row.input) : retained.outbox } });
    }
    return entries;
  }

  write(key: string, journal: ComposerJournal | null): Promise<void> {
    const previous = this.writes.get(key) ?? Promise.resolve();
    const writing = previous.catch(() => {}).then(() => this.writeNext(key, journal));
    this.writes.set(key, writing);
    void writing.finally(() => { if (this.writes.get(key) === writing) this.writes.delete(key); }).catch(() => {});
    return writing;
  }
  private async writeNext(key: string, journal: ComposerJournal | null): Promise<void> {
    const native = journal?.native;
    const previous = this.bases.get(key);
    const rows = this.rows.get(key) ?? new Map<number, string>();
    const next = new Map<number, string>();
    const metadata = journal && native ? { ...journal, native: { document: native.document, next: native.next, holds: native.holds, editing: native.editing, mutations: native.mutations } } : null;
    const encodedMetadata = JSON.stringify(metadata);
    await new Promise<void>((resolve, reject) => {
      const transaction = this.db.transaction("edits", "readwrite");
      const store = transaction.objectStore("edits");
      try {
      if (!journal) {
        store.delete(this.prefix + key);
        const cursor = store.openCursor(this.range(key));
        cursor.onsuccess = () => { const row = cursor.result; if (row) { row.delete(); row.continue(); } };
      } else if (native) {
        if (!previous || previous.checkpoint !== native.checkpoint || previous.confirmed !== native.confirmed) {
          store.put({ ...journal, native: { ...native, updates: [], outbox: [] } }, this.prefix + key);
        }
        if (this.metadata.get(key) !== encodedMetadata) store.put(metadata, this.tail(key) + "metadata");
        for (const update of native.updates ?? []) {
          const value = JSON.stringify(update);
          next.set(update.input.number, value);
          if (rows.get(update.input.number) !== value) store.put(update, this.tail(key) + "update:" + update.input.number);
        }
        for (const number of rows.keys()) if (!next.has(number)) store.delete(this.tail(key) + "update:" + number);
      }
      } catch (error) { transaction.abort(); reject(error); return; }
      transaction.oncomplete = () => resolve();
      transaction.onabort = () => reject(transaction.error ?? new Error("the composer update journal write was aborted"));
    });
    if (native) {
      this.bases.set(key, { checkpoint: native.checkpoint, confirmed: native.confirmed });
      this.metadata.set(key, encodedMetadata); this.rows.set(key, next);
    } else { this.bases.delete(key); this.rows.delete(key); this.metadata.delete(key); }
  }
}
