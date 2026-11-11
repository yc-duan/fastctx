import type { ComposerJournal } from "./document";
import { NativeJournalStorage } from "./native-journal";
import { DOCUMENT_JOURNAL_DATABASE, LEGACY_JOURNAL_DATABASE, journalRecoveryKeys, openJournalDatabase, readJournalDatabase, writeLegacyJournal } from "./journal-database";

export interface ComposerJournalStore {
  read(): Promise<ReadonlyMap<string, ComposerJournal>>;
  write(key: string, journal: ComposerJournal | null): Promise<void>;
  close?(): void;
}

/** Native replicas live in a different database: a released GUI's fallback
 * reader must never mistake them for a legacy draft or queued editor. Offline
 * native text is intentionally invisible after a GUI downgrade; reinstalling a
 * native-capable GUI recovers it. A legacy Edit mirror could insert it twice. */
export async function composerJournalStore(recoveryKey: string): Promise<ComposerJournalStore> {
  const legacy = await openJournalDatabase(LEGACY_JOURNAL_DATABASE);
  let documents: IDBDatabase;
  try { documents = await openJournalDatabase(DOCUMENT_JOURNAL_DATABASE); }
  catch (error) { legacy.close(); throw error; }
  const prefix = `${recoveryKey}\u0000`;
  const native = new NativeJournalStorage(documents, prefix);
  const nativeKeys = new Set<string>();
  return {
    read: async () => {
      const old = await readJournalDatabase(legacy, prefix);
      const current = new Map(await native.hydrate(await readJournalDatabase(documents, prefix)));
      const oldNative = new NativeJournalStorage(legacy, prefix);
      const previous = await oldNative.hydrate(new Map([...old].filter(([, journal]) => !!journal.native)));
      for (const [key, journal] of previous) {
        const stored = current.get(key);
        if (stored && JSON.stringify(stored.native) !== JSON.stringify(journal.native)) throw new Error("two retained native journals need reconciliation; both copies were kept");
        if (!stored) { await native.write(key, journal); current.set(key, journal); }
        // Copy first. An old GUI cannot erase the only copy while this import's
        // legacy cleanup is interrupted or fails.
        await oldNative.write(key, null); old.delete(key);
      }
      for (const [key, journal] of current) if (journal.native) nativeKeys.add(key);
      return new Map([...old, ...current]);
    },
    write: (key, journal) => {
      if (journal?.native || nativeKeys.has(key) || key.startsWith("native:")) {
        nativeKeys.add(key);
        return native.write(key, journal);
      }
      return writeLegacyJournal(legacy, prefix + key, journal);
    },
    close: () => { legacy.close(); documents.close(); },
  };
}

export async function composerRecoveries(): Promise<string[]> {
  const legacy = await openJournalDatabase(LEGACY_JOURNAL_DATABASE);
  let documents: IDBDatabase | undefined;
  try {
    documents = await openJournalDatabase(DOCUMENT_JOURNAL_DATABASE);
    return [...new Set((await Promise.all([journalRecoveryKeys(legacy), journalRecoveryKeys(documents)])).flat())];
  } finally { legacy.close(); documents?.close(); }
}
