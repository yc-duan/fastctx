import type { ComposerJournal } from "./document";

export const LEGACY_JOURNAL_DATABASE = "world-composer-outbox";
export const DOCUMENT_JOURNAL_DATABASE = "world-composer-documents";

export function openJournalDatabase(name: string): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    let finished = false;
    const request = indexedDB.open(name, 1);
    request.onupgradeneeded = () => { request.result.createObjectStore("edits"); };
    request.onsuccess = () => { if (finished) request.result.close(); else { finished = true; resolve(request.result); } };
    request.onerror = () => { finished = true; reject(request.error); };
    request.onblocked = () => { finished = true; reject(new Error("the composer journal database is blocked")); };
  });
}

export function readJournalDatabase(db: IDBDatabase, prefix: string): Promise<Map<string, ComposerJournal>> {
  return new Promise((resolve, reject) => {
    const transaction = db.transaction("edits", "readonly");
    const request = transaction.objectStore("edits").openCursor(IDBKeyRange.bound(prefix, `${prefix}\uffff`));
    const entries = new Map<string, ComposerJournal>();
    request.onsuccess = () => { const cursor = request.result; if (cursor) { entries.set(String(cursor.key).slice(prefix.length), cursor.value as ComposerJournal); cursor.continue(); } };
    transaction.oncomplete = () => resolve(entries);
    transaction.onabort = () => reject(transaction.error ?? new Error("the composer journal read was aborted"));
  });
}

export function writeLegacyJournal(db: IDBDatabase, key: string, journal: ComposerJournal | null): Promise<void> {
  if (journal?.native) return Promise.reject(new Error("native documents do not belong in the legacy journal"));
  return new Promise((resolve, reject) => {
    const transaction = db.transaction("edits", "readwrite");
    try {
      const store = transaction.objectStore("edits");
      if (journal) store.put(journal, key); else store.delete(key);
    } catch (error) { transaction.abort(); reject(error); return; }
    transaction.oncomplete = () => resolve();
    transaction.onabort = () => reject(transaction.error ?? new Error("the composer journal write was aborted"));
  });
}

export function journalRecoveryKeys(db: IDBDatabase): Promise<string[]> {
  return new Promise((resolve, reject) => {
    const transaction = db.transaction("edits", "readonly");
    const request = transaction.objectStore("edits").getAllKeys();
    transaction.oncomplete = () => resolve(request.result.filter((key) => !String(key).startsWith("\u0001")).map((key) => String(key).split("\u0000")[0]));
    transaction.onabort = () => reject(transaction.error ?? new Error("the composer journal recovery read was aborted"));
  });
}
