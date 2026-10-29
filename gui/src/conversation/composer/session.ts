import type { Address } from "../../world/wire";
import type { Attachment } from "../../world/attachments";
import type { ComposerAttachmentAdded, ComposerChange, ComposerMutation, ComposerPiece, ComposerSnapshot, ComposerTarget, ComposerUpdated } from "../../world/composer";
import { composing } from "../../world/composing";
import type { ComposerTransport } from "../../world/composing";
import { ComposerDocument } from "./document";
import type { ComposerJournal } from "./document";
import { composerJournalStore } from "./journal";
import type { ComposerJournalStore } from "./journal";
import { composerWriter } from "./writer";
import { after, unanswered } from "../../world/again";

export interface ComposerOperation {
  readonly id: string;
  readonly change: ComposerChange | { readonly do: "upload"; readonly target: ComposerTarget; readonly epoch: number };
  readonly status: "pending" | "failed" | "completed";
  readonly error: Error | null;
  readonly result: ComposerUpdated | ComposerAttachmentAdded | null;
  /** Failed for want of an answer, and goes again by itself. */
  readonly broken: boolean;
}

export interface ComposerSessionState {
  readonly composer: ComposerSnapshot;
  readonly connected: boolean;
  readonly error: Error | null;
  readonly operations: readonly ComposerOperation[];
  readonly editors: readonly ComposerDocument[];
  readonly recoveryKey: string;
  readonly saving: boolean;
  readonly storageError: Error | null;
  readonly canSend: boolean;
}

export interface ComposerSessionOptions {
  readonly transport?: ComposerTransport;
  /** Stable per writer, never shared by two live pages. The default survives a tab reload. */
  readonly recoveryKey?: string;
  readonly journal?: ComposerJournalStore;
}

interface Operation extends Omit<ComposerOperation, "broken"> {
  status: ComposerOperation["status"];
  error: Error | null;
  result: ComposerOperation["result"];
  run: () => Promise<ComposerUpdated | ComposerAttachmentAdded>;
  flight?: Promise<ComposerUpdated | ComposerAttachmentAdded>;
  attempt: number;
  reject?: (error: Error) => void;
  /** It failed for want of an answer, not by the node's refusal: it goes
   *  again, as it was, once the shared input is back. */
  broken?: boolean;
}

interface DocumentEntry {
  readonly document: ComposerDocument;
  readonly key: string;
  saving?: Promise<void>;
  error: Error | null;
  dirty: boolean;
  unsubscribe: () => void;
}

function errorOf(error: unknown): Error {
  if (error instanceof Error) return error;
  if (typeof error === "object" && error !== null && "message" in error) return new Error(String(error.message));
  return new Error(String(error));
}

function draftKey(target: ComposerTarget): string { return target.kind === "draft" ? "draft" : target.id; }

/** One conversation's shared input. No host state or turn events are used to release messages here. */
export class ComposerSession {
  readonly draft: ComposerDocument;
  private snapshot: ComposerSnapshot;
  private connected = false;
  private error: Error | null = null;
  private watching: { id: number; watch: string } | undefined;
  private generation = 0;
  private closed = false;
  private readonly documents = new Map<ComposerDocument, DocumentEntry>();
  private readonly editors = new Set<ComposerDocument>();
  private readonly operations = new Map<string, Operation>();
  private readonly listeners = new Set<(state: ComposerSessionState) => void>();
  private opening: Promise<void> | undefined;
  private closing: Promise<void> | undefined;
  /** What waits for the shared input to be back. */
  private readonly waiting: { resolve: () => void; reject: (error: Error) => void }[] = [];

  private constructor(
    private readonly transport: ComposerTransport,
    private readonly journal: ComposerJournalStore,
    readonly recoveryKey: string,
    private readonly releaseWriter: () => void,
    snapshot: ComposerSnapshot,
    saved: Awaited<ReturnType<ComposerJournalStore["read"]>>,
    /** The machine of the conversation, where what is asked of it goes. */
    private readonly machine: string | null,
  ) {
    this.snapshot = snapshot;
    const draft = [...saved].find(([key, value]) => key === "draft" && value.target.kind === "draft")
      ?? [...saved].find(([, value]) => value.target.kind === "draft");
    this.draft = this.createDocument({ kind: "draft" }, draft?.[1], draft?.[0]);
    for (const [key, record] of saved) {
      if (key === draft?.[0]) continue;
      this.editors.add(this.createDocument(record.target, record, key));
    }
  }

  static async open(who: Address, options: ComposerSessionOptions = {}): Promise<ComposerSession> {
    const transport = options.transport ?? composing(who);
    const writer = await composerWriter(who, options.recoveryKey);
    let session: ComposerSession | undefined;
    try {
      const journal = options.journal ?? await composerJournalStore(writer.key);
      const saved = await journal.read();
      const initial = await transport.read();
      session = new ComposerSession(transport, journal, writer.key, writer.release, initial.composer, saved, who.machine);
      await session.reconnect();
      return session;
    } catch (error) {
      if (session) await session.close();
      else writer.release();
      throw error;
    }
  }

  get state(): ComposerSessionState {
    const entries = [...this.documents.values()];
    return { composer: this.snapshot, connected: this.connected, error: this.error,
      operations: [...this.operations.values()].map(({ id, change, status, error, result, broken }) => ({ id, change, status, error, result, broken: !!broken })),
      editors: [...this.editors], recoveryKey: this.recoveryKey,
      saving: entries.some((entry) => entry.saving !== undefined), storageError: entries.find((entry) => entry.error)?.error ?? null,
      canSend: this.canSend(this.draft) };
  }

  /** Includes attachment destinations and uploads being prepared before the node exposes them. */
  canSend(document = this.draft): boolean {
    const entry = this.documents.get(document);
    if (!entry || entry.error || !this.connected || this.closed || this.closing || !document.view.state.canSend) return false;
    const { target, epoch } = document.view.state;
    if ([...this.documents.values()].some((other) => {
      const state = other.document.view.state;
      return state.target.kind === target.kind && (target.kind === "draft" || (state.target.kind === "queued" && state.target.id === target.id))
        && (state.pending > 0 || state.composing || state.error !== null || other.error !== null);
    })) return false;
    return !this.pendingMetadata(target, epoch).length && ![...this.operations.values()].some((operation) =>
      operation.status === "pending" && (target.kind === "draft"
        ? operation.change.do === "send" && operation.change.epoch === epoch
        : operation.change.do === "send_queued" && operation.change.id === target.id));
  }

  subscribe(listener: (state: ComposerSessionState) => void): () => void {
    this.listeners.add(listener);
    try { listener(this.state); } catch (error) { console.error("a composer session observer failed", error); }
    return () => { this.listeners.delete(listener); };
  }

  /** Reopens a terminated watch; on an interrupted watch, the shell may also reopen it.
   * New holds are acquired before pending edits are retried. No timers or retry budget. */
  reconnect(): Promise<void> {
    if (this.opening) return this.opening;
    if (this.closed || this.closing) return Promise.reject(new Error("the composer is closed"));
    this.opening = Promise.resolve().then(() => this.openWatch()).finally(() => { this.opening = undefined; });
    return this.opening;
  }

  private async openWatch(): Promise<void> {
    const generation = ++this.generation;
    this.disconnect(null);
    const old = this.watching;
    this.watching = undefined;
    if (old) await this.transport.unwatch(old.id);
    let ready = false;
    const buffered: ComposerPiece[] = [];
    try {
      const watching = await this.transport.watch((piece) => {
        if (generation !== this.generation || this.closed) return;
        if (!ready) buffered.push(piece);
        else this.piece(piece);
      });
      if (generation !== this.generation || this.closed) { await this.transport.unwatch(watching.id); return; }
      this.watching = watching;
      this.receive(watching.composer, true);
      await this.acquireEditors(watching.watch);
      this.connect();
      ready = true;
      for (const piece of buffered) this.piece(piece);
    } catch (error) { this.disconnect(errorOf(error)); throw error; }
  }

  private piece(piece: ComposerPiece): void {
    if (piece.kind !== "snapshot") { this.disconnect(piece.error === null ? null : errorOf(piece.error)); return; }
    if (this.watching?.watch === piece.watch) { this.receive(piece.composer); return; }
    if (!this.watching) return;
    this.disconnect(null);
    this.watching = { ...this.watching, watch: piece.watch };
    this.receive(piece.composer, true);
    const generation = this.generation;
    void this.acquireEditors(piece.watch).then(() => {
      if (generation === this.generation && this.watching?.watch === piece.watch && !this.closed) this.connect();
    }, (error: unknown) => { this.disconnect(errorOf(error)); });
  }

  private connect(): void {
    this.connected = true;
    this.error = null;
    for (const document of this.documents.keys()) document.setConnected(true);
    // What broke with the connection goes again under its own id, so the
    // node does it once (`world-network.md`, 断开不是结束); an upload goes
    // on from what its machine already holds.
    for (const operation of this.operations.values()) {
      if (operation.status === "failed" && operation.broken) void this.run(operation).catch(() => {});
    }
    for (const waiting of this.waiting.splice(0)) waiting.resolve();
    this.emit();
  }

  /** Resolves once the shared input is back; at once while it is. */
  private untilConnected(): Promise<void> {
    if (this.connected) return Promise.resolve();
    this.assertOpen();
    return new Promise((resolve, reject) => this.waiting.push({ resolve, reject }));
  }

  /** Runs an operation until the node answers it: one that broke goes
   *  again, as it was, once the connection is back, or after the core's
   *  wait when the connection never went (the way beyond it broke). */
  private async lasting(operation: Operation): Promise<ComposerUpdated | ComposerAttachmentAdded> {
    for (let tries = 1; ; tries += 1) {
      try {
        return await this.run(operation);
      } catch (error) {
        const forgotten = () => this.operations.get(operation.id) !== operation;
        if (!operation.broken || this.closed || this.closing || forgotten()) throw error;
        if (this.connected) await new Promise<void>((resolve) => after(tries + 1, resolve, this.machine));
        await this.untilConnected();
        if (forgotten()) throw error;
        if (operation.status === "completed") return operation.result!;
        if (operation.flight) {
          try {
            return await operation.flight;
          } catch {
            if (!operation.broken) throw operation.error;
          }
        }
      }
    }
  }

  private disconnect(error: Error | null): void {
    this.connected = false;
    this.error = error;
    for (const document of this.documents.keys()) document.setConnected(false);
    for (const operation of this.operations.values()) {
      if (!operation.flight) continue;
      operation.attempt += 1;
      operation.error = error ?? new Error("the composer connection broke before its operation was acknowledged");
      operation.status = "failed";
      operation.broken = true;
      operation.reject?.(operation.error);
      delete operation.reject;
      delete operation.flight;
    }
    this.emit();
  }

  private receive(snapshot: ComposerSnapshot, reset = false): void {
    if (!reset && snapshot.revision <= this.snapshot.revision) return;
    this.snapshot = snapshot;
    for (const document of this.documents.keys()) document.receive(snapshot, reset);
    this.emit();
  }

  private createDocument(target: ComposerTarget, saved?: ComposerJournal, key = draftKey(target)): ComposerDocument {
    let entry: DocumentEntry;
    const document = new ComposerDocument({ target: saved?.target ?? target, snapshot: saved?.snapshot ?? this.snapshot,
      ...(saved ? { journal: saved } : {}), send: async (mutation) => {
        await this.save(entry);
        try {
          const updated = await this.transport.update(mutation);
          this.receive(updated.composer);
          return updated;
        } catch (error) {
          // The document sends it again by itself (`ComposerDocument`).
          if (unanswered(error)) throw error;
          const failure = errorOf(error);
          if (["this draft was already sent; keep your local edit and apply it to the new epoch", "that queued message no longer exists", "that queued message is being delivered"].includes(failure.message)) {
            const updated = await this.transport.read();
            this.receive(updated.composer);
          }
          throw failure;
        }
      } });
    entry = { document, key, error: null, dirty: false, unsubscribe: () => {} };
    this.documents.set(document, entry);
    document.setConnected(false);
    document.receive(this.snapshot, true);
    entry.unsubscribe = document.subscribe(() => {
      void this.save(entry).catch(() => {});
      this.emit();
    });
    return document;
  }

  private save(entry: DocumentEntry): Promise<void> {
    entry.dirty = true;
    if (entry.saving) return entry.saving;
    entry.saving = Promise.resolve().then(async () => {
      while (entry.dirty) {
        entry.dirty = false;
        await this.journal.write(entry.key, entry.document.checkpoint());
      }
      entry.error = null;
    }).catch((error: unknown) => { entry.error = errorOf(error); throw entry.error; }).finally(() => { delete entry.saving; this.emit(); });
    return entry.saving;
  }

  /** Returns only after the node has held the item against automatic release.
   * The hold is shared, not exclusive; another device can edit or explicitly send it. */
  async editQueued(id: string): Promise<ComposerDocument> {
    this.assertConnected();
    const existing = [...this.editors].find((document) => document.view.state.target.kind === "queued" && document.view.state.target.id === id);
    if (existing) return existing;
    const watch = this.watching!.watch;
    await this.mutate({ do: "editing", id, watch, editing: true });
    if (this.watching?.watch !== watch || !this.connected) throw new Error("the composer watch changed while acquiring the edit hold");
    const document = this.createDocument({ kind: "queued", id });
    this.editors.add(document);
    document.setConnected(true);
    this.emit();
    return document;
  }

  private async acquireEditors(watch: string): Promise<void> {
    for (const document of this.editors) {
      const target = document.view.state.target;
      if (target.kind !== "queued") continue;
      const item = this.snapshot.queue.find((item) => item.id === target.id);
      if (!item || item.sending) continue;
      const updated = await this.transport.update({ operation: crypto.randomUUID(), do: "editing", id: target.id, watch, editing: true });
      this.receive(updated.composer);
    }
  }

  /** Changes are live, so finishing does not revert them. Freezes new input,
   * flushes, then removes this watch's hold. Failure restores editing. */
  async finishEditing(document: ComposerDocument): Promise<void> {
    if (!this.editors.has(document)) throw new Error("this document is not a queued editor of the composer");
    if (document.view.state.composing) throw new Error("finish the input composition before closing its editor");
    document.setClosing(true);
    try {
      await this.flushMetadata(document.view.state.target, document.view.state.epoch);
      await document.flush();
      await this.save(this.documents.get(document)!);
      const target = document.view.state.target;
      if (target.kind === "queued" && this.snapshot.queue.some((item) => item.id === target.id)) {
        this.assertConnected();
        await this.mutate({ do: "editing", id: target.id, watch: this.watching!.watch, editing: false });
      }
      this.editors.delete(document);
      this.documents.get(document)!.unsubscribe();
      document.dispose();
      this.documents.delete(document);
      this.emit();
    } catch (error) {
      document.setClosing(false);
      throw error;
    }
  }

  /** Captures this epoch before flushing. A concurrent send refuses this call;
   * it never silently sends the next draft. Textareas remain editable while the node works.
   * Sent while the connection is down, it waits for it and goes then; one that broke on the
   * way goes again under its own id, so the node sends it once. */
  async send(): Promise<ComposerUpdated> {
    this.assertOpen();
    const epoch = this.draft.view.state.epoch;
    if (this.draft.view.state.composing) throw new Error("finish the input composition before sending");
    for (;;) {
      await this.untilConnected();
      try {
        await this.flushMetadata({ kind: "draft" }, epoch);
        await this.flushDocuments({ kind: "draft" });
        break;
      } catch (error) {
        if (this.connected || this.closed || this.closing) throw error;
      }
    }
    const mutation: ComposerMutation = { do: "send", epoch, operation: crypto.randomUUID() };
    const operation: Operation = { id: mutation.operation, change: { do: "send", epoch }, status: "pending", error: null, result: null, attempt: 0,
      run: () => this.transport.update(mutation) };
    this.operations.set(operation.id, operation);
    return this.lasting(operation) as Promise<ComposerUpdated>;
  }

  async deleteQueued(id: string): Promise<ComposerUpdated> {
    await this.flushEditors(id);
    return this.mutate({ do: "delete", id });
  }
  async moveQueued(id: string, before: string | null): Promise<ComposerUpdated> { return this.mutate({ do: "move", id, before }); }

  async sendQueued(id: string): Promise<ComposerUpdated> {
    const item = this.snapshot.queue.find((item) => item.id === id);
    if (item) await this.flushMetadata({ kind: "queued", id }, item.draft.epoch);
    await this.flushEditors(id);
    return this.mutate({ do: "send_queued", id });
  }

  private async flushEditors(id: string): Promise<void> {
    await this.flushDocuments({ kind: "queued", id });
  }

  private async flushDocuments(target: ComposerTarget): Promise<void> {
    for (const entry of this.documents.values()) {
      const current = entry.document.view.state.target;
      if (current.kind === target.kind && (target.kind === "draft" || (current.kind === "queued" && current.id === target.id))) {
        await entry.document.flush();
        await this.save(entry);
      }
    }
  }

  async detach(attachment: string, document = this.draft): Promise<ComposerUpdated> {
    this.owned(document);
    const { target, epoch } = document.view.state;
    return this.mutate({ do: "detach", target, epoch, attachment });
  }

  /** Persists one destination for all non-images in this draft or queued editor.
   * Uses Talk's machine or machine.hub spelling; null restores its working-machine default.
   * Uploads still go to the resident's host. Placement happens when the message is delivered.
   * Competing choices follow node arrival order; an earlier epoch is refused.
   * A broken connection rejects the call; state.operations retains its ID for retryOperation. */
  async setFilesOn(machine: string | null, document = this.draft): Promise<ComposerUpdated> {
    this.owned(document);
    const { target, epoch } = document.view.state;
    return this.mutate({ do: "files_on", target, epoch, files_on: machine });
  }

  /** Attaches an already verified receipt, including an upload that finished after detachment. */
  async attach(attachment: Attachment, document = this.draft): Promise<ComposerUpdated> {
    this.owned(document);
    const { target, epoch } = document.view.state;
    return this.mutate({ do: "attach", target, epoch, attachment });
  }

  async attachFile(file: File, document = this.draft): Promise<ComposerAttachmentAdded> {
    return this.upload(document, (scope) => this.transport.attachFile(scope, file));
  }

  async attachPath(path: string, sent: (bytes: number) => void = () => {}, document = this.draft): Promise<ComposerAttachmentAdded> {
    return this.upload(document, (scope) => this.transport.attachPath(scope, path, sent));
  }

  private upload(document: ComposerDocument, run: (scope: { operation: string; target: ComposerTarget; epoch: number }) => Promise<ComposerAttachmentAdded>): Promise<ComposerAttachmentAdded> {
    this.assertConnected();
    this.owned(document);
    const { target, epoch } = document.view.state;
    const id = crypto.randomUUID();
    const operation: Operation = { id, change: { do: "upload", target, epoch }, status: "pending", error: null, result: null, attempt: 0,
      run: () => run({ operation: id, target, epoch }) };
    this.operations.set(id, operation);
    return this.lasting(operation) as Promise<ComposerAttachmentAdded>;
  }

  private pendingMetadata(target: ComposerTarget, epoch: number): Operation[] {
    return [...this.operations.values()].filter((operation) => {
      const change = operation.change;
      return operation.status === "pending" && (change.do === "upload" || change.do === "attach" || change.do === "detach" || change.do === "files_on") && change.epoch === epoch
        && change.target.kind === target.kind && (target.kind === "draft" || (change.target.kind === "queued" && change.target.id === target.id));
    });
  }

  private async flushMetadata(target: ComposerTarget, epoch: number): Promise<void> {
    let pending = this.pendingMetadata(target, epoch);
    while (pending.length) {
      await Promise.all(pending.map((operation) => operation.flight));
      pending = this.pendingMetadata(target, epoch);
    }
  }

  private mutate(change: ComposerChange): Promise<ComposerUpdated> {
    this.assertConnected();
    const mutation: ComposerMutation = { ...change, operation: crypto.randomUUID() };
    const operation: Operation = { id: mutation.operation, change, status: "pending", error: null, result: null, attempt: 0,
      run: () => this.transport.update(mutation) };
    this.operations.set(operation.id, operation);
    return this.run(operation) as Promise<ComposerUpdated>;
  }

  private run(operation: Operation): Promise<ComposerUpdated | ComposerAttachmentAdded> {
    if (operation.flight) return operation.flight;
    this.assertConnected();
    operation.status = "pending";
    operation.error = null;
    operation.broken = false;
    const attempt = ++operation.attempt;
    operation.flight = new Promise((resolve, reject) => {
      operation.reject = reject;
      void Promise.resolve().then(() => {
        if (operation.attempt !== attempt) throw new Error("the composer operation was disconnected before transmission");
        return operation.run();
      }).then((result) => {
        if (operation.attempt !== attempt) return;
        operation.result = result;
        operation.status = "completed";
        operation.run = () => Promise.resolve(result);
        if ("composer" in result) this.receive(result.composer);
        else if ("Ok" in result.updated) this.receive(result.updated.Ok.composer);
        resolve(result);
      }, (error: unknown) => {
        if (operation.attempt !== attempt) return;
        operation.error = errorOf(error);
        operation.status = "failed";
        operation.broken = unanswered(error) || !this.connected;
        reject(operation.error);
        void this.transport.read().then((updated) => { this.receive(updated.composer); },
          (error: unknown) => { this.error = errorOf(error); this.emit(); });
      }).finally(() => {
        if (operation.attempt !== attempt) return;
        delete operation.flight;
        delete operation.reject;
        this.emit();
      });
    });
    this.emit();
    return operation.flight;
  }

  /** Exact retry, including send's original epoch and upload's original scope.
   * A failed dispatch receipt remains failed; a deliberate resend is a new sendQueued call. */
  async retryOperation(id: string): Promise<ComposerUpdated | ComposerAttachmentAdded> {
    const operation = this.operations.get(id);
    if (!operation) throw new Error("the composer operation is not retained");
    if (operation.status === "completed") return Promise.resolve(operation.result!);
    return this.lasting(operation);
  }

  forgetOperation(id: string): void {
    const operation = this.operations.get(id);
    if (operation?.status === "pending") throw new Error("the composer operation is still in flight");
    this.operations.delete(id);
    this.emit();
  }

  /** Reads fresh authority before retrying pending text. Does not repeat queue commands or sends. */
  async retry(): Promise<void> {
    this.assertOpen();
    if (!this.connected) await this.reconnect();
    else {
      const updated = await this.transport.read();
      this.receive(updated.composer);
      for (const document of this.documents.keys()) document.retry();
    }
  }

  /** Unbind textareas first to commit composition. Pending edits remain in the
   * local outbox; closing the watch releases holds. Node drafts and queues stay intact. */
  close(): Promise<void> {
    if (this.closing) return this.closing;
    if (this.closed) return Promise.resolve();
    this.closing = Promise.resolve().then(() => this.end()).finally(() => { this.closing = undefined; });
    return this.closing;
  }

  private async end(): Promise<void> {
    this.generation += 1;
    this.disconnect(null);
    for (const waiting of this.waiting.splice(0)) waiting.reject(new Error("the composer is closed"));
    try {
      await Promise.all([...this.documents.values()].map((entry) => this.save(entry)));
      for (const entry of this.documents.values()) { entry.unsubscribe(); entry.document.dispose(); }
      this.closed = true;
    } finally {
      const watching = this.watching;
      this.watching = undefined;
      try { if (watching) await this.transport.unwatch(watching.id); }
      finally { if (this.closed) { this.releaseWriter(); this.listeners.clear(); } }
    }
  }

  private owned(document: ComposerDocument): void {
    if (!this.documents.has(document)) throw new Error("the document belongs to a different composer");
    if (document.view.state.closing) throw new Error("this queued editor is being finished");
  }
  private assertOpen(): void { if (this.closed || this.closing) throw new Error("the composer is closed"); }
  private assertConnected(): void { this.assertOpen(); if (!this.connected) throw new Error("the composer is disconnected"); }
  private emit(): void {
    if (!this.listeners.size) return;
    const state = this.state;
    for (const listener of this.listeners) {
      try { listener(state); } catch (error) { console.error("a composer session observer failed", error); }
    }
  }
}
