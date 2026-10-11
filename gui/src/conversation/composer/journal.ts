import type { ComposerJournal } from "./document";

/** The local outbox holds only unacknowledged edits. Shared drafts and queues live on the node. */
export interface ComposerJournalStore {
  read(): Promise<ReadonlyMap<string, ComposerJournal>>;
  write(key: string, journal: ComposerJournal | null): Promise<void>;
}

function database(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open("world-composer-outbox", 1);
    request.onupgradeneeded = () => { request.result.createObjectStore("edits"); };
    request.onsuccess = () => { resolve(request.result); };
    request.onerror = () => { reject(request.error); };
    request.onblocked = () => { reject(new Error("the composer outbox database is blocked")); };
  });
}

/** A recovery key identifies one writer's outbox. Do not share it between live pages. */
export async function composerJournalStore(recoveryKey: string): Promise<ComposerJournalStore> {
  const db = await database();
  const prefix = `${recoveryKey}\u0000`;
  const range = IDBKeyRange.bound(prefix, `${prefix}\uffff`);
  return {
    read: () => new Promise((resolve, reject) => {
      const transaction = db.transaction("edits", "readonly");
      const request = transaction.objectStore("edits").openCursor(range);
      const entries = new Map<string, ComposerJournal>();
      request.onsuccess = () => {
        const cursor = request.result;
        if (!cursor) return;
        entries.set(String(cursor.key).slice(prefix.length), cursor.value as ComposerJournal);
        cursor.continue();
      };
      transaction.oncomplete = () => { resolve(entries); };
      transaction.onabort = () => { reject(transaction.error ?? new Error("the composer outbox read was aborted")); };
    }),
    write: (key, journal) => new Promise((resolve, reject) => {
      const transaction = db.transaction("edits", "readwrite");
      const store = transaction.objectStore("edits");
      if (journal) store.put(journal, prefix + key);
      else store.delete(prefix + key);
      transaction.oncomplete = () => { resolve(); };
      transaction.onabort = () => { reject(transaction.error ?? new Error("the composer outbox write was aborted")); };
    }),
  };
}

/** Lists retained writer outboxes, including those from closed tabs. The caller
 * may reopen one with its recoveryKey after its previous writer has closed. */
export async function composerRecoveries(): Promise<string[]> {
  const db = await database();
  try {
    return await new Promise((resolve, reject) => {
      const transaction = db.transaction("edits", "readonly");
      const request = transaction.objectStore("edits").getAllKeys();
      transaction.oncomplete = () => { resolve([...new Set(request.result.map((key) => String(key).split("\u0000")[0]))]); };
      transaction.onabort = () => { reject(transaction.error ?? new Error("the composer outbox read was aborted")); };
    });
  } finally { db.close(); }
}
