import type { Address } from "../../world/wire";
import type { Attachment } from "../../world/attachments";
import type { ComposerAttachmentAdded, ComposerChange, ComposerMutation, ComposerPiece, ComposerSnapshot, ComposerTarget, ComposerUpdated } from "../../world/composer";
import { composing } from "../../world/composing";
import type { ComposerTransport } from "../../world/composing";
import { ComposerDocument } from "./document";
import { NativeComposer } from "./native";
import { NativeComposerDocument } from "./native-document";
import type { NativePiece } from "../../world/composer-sync";
import type { ComposerJournal, Seal } from "./document";
import { composerJournalStore } from "./journal";
import type { ComposerJournalStore } from "./journal";
import { composerWriter } from "./writer";
import { recoverable } from "./errors";
import { ANSWER_WITHIN, after, unanswered } from "../../world/again";

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
  /** Sends from this device whose words the node does not have yet. */
  readonly onTheirWay: number;
  /** The draft's attachments that went with them. */
  readonly attachmentsOnTheirWay: ReadonlySet<string>;
}

/** Words that did not go because words sent before them did not: they are
 *  back in the input with those, which say why. */
export class Withdrawn extends Error {
  constructor() {
    super("words sent before these did not go");
  }
}

/** The draft words were pressed in was sent from another device before
 *  they went: its next draft is not theirs to send. */
const SENT_ELSEWHERE = "the draft was sent from another device";

/** Words not handed to the node by the time an answer was due: they did
 *  not go. */
const NOT_HANDED = "the conversation's computer could not be reached in time";

/** What the node answers a taking back of words that went, or are going
 *  (`fastctx_types::composer::ALREADY_SENT`). */
const ALREADY_SENT = "these words were already sent";

/** Words sent from the input (`ComposerSession::send`): what the node made
 *  of them, and what can be done with them while that is not known
 *  (`specs/world-foundation.md`, 宪法第十一条). */
export interface Sending {
  sent: Promise<ComposerUpdated>;
  /** Sends them again at once, under their own id, while the line is up. */
  again(): void;
  /** Takes them back into the input: what the node makes of them is not
   *  waited for any more, and they are not sent again. */
  withdraw(): void;
}

/** What words on their way hear and can be told. */
interface Outcome {
  /** When they were sent. */
  readonly began: number;
  /** Told once when no answer came within `ANSWER_WITHIN`. */
  readonly late: (why: string) => void;
  told: boolean;
  /** Resolves once they are taken back. */
  readonly stop: Promise<void>;
  /** The operation that hands them to the node, once there is one. */
  operation?: Operation;
}

export interface ComposerSessionOptions {
  readonly transport?: ComposerTransport;
  /** Stable per writer, never shared by two live pages. The default survives a tab reload. */
  readonly recoveryKey?: string;
  readonly journal?: ComposerJournalStore;
}

interface Operation extends Omit<ComposerOperation, "broken" | "change"> {
  change: ComposerOperation["change"];
  /** It waits for words sent before it to reach the node, and does not
   *  belong to their draft. */
  waiting?: boolean;
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
  key: string;
  editing: boolean;
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
  private ownsJournal = false;
  /** Settles once the words sent last from here settled. */
  /** How many of the words sent from here the node has taken: what moves the
   *  draft's epoch on from this side. */
  private takenHere = 0;
  private sends: Promise<unknown> = Promise.resolve();
  /** Changes to the draft waiting for words sent before them to reach the
   *  node; each settles once its change is made, or under way. */
  private readonly preparing = new Set<Promise<unknown>>();
  private readonly documents = new Map<ComposerDocument, DocumentEntry>();
  private readonly editors = new Set<ComposerDocument>();
  private readonly operations = new Map<string, Operation>();
  private readonly files = new Map<string, File>();
  private readonly listeners = new Set<(state: ComposerSessionState) => void>();
  private opening: Promise<void> | undefined;
  private acquiring = false;
  private closing: Promise<void> | undefined;
  /** What waits for the shared input to be back. */
  private readonly waiting: { resolve: () => void; reject: (error: Error) => void }[] = [];

  private constructor(
    private readonly transport: ComposerTransport,
    private readonly journal: ComposerJournalStore,
    readonly recoveryKey: string,
    private readonly releaseWriter: () => Promise<void>,
    snapshot: ComposerSnapshot,
    saved: Awaited<ReturnType<ComposerJournalStore["read"]>>,
    /** The machine of the conversation, where what is asked of it goes. */
    private readonly machine: string | null,
    private native?: NativeComposer | undefined,
  ) {
    this.snapshot = snapshot;
    const draft = [...saved].find(([key, value]) => key === "draft" && value.target.kind === "draft")
      ?? [...saved].find(([, value]) => value.target.kind === "draft");
    this.draft = this.createDocument({ kind: "draft" }, draft?.[1], draft?.[0]);
    for (const [key, record] of saved) {
      if (key === draft?.[0]) continue;
      if (record.native && !(record.native.editing ?? !key.startsWith("native:replica:"))) continue;

      this.editors.add(this.createDocument(record.target, record, key));
    }
    if (native) {
      native.persist = async (id) => {
        if (this.closed) return;
        const entry = [...this.documents.values()].find((entry) => entry.document instanceof NativeComposerDocument && entry.document.document === id);
        if (entry) await this.save(entry);
        else {
          const snapshot = native.snapshot(false);
          const target: ComposerTarget = native.document({ kind: "draft" }) === id ? { kind: "draft" } : { kind: "queued", id: snapshot.queue.find((entry) => native.document({ kind: "queued", id: entry.id }) === id)?.id ?? id };
          await this.journal.write(`native:replica:${id}`, { snapshot, target, pending: [], contexts: [], history: [], retired: [], native: native.journal(id) });
        }
      };
      native.forget = (id) => this.closed ? Promise.resolve() : this.journal.write(`native:replica:${id}`, null);
      native.transmit = (input) => this.transport.documentInput!(this.watching!.id, input);
      native.subscribe(() => { this.receive(native.snapshot(), true, true); });
    }
  }

  static async open(who: Address, options: ComposerSessionOptions = {}): Promise<ComposerSession> {
    const transport = options.transport ?? composing(who);
    const writer = await composerWriter(who, options.recoveryKey);
    let session: ComposerSession | undefined;
    let journal: ComposerJournalStore | undefined;
    try {
      journal = options.journal ?? await composerJournalStore(writer.key);
      const saved = await journal.read();
      // An old identity outbox is drained through its original protocol before
      // a later open upgrades it. Native records use separate keys and survive
      // a temporary connection to an older node unchanged.
      if (transport.watchDocuments && ![...saved.values()].some((record) => !record.native && record.pending.length)) {
        const native = new NativeComposer([...saved.values()]);
        const buffered: NativePiece[] = [];
        const opened = await transport.watchDocuments(native.known(), (piece) => {
          if (session) { if (session.generation === 0 && !session.closed) session.nativePiece(piece); } else buffered.push(piece);
        });
        if (opened) {
          native.receive(opened.frame);
          const records = new Map([...saved].filter(([, record]) => !!record.native));
          session = new ComposerSession(transport, journal, writer.key, writer.release, native.snapshot(), records, who.machine, native);
          session.ownsJournal = !options.journal;
          session.watching = opened;
          session.acquiring = true;
          native.connect(false);
          await session.acquireEditors(opened.watch);
          session.acquiring = false;
          session.connect();
          for (const piece of buffered) session.nativePiece(piece);
          return session;
        }
        native.dispose();
      }
      const initial = await transport.read();
      const legacy = new Map([...saved].filter(([, record]) => !record.native));
      session = new ComposerSession(transport, journal, writer.key, writer.release, initial.composer, legacy, who.machine);
      session.ownsJournal = !options.journal;
      await session.reconnect();
      return session;
    } catch (error) {
      try { if (session) await session.close(); }
      catch (cleanup) { console.error("closing an unopened composer failed", cleanup); }
      finally {
        if (!options.journal) journal?.close?.();
        await writer.release();
      }
      throw error;
    }
  }

  get state(): ComposerSessionState {
    const entries = [...this.documents.values()];
    return { composer: this.snapshot, connected: this.connected, error: this.error,
      operations: [...this.operations.values()].map(({ id, change, status, error, result, broken }) => ({ id, change, status, error, result, broken: !!broken })),
      editors: [...this.editors], recoveryKey: this.recoveryKey,
      saving: entries.some((entry) => entry.saving !== undefined), storageError: entries.find((entry) => entry.error)?.error ?? null,
      canSend: this.canSend(this.draft), onTheirWay: this.draft.onTheirWay, attachmentsOnTheirWay: this.draft.attachmentsOnTheirWay };
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

  private nativePiece(piece: NativePiece): void {
    if (this.closed || !this.native) return;
    if (piece.kind === "interrupted") { this.disconnect(piece.error ? errorOf(piece.error) : null); return; }
    if (!this.watching) return;
    const changed = this.watching.watch !== piece.watch;
    if (changed) { this.disconnect(null); this.watching = { ...this.watching, watch: piece.watch }; }
    this.native.receive(piece.frame);
    if (changed) this.acquiring = true;
    this.native.connect(!this.acquiring);
    if (!changed && !this.connected && !this.acquiring) this.connect();
    if (changed) {
      const generation = this.generation;
      void this.acquireEditors(piece.watch).then(() => {
        if (generation !== this.generation || this.watching?.watch !== piece.watch || this.closed) return;
        this.acquiring = false; this.connect();
      }, (error: unknown) => {
        if (generation !== this.generation || this.watching?.watch !== piece.watch || this.closed) return;
        this.acquiring = false; this.disconnect(errorOf(error));
      });
    }
  }

  private async openWatch(): Promise<void> {
    if (this.native) {
      const generation = ++this.generation;
      this.disconnect(null);
      const old = this.watching;
      this.watching = undefined;
      if (old) await this.transport.unwatch(old.id);
      let ready = false;
      const buffered: NativePiece[] = [];
      const opened = await this.transport.watchDocuments!(this.native.known(), (piece) => {
        if (generation !== this.generation || this.closed) return;
        if (ready) this.nativePiece(piece); else buffered.push(piece);
      });
      if (!opened) {
        let updated: ComposerUpdated;
        try {
          for (;;) {
            if (this.native.pending) {
              if (!this.transport.syncText) throw new Error("the older node cannot synchronize this retained input yet; the local text is kept");
              await this.native.drainForLegacy((input) => this.transport.syncText!(input));
            }
            updated = await this.transport.read();
            if (!this.native.pending) break;
          }
        } catch (error) { this.draft.setStorageError(errorOf(error)); throw error; }
        const native = this.native;
        for (const [document, entry] of this.documents) if (document instanceof NativeComposerDocument) {
          document.useLegacy(updated.composer, async (mutation) => {
            await this.save(entry);
            const updated = await this.transport.update(mutation); this.receive(updated.composer); return updated;
          });
          entry.key = draftKey(document.view.state.target);
        }
        this.native = undefined; native.dispose(); this.receive(updated.composer, true);
        return this.openWatch();
      }
      if (generation !== this.generation || this.closed) { await this.transport.unwatch(opened.id); return; }
      this.watching = opened;
      this.acquiring = true;
      this.native.receive(opened.frame);
      ready = true;
      this.native.connect(false);
      for (const piece of buffered) this.nativePiece(piece);
      try {
        await this.acquireEditors(opened.watch);
        if (generation === this.generation && !this.closed) { this.acquiring = false; this.connect(); }
      } catch (error) { this.acquiring = false; this.disconnect(errorOf(error)); throw error; }
      return;
    }
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
    this.native?.connect();
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
  private async lasting(operation: Operation, outcome?: Outcome): Promise<ComposerUpdated | ComposerAttachmentAdded> {
    // Words on their way hear when they are late, and stop when taken back.
    const wait = <T>(waiting: Promise<T>): Promise<T> => (outcome ? this.meanwhile(waiting, outcome, true) : waiting);
    for (let tries = 1; ; tries += 1) {
      try {
        return await wait(this.run(operation));
      } catch (error) {
        if (error instanceof Withdrawn) throw error;
        const forgotten = () => this.operations.get(operation.id) !== operation;
        if (!operation.broken || this.closed || this.closing || forgotten()) throw error;
        if (this.connected) await wait(new Promise<void>((resolve) => after(tries + 1, resolve, this.machine)));
        await wait(this.untilConnected());
        if (forgotten()) throw error;
        if (operation.status === "completed") return operation.result!;
        if (operation.flight) {
          try {
            return await wait(operation.flight);
          } catch (error) {
            if (error instanceof Withdrawn) throw error;
            if (!operation.broken) throw operation.error;
          }
        }
      }
    }
  }

  private disconnect(error: Error | null): void {
    this.native?.disconnect();
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

  private receive(snapshot: ComposerSnapshot, reset = false, native = false): void {
    if (this.native && !native) return;
    if (!reset && snapshot.revision <= this.snapshot.revision) return;
    this.snapshot = snapshot;
    const held = new Set([snapshot.draft, ...snapshot.queue.map((item) => item.draft), ...snapshot.unobserved.map((item) => item.draft)]
      .flatMap((draft) => draft.attachments.map((file) => file.id)));
    for (const id of this.files.keys()) if (!held.has(id)) this.files.delete(id);
    for (const document of this.documents.keys()) document.receive(snapshot, reset);
    this.retainNative();
    this.emit();
  }

  private retainNative(): void {
    this.native?.retain([...this.documents.keys()].filter((document): document is NativeComposerDocument => document instanceof NativeComposerDocument).map((document) => document.document));
  }
  private createDocument(target: ComposerTarget, saved?: ComposerJournal, key = draftKey(target)): ComposerDocument {
    let entry: DocumentEntry;
    const document = this.native ? new NativeComposerDocument(this.native, target, saved) : new ComposerDocument({ target: saved?.target ?? target, snapshot: saved?.snapshot ?? this.snapshot,
      ...(saved ? { journal: saved } : {}), send: async (mutation) => {
        await this.save(entry);
        try {
          const updated = await this.update(mutation);
          this.receive(updated.composer);
          return updated;
        } catch (error) {
          // The document sends it again by itself (`ComposerDocument`).
          if (unanswered(error)) throw error;
          const failure = errorOf(error);
          if (recoverable(failure) || ["this draft was already sent; keep your local edit and apply it to the new epoch", "that queued message no longer exists", "that queued message is being delivered"].includes(failure.message)) {
            try {
              const updated = await this.transport.read();
              this.receive(updated.composer);
            } catch (error) {
              if (!recoverable(failure)) throw error;
            }
          }
          throw failure;
        }
      } });
    // One key per native document prevents a stale editor record from hiding a
    // newer retained replica and its exact repair outbox after reopening.
    const journalKey = document instanceof NativeComposerDocument ? `native:replica:${document.document}` : key;
    entry = { document, key: journalKey, editing: target.kind === "queued", error: null, dirty: false, unsubscribe: () => {} };
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
    if (this.closed) return Promise.resolve();
    entry.dirty = true;
    if (entry.saving) return entry.saving;
    entry.saving = Promise.resolve().then(async () => {
      while (entry.dirty) {
        entry.dirty = false;
        const checkpoint = entry.document.checkpoint();
        await this.journal.write(entry.key, checkpoint?.native ? { ...checkpoint, native: { ...checkpoint.native, editing: entry.editing } } : checkpoint);
      }
      entry.error = null; entry.document.setStorageError(null);
    }).catch((error: unknown) => {
      entry.error = errorOf(error);
      entry.document.setStorageError(entry.error);
      throw entry.error;
    }).finally(() => { delete entry.saving; this.emit(); });
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
      const updated = await this.update({ operation: crypto.randomUUID(), do: "editing", id: target.id, watch, editing: true });
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
      const entry = this.documents.get(document)!;
      entry.editing = false;
      await this.save(entry);
      const target = document.view.state.target;
      if (target.kind === "queued" && this.snapshot.queue.some((item) => item.id === target.id)) {
        this.assertConnected();
        await this.mutate({ do: "editing", id: target.id, watch: this.watching!.watch, editing: false });
      }
      this.editors.delete(document);
      this.documents.get(document)!.unsubscribe();
      document.dispose();
      this.documents.delete(document);
      this.retainNative(); this.emit();
    } catch (error) {
      const entry = this.documents.get(document)!;
      entry.editing = true;
      void this.save(entry).catch(() => {});
      document.setClosing(false);
      throw error;
    }
  }

  /** Sends what the input shows now. The words leave it at once, and what is
   * typed from here goes to the draft after them: each send goes once the
   * node has the words sent before it, under the epoch its draft is at then,
   * which is captured before flushing, so a concurrent send refuses it rather
   * than this one sending the next draft. Sent while the connection is down,
   * they wait for it and go then; one that broke on the way goes again under
   * its own id, so the node sends it once. Words that did not go are back in
   * the input, ahead of what was typed since, with any sent after them
   * (which fail as `Withdrawn`). */
  send(late: (why: string) => void = () => {}): Sending {
    const document = this.draft;
    // Changes asked of the draft before these words go before them.
    const prepared = [...this.preparing];
    // The draft they are pressed in, as the words sent from here before
    // them will have moved it on.
    const pressed = { epoch: document.view.state.epoch, taken: this.takenHere };
    let seal: Seal;
    try {
      this.assertOpen();
      if (document.view.state.composing) throw new Error("finish the input composition before sending");
      seal = document.seal();
    } catch (error) {
      return { sent: Promise.reject(error), again: () => {}, withdraw: () => {} };
    }
    const before = this.sends;
    this.sends = seal.settled;
    void seal.settled.then((taken) => {
      if (taken) this.takenHere += 1;
      this.emit();
    });
    this.emit();
    let withdraw = () => {};
    const stop = new Promise<void>((resolve) => {
      withdraw = resolve;
    });
    const outcome: Outcome = { began: Date.now(), late, told: false, stop };
    // Words waiting behind words sent before them are not handed to the node
    // yet: no answer to those by the time one is due, and they did not go.
    const behind = this.meanwhile(before, outcome, false).catch((error: unknown) => {
      document.unseal(seal);
      throw error;
    });
    const sent = behind.then(() => this.sendSealed(document, seal, prepared, pressed, outcome));
    return {
      sent,
      again: () => {
        const operation = outcome.operation;
        if (operation && this.connected && operation.status === "failed" && operation.broken) {
          void this.run(operation).catch(() => {});
        }
      },
      withdraw: () => withdraw(),
    };
  }

  /** Waits for `waiting` as words on their way do: taken back, they stop
   *  waiting. Past the time an answer is due, words not yet `handed` to the
   *  node did not go; words handed to it are told late once and go on
   *  waiting, to be settled when the line is back. */
  private meanwhile<T>(waiting: Promise<T>, outcome: Outcome, handed: boolean): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      let settled = false;
      const finish = (act: () => void) => {
        if (settled) return;
        settled = true;
        clearTimeout(due);
        act();
      };
      const due = window.setTimeout(() => {
        if (!handed) {
          finish(() => reject(new Error(this.error?.message || NOT_HANDED)));
        } else if (!outcome.told) {
          outcome.told = true;
          outcome.late(this.error?.message ?? "");
        }
      }, Math.max(0, outcome.began + ANSWER_WITHIN - Date.now()));
      waiting.then(
        (value) => finish(() => resolve(value)),
        (error: unknown) => finish(() => reject(error)),
      );
      void outcome.stop.then(() => finish(() => reject(new Withdrawn())));
    });
  }

  private async sendSealed(
    document: ComposerDocument,
    seal: Seal,
    prepared: readonly Promise<unknown>[],
    pressed: { epoch: number; taken: number },
    outcome: Outcome,
  ): Promise<ComposerUpdated> {
    if (!document.sealed(seal)) throw new Withdrawn();
    await Promise.allSettled(prepared);
    let epoch: number;
    try {
      for (;;) {
        await this.meanwhile(this.untilConnected(), outcome, false);
        epoch = document.view.state.epoch;
        try {
          // What was typed goes to the node before the words are sent; a node
          // that does not take it in time leaves them not sent.
          await this.meanwhile(this.flushMetadata({ kind: "draft" }, epoch), outcome, false);
          // Only pre-seal updates must be Saved. Later typing remains behind the
          // hold, so it cannot delay or enter the captured send.
          if (document instanceof NativeComposerDocument && document.nativeActive) await this.meanwhile(document.flushCaptured(seal), outcome, false);
          else await this.meanwhile(this.flushDocuments({ kind: "draft" }), outcome, false);
          break;
        } catch (error) {
          if (this.connected || this.closed || this.closing) throw error;
        }
      }
    } catch (error) {
      document.unseal(seal);
      throw error;
    }
    // Moved on by anything but the words sent from here, the draft went from
    // another device: what is in it now is not these words to send. Those of
    // them still in it are back in the input.
    if (epoch !== pressed.epoch + (this.takenHere - pressed.taken)) {
      document.unseal(seal);
      throw new Error(SENT_ELSEWHERE);
    }
    document.asking(seal, epoch);
    const operationId = crypto.randomUUID();
    const mutation: ComposerMutation = { ...(document instanceof NativeComposerDocument && document.nativeActive ? document.captured(seal, epoch, operationId) : { do: "send" as const, epoch }), operation: operationId };
    const operation: Operation = { id: mutation.operation, change: { do: "send", epoch }, status: "pending", error: null, result: null, attempt: 0,
      run: () => this.update(mutation) };
    this.operations.set(operation.id, operation);
    outcome.operation = operation;
    try {
      const updated = await this.lasting(operation, outcome) as ComposerUpdated;
      document.taken(seal);
      return updated;
    } catch (error) {
      if (!(error instanceof Withdrawn)) {
        document.unseal(seal);
        throw error;
      }
      let back: boolean;
      try {
        back = await this.takeBack(document, operation, outcome);
      } catch (failure) {
        document.unseal(seal);
        throw failure;
      }
      if (back) {
        document.unseal(seal);
        throw error;
      }
      // The node did not give them back: they went, or wait in its queue,
      // and what it made of them is still to hear.
      try {
        const updated = await this.lasting(operation) as ComposerUpdated;
        document.taken(seal);
        return updated;
      } catch (failure) {
        document.unseal(seal);
        throw failure;
      }
    }
  }

  /** Takes back the words `send` hands to the node; whether they are back
   *  in the input. Words that never left this device are taken back here,
   *  and not sent again when the line is back. Words handed to the node are
   *  its to give back: it is asked, under a name of the asking's own, and
   *  either has them back in the shared draft, which shows them here as it
   *  shows every change to it, or says they went. Until it answers, they
   *  stay as they are: taken back here alone, words the node did send would
   *  stand in the input a second time. */
  private async takeBack(document: ComposerDocument, send: Operation, outcome: Outcome): Promise<boolean> {
    const native = this.native;
    if (!native?.handed(send.id)) {
      this.operations.delete(send.id);
      try { await native?.withdraw(send.id); }
      catch (failure) {
        const entry = this.documents.get(document);
        if (entry) { entry.error = errorOf(failure); entry.document.setStorageError(entry.error); }
        this.emit();
      }
      return true;
    }
    const mutation: ComposerMutation = { operation: crypto.randomUUID(), do: "withdraw", send: send.id };
    const asking: Operation = { id: mutation.operation, change: { do: "withdraw", send: send.id }, status: "pending", error: null, result: null, attempt: 0,
      run: () => this.update(mutation) };
    this.operations.set(asking.id, asking);
    try {
      await this.lasting(asking);
    } catch (failure) {
      const why = errorOf(failure).message;
      // Words that went say so themselves, once the node says how they
      // landed; any other reason is said under them.
      if (why !== ALREADY_SENT) outcome.late(why);
      return false;
    } finally {
      this.operations.delete(asking.id);
    }
    this.operations.delete(send.id);
    await native.withdraw(send.id);
    return true;
  }

  /** What is done to the draft while words sent from it are on their way
   * waits until the node has them, and is done to the draft after them;
   * words sent meanwhile go after it. */
  private afterSent<T>(document: ComposerDocument, change: () => Promise<T>): Promise<T> {
    const before = document.view.state.target.kind === "draft" ? document.unsealed() : null;
    if (!before) return change();
    const done = before.then(change);
    this.preparing.add(done);
    const forget = () => { this.preparing.delete(done); };
    done.then(forget, forget);
    return done;
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
    return this.afterSent(document, () => {
      const { target, epoch } = document.view.state;
      return this.mutate({ do: "detach", target, epoch, attachment });
    });
  }

  /** Persists one destination for all non-images in this draft or queued editor.
   * Uses Talk's machine or machine.hub spelling; null restores its working-machine default.
   * Uploads still go to the resident's host. Placement happens when the message is delivered.
   * Competing choices follow node arrival order; an earlier epoch is refused.
   * A broken connection rejects the call; state.operations retains its ID for retryOperation. */
  async setFilesOn(machine: string | null, document = this.draft): Promise<ComposerUpdated> {
    this.owned(document);
    return this.afterSent(document, () => {
      const { target, epoch } = document.view.state;
      return this.mutate({ do: "files_on", target, epoch, files_on: machine });
    });
  }

  /** Attaches an already verified receipt, including an upload that finished after detachment. */
  async attach(attachment: Attachment, document = this.draft): Promise<ComposerUpdated> {
    this.owned(document);
    return this.afterSent(document, () => {
      const { target, epoch } = document.view.state;
      return this.mutate({ do: "attach", target, epoch, attachment });
    });
  }

  /** Discards refused edits only after the caller has saved the entire visible text.
   * The input stays frozen until the outbox deletion has committed. */
  async recoverText(document: ComposerDocument, preserve: (text: string) => Promise<void>): Promise<void> {
    this.assertOpen();
    const entry = this.documents.get(document);
    if (!entry) throw new Error("the document belongs to a different composer");
    const text = document.beginRecovery();
    if (text === null) return;
    try {
      await preserve(text);
      // Closing hands this outbox to a later writer. A late upload or copy
      // must never delete that writer's newer edits.
      if (this.closed || this.closing) return;
      const priorKey = entry.key;
      document.discardPending();
      if (document instanceof NativeComposerDocument) {
        entry.key = `native:replica:${document.document}`;
        entry.editing = document.view.state.target.kind === "queued";
      }
      await this.save(entry);
      if (priorKey !== entry.key) await this.journal.write(priorKey, null);
      document.setPreservationError(null);
      this.retainNative();
    } catch (error) {
      document.setPreservationError(errorOf(error));
      throw error;
    } finally { if (!this.closed && !this.closing) document.finishRecovery(); }
  }

  /** Bytes still held by this page, while their attachment belongs to a draft. */
  getFile(id: string): File | undefined { return this.files.get(id); }

  async attachFile(file: File, document = this.draft): Promise<ComposerAttachmentAdded> {
    const added = await this.upload(document, (scope) => this.transport.attachFile(scope, file));
    if (file.type.startsWith("text/plain")) { this.files.set(added.attachment.id, file); this.emit(); }
    return added;
  }

  async attachPath(path: string, sent: (bytes: number) => void = () => {}, document = this.draft): Promise<ComposerAttachmentAdded> {
    return this.upload(document, (scope) => this.transport.attachPath(scope, path, sent));
  }

  /** Listed at once, as this device's own; one added while words sent from
   * the draft are on their way starts once the node has them, in the draft
   * after them. Its scope, once it starts, is kept for every retry. */
  private upload(document: ComposerDocument, run: (scope: { operation: string; target: ComposerTarget; epoch: number }) => Promise<ComposerAttachmentAdded>): Promise<ComposerAttachmentAdded> {
    this.assertConnected();
    this.owned(document);
    const { target } = document.view.state;
    const before = target.kind === "draft" ? document.unsealed() : null;
    const id = crypto.randomUUID();
    let scope: { operation: string; target: ComposerTarget; epoch: number } | null = null;
    // Words sent after it is asked for go once it is under way, or over.
    let begun = () => {};
    if (before) {
      const started = new Promise<void>((resolve) => { begun = resolve; });
      this.preparing.add(started);
      void started.then(() => { this.preparing.delete(started); });
    }
    const operation: Operation = { id, change: { do: "upload", target, epoch: document.view.state.epoch }, status: "pending", error: null, result: null, attempt: 0,
      waiting: before !== null,
      run: async () => {
        if (!scope) {
          if (before) await before;
          scope ??= { operation: id, target, epoch: document.view.state.epoch };
          operation.change = { do: "upload", target, epoch: scope.epoch };
          operation.waiting = false;
          begun();
        }
        return run(scope);
      } };
    this.operations.set(id, operation);
    const done = this.lasting(operation) as Promise<ComposerAttachmentAdded>;
    done.then(begun, begun);
    return done;
  }

  private pendingMetadata(target: ComposerTarget, epoch: number): Operation[] {
    return [...this.operations.values()].filter((operation) => {
      const change = operation.change;
      return operation.status === "pending" && !operation.waiting && (change.do === "upload" || change.do === "attach" || change.do === "detach" || change.do === "files_on") && change.epoch === epoch
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

  private update(mutation: ComposerMutation): Promise<ComposerUpdated> {
    return this.native ? this.native.mutate(mutation) : this.transport.update(mutation);
  }

  private mutate(change: ComposerChange): Promise<ComposerUpdated> {
    this.assertConnected();
    const mutation: ComposerMutation = { ...change, operation: crypto.randomUUID() };
    const operation: Operation = { id: mutation.operation, change, status: "pending", error: null, result: null, attempt: 0,
      run: () => this.update(mutation) };
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
        if (!this.native) void this.transport.read().then((updated) => { this.receive(updated.composer); },
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
    else if (this.native) await this.reconnect();
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
      await this.native?.flushRetained();
      for (const entry of this.documents.values()) { entry.unsubscribe(); entry.document.dispose(); }
      this.native?.dispose();
      this.closed = true;
    } finally {
      const watching = this.watching;
      this.watching = undefined;
      try { if (watching) await this.transport.unwatch(watching.id); }
      finally { if (this.closed) { await this.releaseWriter(); if (this.ownsJournal) this.journal.close?.(); this.listeners.clear(); this.files.clear(); } }
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
