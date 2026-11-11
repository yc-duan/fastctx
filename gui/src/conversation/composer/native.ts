// Native transport keeps a durable Yjs replica after Saved. Only exact pending
// updates leave the outbox; reconnect vectors repair either side's missing text.
import * as Y from "yjs";
import type { ComposerDraft, ComposerSnapshot, ComposerTarget, ComposerMutation, ComposerUpdated } from "../../world/composer";
import type { DocumentFrame, DocumentInput, DocumentTextInput, DocumentMutationInput, DocumentOperation, DocumentVector, SharedComposer, SharedDocument } from "../../world/composer-sync";
import type { ComposerJournal } from "./document";
import { ComposerReplica } from "./replica";
import { ANSWER_WITHIN, unanswered } from "../../world/again";
import { ALREADY_SENT, SEND_WITHDRAWN, Withdrawn } from "./send-outcome";
import { encode, decode } from "./bytes";
export { encode, decode } from "./bytes";
import { compactPending, pendingIsFull, WAITING_FOR_SYNC } from "./pending";
import { JournalFailure, JournalWrites } from "./journal-writes";

interface Hold { token: string; through: number; epoch: number; operation?: string }
export interface NativeJournal {
  /** Replica retention does not imply that a queued editor is still open. */
  editing?: boolean;
  holds?: readonly Hold[];
  document: string;
  checkpoint: string;
  confirmed?: string;
  mutations?: readonly DocumentMutationInput[];
  sent?: readonly number[];
  transmitted?: readonly string[];
  /** Imported from an earlier journal; new writes use transmitted. */
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
  storageBlocked?: boolean;
  sent: number | null;
  retry: ReturnType<typeof setTimeout> | undefined;
}

interface PendingMutation {
  input: DocumentMutationInput;
  sent: boolean;
  everTransmitted: boolean;
  cancelled: boolean;
  storageBlocked?: boolean;
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
  private readonly persistence = new JournalWrites(() => !this.disposed, () => this.changed());
  private ready = false;
  private textReady = false;
  private mutationsSupported = false;
  legacyMutate?: (mutation: ComposerMutation) => Promise<ComposerUpdated>;
  get supportsMutations(): boolean { return this.mutationsSupported; }
  private writing = false;
  private generation = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  set persist(write: (document: string) => Promise<void>) { this.persistence.save = write; }
  get persist(): (document: string) => Promise<void> { return (id) => this.persistence.persist(id); }
  set forget(write: (document: string) => Promise<void>) { this.persistence.delete = write; }
  get forget(): (document: string) => Promise<void> { return (id) => this.persistence.forget(id); }
  storageSucceeded(id: string): void { this.persistence.succeeded(id); }
  storageFailed(id: string, error: unknown): void { this.persistence.failed(id, error); }
  private retained = new Set<string>();
  private readonly pruning = new Set<string>();
  transmit: (input: DocumentInput) => Promise<void> = async () => { throw new Error("the document stream is disconnected"); };

  constructor(saved: readonly ComposerJournal[]) {
    for (const journal of saved) if (journal.native) {
      const source = journal.native;
      this.record(source.document, source);
      this.next = Math.max(this.next, source.next);
      for (const input of source.mutations ?? []) this.mutations.set(input.mutation.operation, { input, sent: false, everTransmitted: (source.transmitted ?? source.handed)?.includes(input.mutation.operation) ?? true, cancelled: false, waiters: [] });
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
      updates, everSent: new Set(saved?.sent ?? outbox.map((input) => input.number)), next: Math.max(saved?.next ?? 0, Date.now() * 1000), outbox, holds: [...(saved?.holds ?? [])], error: null, sent: null, retry: undefined };
    this.records.set(document, record);
    this.compact(record);
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
    if (pendingIsFull(record.outbox)) throw new Error(WAITING_FOR_SYNC);
  }
  private compact(record: Record): void {
    const compacted = compactPending(record.outbox, record.everSent, record.holds.map((hold) => hold.through));
    const kept = new Set(compacted.map((input) => input.number));
    for (const input of record.outbox) if (!kept.has(input.number)) record.updates.delete(input.number);
    for (const input of compacted) record.updates.set(input.number, { input, pending: true });
    record.outbox = compacted;
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
      void this.persist(document).catch(() => {});
    }, 1000);
  }
  get pending(): boolean { return !!this.mutations.size || [...this.records.values()].some((record) => !!record.outbox.length || !!record.holds.length); }
  initializeLegacy(snapshot: ComposerSnapshot, saved: readonly ComposerJournal[]): void {
    if (this.state) return;
    const retained = saved.filter((journal) => journal.native);
    const describe = (draft: ComposerDraft, id: string): SharedDocument => ({ id, revision: 0, epoch: draft.epoch,
      attachments: draft.attachments, uploads: draft.uploads, files_on: draft.files_on });
    const draft = retained.find((journal) => journal.target.kind === "draft")?.native?.document;
    if (!draft) throw new Error("the retained native draft is absent");
    const queued = (entry: ComposerSnapshot["queue"][number]) => ({ ...entry, document: describe(entry.draft,
      retained.find((journal) => journal.target.kind === "queued" && journal.target.id === entry.id)?.native?.document ?? `legacy:${entry.id}`) });
    this.state = { ...snapshot, commands: snapshot.commands ?? [], draft: describe(snapshot.draft, draft), queue: snapshot.queue.map(queued), unobserved: snapshot.unobserved.map(queued) };
  }
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
      }).catch((error: unknown) => { this.persistence.failed(id, error); }).finally(() => this.pruning.delete(id));
    }
  }
  known(): DocumentVector[] { return [...this.records].map(([document, record]) => ({ document, vector: encode(record.replica.vector()) })); }
  connect(text = true): void {
    if (this.disposed) return;
    this.textReady = text;
    if (this.ready) { this.pump(); return; }
    this.ready = true;
    for (const record of this.records.values()) { record.sent = null; record.error = null; record.storageBlocked = false; this.compact(record); }
    for (const pending of this.mutations.values()) { pending.sent = false; pending.storageBlocked = false; }
    this.changed(); this.pump();
  }
  disconnect(): void {
    this.ready = false; this.textReady = false; this.generation += 1;
    if (this.timer) clearTimeout(this.timer);
    this.timer = undefined;
    for (const record of this.records.values()) { record.sent = null; if (record.retry) clearTimeout(record.retry); record.retry = undefined; }
    for (const pending of this.mutations.values()) { pending.sent = false; pending.storageBlocked = false; }
    this.changed();
  }
  receive(frame: DocumentFrame): void {
    if (this.disposed) return;
    this.receiving = true;
    try {
    if (frame.kind === "state") {
      if (frame.version !== 1) throw new Error("this document synchronization version is not supported");
      const supported = frame.mutations === true;
      if (supported !== this.mutationsSupported) for (const pending of this.mutations.values()) pending.sent = false;
      this.mutationsSupported = supported;
      if (supported) for (const record of this.records.values()) if (record.error instanceof Error && record.error.message === "this pending captured send requires a newer node; its text is kept") record.error = null;
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
    if (operation.failure) { void this.rejectMutation(pending, operation.failure === SEND_WITHDRAWN ? new Withdrawn() : new Error(operation.failure)); return; }
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
        this.mutations.delete(pending.input.mutation.operation);
        for (const waiter of pending.waiters.splice(0)) waiter.reject(error);
        this.changed(); return;
      } catch (failure) {
        this.persistence.failed(this.state.draft.id, failure);
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
        const error = this.disposed ? new Error("the shared document is disposed") : record.error ?? this.persistence.error(document);
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
    this.compact(record); this.pump();
  }
  mayHaveTransmitted(operation: string, started = false): boolean {
    return this.mutations.get(operation)?.everTransmitted ?? started;
  }
  async withdraw(operation: string, started = false): Promise<"withdrawn" | "sent"> {
    const original = this.mutations.get(operation);
    if (original?.everTransmitted || !original && started) {
      if (!this.ready || !this.supportsMutations) throw new Error("the node cannot confirm withdrawal; this send remains unknown");
      const existing = [...this.mutations.values()].find((pending) => pending.input.mutation.do === "withdraw" && pending.input.mutation.send === operation);
      const mutation: ComposerMutation = existing?.input.mutation ?? { operation: crypto.randomUUID(), do: "withdraw", send: operation };
      const controller = new AbortController();
      const disconnected = () => { if (!this.ready || this.disposed) controller.abort(); };
      this.listeners.add(disconnected);
      const timer = setTimeout(() => controller.abort(), ANSWER_WITHIN);
      try { await this.mutate(mutation, controller.signal); }
      catch (error) { if (error instanceof Error && error.message === ALREADY_SENT) return "sent"; throw error; }
      finally { clearTimeout(timer); this.listeners.delete(disconnected); }
    }
    if (original) await this.rejectMutation(original, new Withdrawn());
    for (const record of this.records.values()) {
      record.holds = record.holds.filter((hold) => hold.operation !== operation); this.compact(record);
    }
    this.changed(); this.pump();
    return "withdrawn";
  }
  mutate(mutation: ComposerMutation, signal?: AbortSignal): Promise<ComposerUpdated> {
    let pending = this.mutations.get(mutation.operation);
    if (!pending) {
      const record = this.record(this.state.draft.id);
      record.next = this.next = Math.max(this.next, record.next) + 1;
      if (this.mutations.size >= 128) return Promise.reject(new Error("this input has too many operations waiting to synchronize"));
      pending = { input: { kind: "mutate", number: this.next, mutation }, sent: false, everTransmitted: false, cancelled: false, waiters: [] };
      this.mutations.set(mutation.operation, pending);
    } else if (JSON.stringify(pending.input.mutation) !== JSON.stringify(mutation)) return Promise.reject(new Error("a composer operation was reused for different content"));
    pending.sent = false;
    let abort = () => {};
    const result = new Promise<ComposerUpdated>((resolve, reject) => {
      const waiter = { resolve, reject }; pending!.waiters.push(waiter);
      abort = () => {
        const at = pending!.waiters.indexOf(waiter); if (at !== -1) pending!.waiters.splice(at, 1);
        reject(Object.assign(new Error("the withdrawal could not be confirmed; the send remains unknown"), { again: true }));
      };
      signal?.addEventListener("abort", abort, { once: true });
      if (signal?.aborted) abort();
    });
    this.changed(); this.pump();
    return result.finally(() => signal?.removeEventListener("abort", abort));
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
    for (const record of this.records.values()) { record.error = null; record.sent = null; record.storageBlocked = false; this.compact(record); }
    for (const pending of this.mutations.values()) { pending.sent = false; pending.storageBlocked = false; }
    this.pump();
  }
  replica(document: string): ComposerReplica { return this.record(document).replica; }
  status(document: string): { pending: number; error: unknown } { const record = this.record(document); return { pending: record.outbox.length, error: this.persistence.error(document) ?? record.error ?? (pendingIsFull(record.outbox) ? new Error(WAITING_FOR_SYNC) : null) }; }
  journal(document: string): NativeJournal {
    const record = this.record(document);
    return { document, checkpoint: record.checkpoint, confirmed: record.confirmedCheckpoint, updates: [...record.updates.values()], sent: [...record.everSent], next: record.next, outbox: [...record.outbox], holds: record.holds.filter((hold) => hold.operation && this.mutations.has(hold.operation)), ...(document === this.state.draft.id ? { mutations: [...this.mutations.values()].filter((pending) => !pending.cancelled).map((pending) => pending.input), transmitted: [...this.mutations.values()].filter((pending) => pending.everTransmitted).map((pending) => pending.input.mutation.operation) } : {}) };
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
  private withdrawing(pending: PendingMutation): boolean {
    return pending.input.mutation.do !== "withdraw" && [...this.mutations.values()].some((other) => !other.cancelled
      && other.input.mutation.do === "withdraw" && other.input.mutation.send === pending.input.mutation.operation);
  }
  private pump(): void {
    if (this.disposed || !this.ready || this.writing) return;
    this.writing = true;
    const generation = this.generation;
    let writingDocument = this.state.draft.id;
    void (async () => {
      for (const [document, record] of this.records) {
        writingDocument = document;
        if (!this.textReady) continue;
        const input = record.outbox[0];
        if (!input || record.sent !== null || record.error || record.storageBlocked || record.holds.some((hold) => input.number > hold.through)) continue;
        try {
          // Persist the exact update, its number, and the independent replica
          // before anything can cross the IPC connection.
          record.everSent.add(input.number);
          await this.persist(document);
          if (!this.ready || generation !== this.generation) return;
          record.sent = input.number;
          await this.transmit(input);
        } catch (error) {
          record.sent = null; record.retryable = true;
          if (error instanceof JournalFailure) record.storageBlocked = true; else record.error = error;
          this.changed();
        }
      }
      for (const pending of this.mutations.values()) {
        writingDocument = this.state.draft.id;
        if (pending.sent || pending.cancelled || pending.storageBlocked || this.withdrawing(pending)) continue;
        if (!this.supportsMutations && (pending.input.mutation.do === "send_document" || pending.input.mutation.do === "withdraw" || !this.legacyMutate)) {
          pending.sent = true;
          this.record(this.state.draft.id).error = new Error("this pending captured send requires a newer node; its text is kept");
          this.changed(); continue;
        }
        try {
          pending.everTransmitted = true;
          await this.persist(this.state.draft.id);
          if (!this.ready || generation !== this.generation) return;
          pending.sent = true;
          if (this.supportsMutations) await this.transmit(pending.input);
          else {
            const updated = await this.legacyMutate!(pending.input.mutation);
            if (this.disposed || pending.cancelled) continue;
            pending.result = { revision: updated.composer.revision, operation: {
              operation: pending.input.mutation.operation, accepted: false, landed: updated.landed, delivery: updated.delivery ?? null, failure: null,
            } };
            if (pending.result.revision <= this.state.revision) this.settle(pending.result.operation);
          }
        } catch (error) {
          pending.sent = !(error instanceof JournalFailure);
          if (error instanceof JournalFailure) pending.storageBlocked = true;
          else if (unanswered(error)) for (const waiter of pending.waiters.splice(0)) waiter.reject(error);
          else await this.rejectMutation(pending, error);
        }
      }
    })().finally(() => {
      this.writing = false;
      if (this.ready && this.textReady && [...this.records.values()].some((record) => record.outbox.length && record.sent === null && !record.error && !record.storageBlocked && !record.holds.some((hold) => record.outbox[0].number > hold.through)) || this.ready && [...this.mutations.values()].some((pending) => !pending.sent && !pending.cancelled && !pending.storageBlocked && !this.withdrawing(pending))) queueMicrotask(() => this.pump());
      if (this.ready && !this.timer && ([...this.mutations.values()].some((pending) => !pending.cancelled) || [...this.records.values()].some((record) => record.outbox.length && (!record.error || record.retryable)))) this.timer = setTimeout(() => {
        this.timer = undefined;
        for (const record of this.records.values()) { record.sent = null; record.storageBlocked = false; if (record.retryable) record.error = null; }
        for (const pending of this.mutations.values()) { pending.sent = false; pending.storageBlocked = false; }
        this.pump();
      }, 30_000);
    }).catch((error: unknown) => { this.persistence.failed(writingDocument, error); });
  }
  retainedJournals(): ReadonlyMap<string, ComposerJournal> {
    const snapshot = this.snapshot(false);
    const targets = new Map<string, ComposerTarget>([[this.state.draft.id, { kind: "draft" }],
      ...[...this.state.queue, ...this.state.unobserved].map((entry) => [entry.document.id, { kind: "queued", id: entry.id }] as [string, ComposerTarget])]);
    return new Map([...this.records.keys()].map((id) => [`native:replica:${id}`, {
      snapshot, target: targets.get(id) ?? { kind: "queued", id }, pending: [], contexts: [], history: [], retired: [], native: this.journal(id),
    }]));
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
