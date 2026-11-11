// Native transport keeps a durable Yjs replica after Saved. Only exact pending
// updates leave the outbox; reconnect vectors repair either side's missing text.
import * as Y from "yjs";
import type { ComposerDraft, ComposerSnapshot, ComposerTarget, ComposerMutation, ComposerUpdated } from "../../world/composer";
import type { DocumentFrame, DocumentInput, DocumentTextInput, DocumentMutationInput, DocumentOperation, DocumentVector, SharedComposer, SharedDocument } from "../../world/composer-sync";
import type { ComposerJournal } from "./document";
import { ComposerReplica } from "./replica";
import { unanswered } from "../../world/again";

interface Hold { token: string; through: number; epoch: number; operation?: string }
export interface NativeJournal {
  /** Replica retention does not imply that a queued editor is still open. */
  editing?: boolean;
  holds?: readonly Hold[];
  document: string;
  checkpoint: string;
  confirmed?: string;
  mutations?: readonly DocumentMutationInput[];
  /** The operations of `mutations` handed to the node at least once: what
   *  became of them is the node's to say. */
  handed?: readonly string[];
  next: number;
  outbox: DocumentTextInput[];
  updates?: readonly { input: DocumentTextInput; pending: boolean }[];
}
interface Record {
  checkpoint: string;
  confirmedCheckpoint: string;
  updates: Map<number, { input: DocumentTextInput; pending: boolean }>;
  everSent: Set<number>;
  checkpointTimer?: ReturnType<typeof setTimeout>;
  replica: ComposerReplica;
  confirmed: Y.Doc;
  next: number;
  outbox: DocumentTextInput[];
  holds: Hold[];
  error: unknown;
  retryable?: boolean;
  sent: number | null;
  retry: ReturnType<typeof setTimeout> | undefined;
}
export const encode = (bytes: Uint8Array): string => {
  let value = "";
  for (let at = 0; at < bytes.length; at += 8192) value += String.fromCharCode(...bytes.subarray(at, at + 8192));
  return btoa(value);
};
export const decode = (value: string): Uint8Array => Uint8Array.from(atob(value), (character) => character.charCodeAt(0));

interface PendingMutation {
  input: DocumentMutationInput;
  sent: boolean;
  /** It has gone over the connection at least once, in this page or one before it. */
  handed: boolean;
  cancelled: boolean;
  result?: { revision: number; operation: DocumentOperation };
  removing?: Promise<void>;
  waiters: { resolve: (result: ComposerUpdated) => void; reject: (error: unknown) => void }[];
}
export class NativeComposer {
  private next = Date.now() * 1000;
  private readonly mutations = new Map<string, PendingMutation>();
  private readonly records = new Map<string, Record>();
  private state!: SharedComposer;
  private readonly listeners = new Set<() => void>();
  private disposed = false;
  private receiving = false;
  private storageError: unknown = null;
  private ready = false;
  private textReady = false;
  private writing = false;
  private generation = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  persist: (document: string) => Promise<void> = async () => {};
  forget: (document: string) => Promise<void> = async () => {};
  private retained = new Set<string>();
  private readonly pruning = new Set<string>();
  transmit: (input: DocumentInput) => Promise<void> = async () => { throw new Error("the document stream is disconnected"); };

  constructor(saved: readonly ComposerJournal[]) {
    for (const journal of saved) if (journal.native) {
      const source = journal.native;
      this.record(source.document, source);
      this.next = Math.max(this.next, source.next);
      // One kept without saying whether it was handed over may have been.
      for (const input of source.mutations ?? []) this.mutations.set(input.mutation.operation, { input, sent: false, handed: source.handed ? source.handed.includes(input.mutation.operation) : true, cancelled: false, waiters: [] });
    }
  }
  private record(document: string, saved?: NativeJournal): Record {
    const old = this.records.get(document);
    if (old) return old;
    const replica = new ComposerReplica(document, saved ? decode(saved.checkpoint) : undefined);
    const confirmed = new Y.Doc();
    if (saved) Y.applyUpdate(confirmed, decode(saved.confirmed ?? saved.checkpoint));
    const updates = new Map((saved?.updates?.length ? saved.updates : (saved?.outbox ?? []).map((input) => ({ input, pending: true }))).map((row) => [row.input.number, row]));
    for (const row of updates.values()) {
      replica.apply(decode(row.input.update));
      if (!row.pending) Y.applyUpdate(confirmed, decode(row.input.update));
    }
    const outbox = [...(saved?.outbox ?? [])];
    const record: Record = { replica, confirmed, checkpoint: saved?.checkpoint ?? encode(replica.snapshot()), confirmedCheckpoint: saved?.confirmed ?? saved?.checkpoint ?? encode(Y.encodeStateAsUpdate(confirmed)),
      updates, everSent: new Set(outbox.map((input) => input.number)), next: Math.max(saved?.next ?? 0, Date.now() * 1000), outbox, holds: [...(saved?.holds ?? [])], error: null, sent: null, retry: undefined };
    this.records.set(document, record);
    replica.subscribe(({ update, local }) => {
      if (local) this.enqueue(document, update);
      this.changed();
      if (local) this.pump();
    });
    return record;
  }
  private enqueue(document: string, update: Uint8Array): void {
    const record = this.record(document);
    const last = record.outbox.at(-1);
    if (last && !record.everSent.has(last.number) && !record.holds.some((hold) => last.number <= hold.through)) {
      const merged = Y.mergeUpdates([decode(last.update), update]);
      if (merged.length <= 1024 * 1024) {
        const input = { ...last, update: encode(merged) };
        record.outbox[record.outbox.length - 1] = input;
        record.updates.set(input.number, { input, pending: true });
        this.checkpointLater(document); return;
      }
    }
    record.next = this.next = Math.max(this.next, record.next) + 1;
    const input: DocumentTextInput = { kind: "update", number: record.next, document, update: encode(update) };
    record.outbox.push(input); record.updates.set(input.number, { input, pending: true });
    this.checkpointLater(document);
  }
  assertEditable(document: string): void {
    const record = this.record(document);
    if (this.disposed) throw new Error("the shared document is disposed");
    if (record.outbox.length >= 256 || record.outbox.reduce((bytes, input) => bytes + input.update.length, 0) > 8 * 1024 * 1024 - Math.ceil(1024 * 1024 / 3) * 4) {
      record.error = new Error("this input is waiting to synchronize; reconnect before adding more text");
      this.changed(); throw record.error;
    }
  }
  private remote(document: string, update: string): void {
    const record = this.record(document);
    const bytes = decode(update);
    Y.applyUpdate(record.confirmed, bytes); record.replica.apply(bytes);
    if (bytes.length <= 2 || record.outbox.some((input) => input.update === update)) return;
    record.next = this.next = Math.max(this.next, record.next) + 1;
    const input: DocumentTextInput = { kind: "update", number: record.next, document, update };
    record.updates.set(input.number, { input, pending: false });
    this.checkpointLater(document);
  }
  private checkpointLater(document: string): void {
    const record = this.record(document);
    if (record.checkpointTimer || this.disposed) return;
    record.checkpointTimer = setTimeout(() => {
      delete record.checkpointTimer;
      if (this.disposed) return;
      record.checkpoint = encode(record.replica.snapshot());
      record.confirmedCheckpoint = encode(Y.encodeStateAsUpdate(record.confirmed));
      for (const [number, row] of record.updates) if (!row.pending) record.updates.delete(number);
      void this.persist(document).catch((error: unknown) => { this.storageError = error; this.changed(); });
    }, 1000);
  }
  get pending(): boolean { return !!this.mutations.size || [...this.records.values()].some((record) => !!record.outbox.length || !!record.holds.length); }
  async drainForLegacy(sync: (input: DocumentTextInput) => Promise<ComposerUpdated>): Promise<void> {
    if (this.mutations.size || [...this.records.values()].some((record) => record.holds.length)) {
      throw new Error("the pending captured send requires a newer node; its local text is kept");
    }
    for (;;) {
      const next = [...this.records.values()].find((record) => record.outbox.length);
      if (!next) return;
      const input = next.outbox[0]; next.everSent.add(input.number);
      await this.persist(input.document);
      const updated = await sync(input);
      // Request::Sync on older nodes returns only after update_document's Saved
      // and the following Read. Convert that commit proof at this boundary.
      this.saved(input.document, input.number);
      this.state = { ...this.state, revision: Math.max(this.state.revision, updated.composer.revision) };
      this.changed();
      await this.persist(input.document);
    }
  }
  retain(editors: readonly string[]): void {
    if (this.disposed || !this.state) return;
    this.retained = new Set([this.state.draft.id, ...this.state.queue.map((entry) => entry.document.id), ...this.state.unobserved.map((entry) => entry.document.id), ...editors]);
    for (const [id, record] of this.records) {
      if (this.retained.has(id) || record.outbox.length || record.holds.length || this.pruning.has(id)) continue;
      this.pruning.add(id);
      if (record.checkpointTimer) { clearTimeout(record.checkpointTimer); delete record.checkpointTimer; }
      void this.forget(id).then(async () => {
        if (this.disposed) return;
        if (this.retained.has(id) || record.outbox.length || record.holds.length) { await this.persist(id); return; }
        if (record.checkpointTimer) clearTimeout(record.checkpointTimer);
        record.replica.dispose(); record.confirmed.destroy(); this.records.delete(id);
      }).catch((error: unknown) => { this.storageError = error; this.changed(); }).finally(() => this.pruning.delete(id));
    }
  }
  known(): DocumentVector[] { return [...this.records].map(([document, record]) => ({ document, vector: encode(record.replica.vector()) })); }
  connect(text = true): void {
    if (this.disposed) return;
    this.textReady = text;
    if (this.ready) { this.pump(); return; }
    this.ready = true;
    for (const record of this.records.values()) { record.sent = null; record.error = null; }
    for (const pending of this.mutations.values()) pending.sent = false;
    this.changed(); this.pump();
  }
  disconnect(): void {
    this.ready = false; this.textReady = false; this.generation += 1;
    if (this.timer) clearTimeout(this.timer);
    this.timer = undefined;
    for (const record of this.records.values()) { record.sent = null; if (record.retry) clearTimeout(record.retry); record.retry = undefined; }
    for (const pending of this.mutations.values()) pending.sent = false;
  }
  receive(frame: DocumentFrame): void {
    if (this.disposed) return;
    this.receiving = true;
    try {
    if (frame.kind === "state") {
      if (frame.version !== 1) throw new Error("this document synchronization version is not supported");
      this.state = frame.state;
      for (const document of [frame.state.draft, ...frame.state.queue.map((entry) => entry.document)]) {
        const record = this.record(document.id);
        record.holds = record.holds.filter((hold) => hold.epoch >= document.epoch);
      }
      for (const update of frame.documents) {
        this.remote(update.document, update.update);
      }
      // A state vector alone omits deletions. The update includes the retained
      // delete set as well, so either side can repair after a lost disk flush.
      for (const vector of frame.vectors ?? []) {
        const record = this.record(vector.document);
        const missing = Y.encodeStateAsUpdate(record.confirmed, decode(vector.vector));
        if (missing.length > 2 && !record.outbox.some((one) => one.update === encode(missing))) this.enqueue(vector.document, missing);
      }
    } else if (frame.kind === "update") {
      this.remote(frame.document.document, frame.document.update);
      this.state = { ...this.state, revision: Math.max(this.state.revision, frame.revision) };
    } else if (frame.kind === "saved") {
      this.saved(frame.document, frame.number);
    } else if (frame.kind === "result") {
      const pending = this.mutations.get(frame.operation.operation);
      if (pending) pending.result = { revision: frame.revision, operation: frame.operation };
    } else {
      const pending = [...this.mutations.values()].find((pending) => pending.input.number === frame.number);
      if (pending) {
        const again = frame.failure.not_run === true || frame.failure.code === "unanswered" || frame.failure.code === "busy";
        const error = Object.assign(new Error(frame.failure.message), { again });
        if (!again) void this.rejectMutation(pending, error);
        else {
          pending.sent = true;
          for (const waiter of pending.waiters.splice(0)) waiter.reject(error);
        }
      }
      for (const record of this.records.values()) if (record.outbox.some((one) => one.number === frame.number)) {
        record.sent = null;
        record.error = new Error(frame.failure.message);
        if (frame.failure.not_run === true || frame.failure.code === "unanswered" || frame.failure.code === "busy") {
          if (record.retry) clearTimeout(record.retry);
          record.retry = setTimeout(() => {
            record.retry = undefined;
            if (!this.ready) return;
            record.error = null; this.changed(); this.pump();
          }, 500);
        }
      }
    }
    for (const operation of frame.kind === "state" ? frame.state.operations ?? [] : []) this.settle(operation);
    for (const pending of this.mutations.values()) if (pending.result && pending.result.revision <= this.state.revision) this.settle(pending.result.operation);
    if (![...this.records.values()].some((record) => record.outbox.length) && !this.mutations.size && this.timer) {
      clearTimeout(this.timer); this.timer = undefined;
    }
    } finally { this.receiving = false; }
    this.changed(); this.pump();
  }
  private saved(document: string, number: number): void {
    const record = this.record(document);
    const at = record.outbox.findIndex((one) => one.number === number);
    if (at !== -1) {
      const input = record.outbox[at]; Y.applyUpdate(record.confirmed, decode(input.update)); record.outbox.splice(at, 1);
      record.updates.set(input.number, { input, pending: false }); record.everSent.delete(input.number);
      this.checkpointLater(document);
    }
    if (record.sent === number) record.sent = null;
    record.error = null;
  }
  private settle(operation: DocumentOperation): void {
    if (operation.accepted) return;
    const pending = this.mutations.get(operation.operation);
    if (!pending) return;
    if (pending.cancelled) return;
    if (operation.failure) { void this.rejectMutation(pending, new Error(operation.failure)); return; }
    this.mutations.delete(operation.operation);
    for (const waiter of pending.waiters) waiter.resolve({ composer: this.snapshot(), landed: operation.landed, ...(operation.delivery ? { delivery: operation.delivery } : {}) });
  }
  private rejectMutation(pending: PendingMutation, error: unknown): Promise<void> {
    return pending.removing ??= this.removeMutation(pending, error);
  }
  private async removeMutation(pending: PendingMutation, error: unknown): Promise<void> {
    // A definite failure cannot remain replayable after the caller restores its
    // captured text. Remove it from the persisted outbox before notifying callers.
    pending.cancelled = true;
    while (!this.disposed) {
      try {
        await this.persist(this.state.draft.id);
        this.storageError = null;
        this.mutations.delete(pending.input.mutation.operation);
        for (const waiter of pending.waiters.splice(0)) waiter.reject(error);
        this.changed(); return;
      } catch (failure) {
        this.storageError = failure; this.changed();
        // Until cancellation is durable, keep the capture sealed and visibly
        // blocked. Returning a definite refusal sooner could revive it on reopen.
        await new Promise((resolve) => setTimeout(resolve, 1000));
      }
    }
  }
  flushThrough(document: string, token: string): Promise<void> {
    const record = this.record(document);
    const through = record.holds.find((hold) => hold.token === token)?.through;
    if (through === undefined) return Promise.reject(new Error("the captured input is no longer held"));
    return new Promise((resolve, reject) => {
      const check = () => {
        const error = this.disposed ? new Error("the shared document is disposed") : record.error ?? this.storageError;
        if (error || !record.outbox.some((input) => input.number <= through)) {
          this.listeners.delete(check);
          if (error) reject(error); else resolve();
        }
      };
      this.listeners.add(check); check(); this.pump();
    });
  }
  hold(document: string, epoch: number): string {
    const record = this.record(document);
    const token = crypto.randomUUID();
    record.holds.push({ token, through: record.next, epoch });
    return token;
  }
  bind(document: string, token: string, operation: string): void {
    const hold = this.record(document).holds.find((hold) => hold.token === token);
    if (hold) hold.operation = operation;
  }
  release(document: string, token: string): void {
    const record = this.record(document);
    record.holds = record.holds.filter((hold) => hold.token !== token);
    this.pump();
  }
  /** Whether `operation` waits here and was handed to the node: one that
   *  never left can be taken back without asking. */
  handed(operation: string): boolean { return this.mutations.get(operation)?.handed ?? false; }
  async withdraw(operation: string): Promise<void> {
    const pending = this.mutations.get(operation);
    if (!pending) return;
    pending.cancelled = true;
    this.changed();
    // Complete withdrawal only after reopening the page can no longer replay it.
    // Already-transmitted work can still commit under its original receipt.
    try { await this.persist(this.state.draft.id); }
    catch (error) { void this.rejectMutation(pending, new Error("the send was withdrawn")); throw error; }
    this.mutations.delete(operation);
    for (const waiter of pending.waiters.splice(0)) waiter.reject(new Error("the send was withdrawn"));
    this.changed();
  }
  mutate(mutation: ComposerMutation): Promise<ComposerUpdated> {
    let pending = this.mutations.get(mutation.operation);
    if (!pending) {
      const record = this.record(this.state.draft.id);
      record.next = this.next = Math.max(this.next, record.next) + 1;
      if (this.mutations.size >= 128) return Promise.reject(new Error("this input has too many operations waiting to synchronize"));
      pending = { input: { kind: "mutate", number: this.next, mutation }, sent: false, handed: false, cancelled: false, waiters: [] };
      this.mutations.set(mutation.operation, pending);
    } else if (JSON.stringify(pending.input.mutation) !== JSON.stringify(mutation)) return Promise.reject(new Error("a composer operation was reused for different content"));
    pending.sent = false;
    const result = new Promise<ComposerUpdated>((resolve, reject) => { pending!.waiters.push({ resolve, reject }); });
    this.changed(); this.pump();
    return result;
  }
  document(target: ComposerTarget): string {
    const document = target.kind === "draft" ? this.state.draft : this.state.queue.find((entry) => entry.id === target.id)?.document;
    if (!document) throw new Error("the composer target is absent");
    return document.id;
  }
  discard(document: string): void {
    const record = this.record(document);
    // The caller has already preserved the visible text. Applying the inverse
    // visible state is not safe after a permanent identity or permission refusal.
    const checkpoint = Y.encodeStateAsUpdate(record.confirmed);
    const next = record.next;
    record.replica.dispose(); record.confirmed.destroy();
    this.records.delete(document);
    this.record(document, { document, checkpoint: encode(checkpoint), confirmed: encode(checkpoint), next, outbox: [] });
    this.changed();
  }
  retry(): void {
    for (const record of this.records.values()) { record.error = null; record.sent = null; }
    for (const pending of this.mutations.values()) pending.sent = false;
    this.pump();
  }
  replica(document: string): ComposerReplica { return this.record(document).replica; }
  status(document: string): { pending: number; error: unknown } { const record = this.record(document); return { pending: record.outbox.length, error: this.storageError ?? record.error }; }
  journal(document: string): NativeJournal {
    const record = this.record(document);
    return { document, checkpoint: record.checkpoint, confirmed: record.confirmedCheckpoint, updates: [...record.updates.values()], next: record.next, outbox: [...record.outbox], holds: record.holds.filter((hold) => hold.operation && this.mutations.has(hold.operation)), ...(document === this.state.draft.id ? { mutations: [...this.mutations.values()].filter((pending) => !pending.cancelled).map((pending) => pending.input),
        handed: [...this.mutations.values()].filter((pending) => !pending.cancelled && pending.handed).map((pending) => pending.input.mutation.operation) } : {}) };
  }
  snapshot(text = true): ComposerSnapshot {
    const draft = (metadata: SharedDocument): ComposerDraft => {
      const characters = text ? this.record(metadata.id).replica.characters : [];
      return { ...metadata, text: text ? this.record(metadata.id).replica.content : "", characters };
    };
    const queued = (entry: SharedComposer["queue"][number]) => ({ ...entry, draft: draft(entry.document) });
    return { ...this.state, draft: draft(this.state.draft), queue: this.state.queue.map(queued), unobserved: this.state.unobserved.map(queued) };
  }
  subscribe(listener: () => void): () => void { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
  private changed(): void {
    if (this.receiving || !this.state) return;
    for (const listener of this.listeners) {
      try { listener(); } catch (error) { console.error("a shared composer observer failed", error); }
    }
  }
  private pump(): void {
    if (this.disposed || !this.ready || this.writing) return;
    this.writing = true;
    const generation = this.generation;
    void (async () => {
      for (const [document, record] of this.records) {
        if (!this.textReady) continue;
        const input = record.outbox[0];
        if (!input || record.sent !== null || record.error || record.holds.some((hold) => input.number > hold.through)) continue;
        try {
          // Persist the exact update, its number, and the independent replica
          // before anything can cross the IPC connection.
          record.everSent.add(input.number);
          await this.persist(document);
          this.storageError = null;
          if (!this.ready || generation !== this.generation) return;
          record.sent = input.number;
          await this.transmit(input);
        } catch (error) { record.sent = null; record.error = error; record.retryable = true; this.changed(); }
      }
      for (const pending of this.mutations.values()) {
        if (pending.sent || pending.cancelled) continue;
        try {
          // That it is handed over is kept before it is: taking it back
          // from then on is asked of the node.
          pending.handed = true;
          await this.persist(this.state.draft.id);
          if (!this.ready || generation !== this.generation) return;
          pending.sent = true;
          await this.transmit(pending.input);
        } catch (error) {
          pending.sent = true;
          if (unanswered(error)) for (const waiter of pending.waiters.splice(0)) waiter.reject(error);
          else await this.rejectMutation(pending, error);
        }
      }
    })().finally(() => {
      this.writing = false;
      if (this.ready && this.textReady && [...this.records.values()].some((record) => record.outbox.length && record.sent === null && !record.error && !record.holds.some((hold) => record.outbox[0].number > hold.through)) || this.ready && [...this.mutations.values()].some((pending) => !pending.sent && !pending.cancelled)) queueMicrotask(() => this.pump());
      if (this.ready && !this.timer && ([...this.mutations.values()].some((pending) => !pending.cancelled) || [...this.records.values()].some((record) => record.outbox.length && (!record.error || record.retryable)))) this.timer = setTimeout(() => {
        this.timer = undefined;
        for (const record of this.records.values()) { record.sent = null; if (record.retryable) record.error = null; }
        for (const pending of this.mutations.values()) pending.sent = false;
        this.pump();
      }, 30_000);
    }).catch((error: unknown) => { this.storageError = error; this.changed(); });
  }
  async flushRetained(): Promise<void> {
    for (const record of this.records.values()) if (record.checkpointTimer) { clearTimeout(record.checkpointTimer); delete record.checkpointTimer; }
    await Promise.all([...this.records.keys()].map((id) => this.persist(id)));
  }
  dispose(): void {
    this.disposed = true;
    this.disconnect(); this.changed();
    for (const pending of this.mutations.values()) for (const waiter of pending.waiters.splice(0)) waiter.reject(Object.assign(new Error("the composer is closed"), { again: true }));
    for (const record of this.records.values()) { if (record.checkpointTimer) clearTimeout(record.checkpointTimer); record.replica.dispose(); record.confirmed.destroy(); }
    this.listeners.clear(); this.records.clear(); this.mutations.clear();
  }
}
