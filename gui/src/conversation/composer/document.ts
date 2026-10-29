import type { Character, CharacterId, ComposerDraft, ComposerEdit, ComposerSnapshot, ComposerTarget, ComposerUpdated } from "../../world/composer";
import { context, inserted, key, overlay, position, text, wellFormed } from "./characters";
import type { Context } from "./characters";
import { after, unanswered } from "../../world/again";

interface Pending {
  readonly mutation: ComposerEdit;
  readonly context: Context | null;
  attempted: boolean;
  observed: boolean;
}

/** Recovery data for unacknowledged edits, never an authority for the shared draft. */
export interface ComposerJournal {
  readonly snapshot: ComposerSnapshot;
  readonly target: ComposerTarget;
  readonly pending: readonly { mutation: ComposerEdit; context: number | null; attempted: boolean; observed: boolean }[];
  readonly contexts: readonly { id: CharacterId; previous: number | null }[];
  readonly history: readonly [number, readonly Character[]][];
  readonly retired: readonly string[];
}

export interface ComposerState {
  /** Changes to draft if a vanished queued item has unsaved local edits. */
  readonly target: ComposerTarget;
  readonly revision: number;
  readonly epoch: number;
  readonly pending: number;
  readonly connected: boolean;
  readonly composing: boolean;
  readonly closing: boolean;
  readonly targetAvailable: boolean;
  readonly sending: boolean;
  readonly needsSnapshot: boolean;
  readonly error: unknown;
  /** Send only while true, using this state's epoch. The authority still checks
   *  the epoch atomically against sends from other devices. */
  readonly canSend: boolean;
}

export interface ComposerDocumentOptions {
  readonly target: ComposerTarget;
  readonly snapshot: ComposerSnapshot;
  /** Resolve only after Updated; rejection retains the exact request for retry. */
  readonly send: (mutation: ComposerEdit) => Promise<ComposerUpdated>;
  readonly operationId?: () => string;
  readonly journal?: ComposerJournal;
}

export interface ComposerView {
  readonly characters: readonly Character[];
  readonly state: ComposerState;
  readonly renamed: ReadonlyMap<string, CharacterId>;
}

const staleEpoch = "this draft was already sent; keep your local edit and apply it to the new epoch";

/** Ordered, optimistic identity edits for one draft or queued document. Watch
 *  snapshots are views, never acknowledgements. At most one request is in flight. */
export class ComposerDocument {
  private readonly options: ComposerDocumentOptions;
  private snapshot: ComposerSnapshot;
  private target: ComposerTarget;
  private draft: ComposerDraft;
  private available = true;
  private sending = false;
  private pending: Pending[] = [];
  private readonly history = new Map<number, readonly Character[]>();
  private readonly retired = new Set<string>();
  private characters: readonly Character[] = [];
  private connected = true;
  private composing = false;
  private closing = false;
  private flight: Pending | undefined;
  private error: unknown = null;
  /** An edit no answer came to, or a busy machine did not start, goes again
   *  after this wait: how many times it went, and how to call the wait off. */
  private tries = 0;
  private waiting: (() => void) | undefined;
  private rejectedEpoch = false;
  private rejectedTarget = false;
  private disposed = false;
  private readonly listeners = new Set<(view: ComposerView) => void>();
  private generation = 0;

  constructor(options: ComposerDocumentOptions) {
    this.options = { ...options, target: Object.freeze({ ...options.target }) };
    this.target = this.options.target;
    this.snapshot = options.snapshot;
    const draft = this.getDraft(options.snapshot);
    if (!draft) throw new Error("the composer target is absent from the initial snapshot");
    this.draft = draft;
    this.characters = draft.characters;
    this.history.set(draft.epoch, draft.characters);
    if (options.journal) {
      const journal = options.journal;
      const contexts: Context[] = [];
      for (const node of journal.contexts) contexts.push({ id: node.id, previous: node.previous === null ? null : contexts[node.previous] });
      this.pending = journal.pending.map((entry) => ({ ...entry, context: entry.context === null ? null : contexts[entry.context] }));
      for (const [epoch, characters] of journal.history) this.history.set(epoch, characters);
      for (const id of journal.retired) this.retired.add(id);
      this.connected = false;
      this.project();
    }
  }

  get attachments(): ComposerDraft["attachments"] { return this.draft.attachments; }
  get uploads(): ComposerDraft["uploads"] { return this.draft.uploads; }
  /** The last node snapshot's destination, without an optimistic local selection. */
  get filesOn(): ComposerDraft["files_on"] { return this.draft.files_on; }

  /** A flat context table avoids recursive cloning of a long pasted prefix. */
  checkpoint(): ComposerJournal | null {
    if (!this.pending.length) return null;
    const indices = new Map<Context, number>();
    const contexts: { id: CharacterId; previous: number | null }[] = [];
    const register = (context: Context | null): number | null => {
      const path: Context[] = [];
      for (let node = context; node && !indices.has(node); node = node.previous) path.push(node);
      for (const node of path.reverse()) {
        indices.set(node, contexts.length);
        contexts.push({ id: node.id, previous: node.previous ? indices.get(node.previous)! : null });
      }
      return context ? indices.get(context)! : null;
    };
    return { snapshot: this.snapshot, target: this.target,
      pending: this.pending.map((entry) => ({ ...entry, context: register(entry.context) })), contexts,
      history: [...this.history], retired: [...this.retired] };
  }

  /** Waits for receipts for every local edit, including any active composition.
   * Rejects on disconnect or refusal; the journal remains available for exact retry. */
  flush(): Promise<void> {
    return new Promise((resolve, reject) => {
      let unsubscribe = () => {};
      const inspect = (view: ComposerView) => {
        const state = view.state;
        if (state.error !== null || !state.connected || this.disposed) {
          unsubscribe();
          reject(state.error ?? new Error(this.disposed ? "the composer is disposed" : "the composer is disconnected"));
        } else if (!state.pending && !state.composing) { unsubscribe(); resolve(); }
      };
      this.listeners.add(inspect);
      unsubscribe = () => { this.listeners.delete(inspect); };
      inspect(this.view);
    });
  }

  get view(): ComposerView {
    const state: ComposerState = {
      target: this.target,
      revision: this.snapshot.revision, epoch: this.draft.epoch, pending: this.pending.length,
      connected: this.connected, composing: this.composing, closing: this.closing, targetAvailable: this.available,
      sending: this.sending, needsSnapshot: this.rejectedEpoch || this.rejectedTarget, error: this.error,
      canSend: !this.disposed && !this.closing && this.connected && !this.composing && this.available && !this.sending
        && !this.pending.length && this.error === null && !this.draft.uploads.length
        && (/\P{White_Space}/u.test(text(this.characters)) || !!this.draft.attachments.length),
    };
    return { characters: this.characters, state, renamed: new Map() };
  }

  subscribe(listener: (view: ComposerView) => void): () => void {
    this.listeners.add(listener);
    try { listener(this.view); } catch (error) { console.error("a composer observer failed", error); }
    return () => { this.listeners.delete(listener); };
  }

  /** Only a new subscription may reset revision filtering. Its owner must stop
   *  feeding the superseded subscription before calling receive(..., true). */
  receive(snapshot: ComposerSnapshot, reset = false): void {
    if (!reset && snapshot.revision <= this.snapshot.revision) return;
    this.snapshot = snapshot;
    const draft = this.getDraft(snapshot);
    this.available = draft !== undefined;
    if (draft) {
      this.draft = draft;
      this.history.set(draft.epoch, draft.characters);
    }
    if (this.available && !this.sending && (this.error instanceof Error ? this.error.message : this.error) === "that queued message is being delivered") {
      this.error = null;
      this.rejectedTarget = false;
    }
    const visible = new Set([snapshot.draft, ...snapshot.queue.map((item) => item.draft), ...snapshot.unobserved.map((item) => item.draft)]
      .flatMap((document) => document.characters.map((character) => character.id.operation)));
    for (const entry of this.pending) {
      if (visible.has(entry.mutation.operation)) entry.observed = true;
    }
    this.refresh();
  }

  /** The supplied view is the one the browser actually edited, including a view
   *  frozen during composition. Start and end count scalars, not UTF-16 units. */
  edit(start: number, end: number, insert: string, characters = this.characters, epoch = this.draft.epoch): ComposerEdit | undefined {
    if (this.disposed) throw new Error("the composer is disposed");
    if (this.closing) throw new Error("this queued editor is being finished");
    if (start < 0 || end < start || end > characters.length) throw new Error("the edit is outside its document");
    insert = wellFormed(insert);
    if (start === end && !insert) return;
    const preceding = context(characters, start);
    const mutation = this.mutation(epoch, preceding?.id ?? null, characters.slice(start, end).map((character) => character.id), insert);
    this.pending.push({ mutation,
      context: preceding, attempted: false, observed: false });
    if (characters === this.characters && epoch === this.draft.epoch) {
      this.characters = overlay(characters, mutation, preceding);
      this.emit();
      this.pump();
    } else this.refresh();
    return mutation;
  }

  setConnected(connected: boolean): void {
    this.connected = connected;
    if (!connected) {
      this.generation += 1;
      this.flight = undefined;
    }
    if (connected) this.retry();
    else this.emit();
  }

  setComposing(composing: boolean): void {
    this.composing = composing;
    if (!composing && (this.retired.size || this.pending.some((entry) => entry.mutation.epoch !== this.draft.epoch))) this.refresh();
    else {
      this.emit();
      this.pump();
    }
  }

  /** Finish freezes new input while existing edits obtain their receipts. */
  setClosing(closing: boolean): void {
    this.closing = closing;
    this.emit();
  }

  /** No timer or guessed retry budget: call on reconnect or explicit retry. */
  retry(): void {
    if (!this.rejectedEpoch && !this.rejectedTarget) this.error = null;
    this.refresh();
  }

  /** Stops network work. Pending text stays readable; the owner must keep this
   *  document alive until saved if it wants to rebind a different textarea. */
  dispose(): void {
    this.disposed = true;
    this.generation += 1;
    this.flight = undefined;
    this.waiting?.();
    this.waiting = undefined;
    this.emit();
    this.listeners.clear();
  }

  private getDraft(snapshot: ComposerSnapshot): ComposerDraft | undefined {
    if (this.target.kind === "draft") {
      this.sending = false;
      return snapshot.draft;
    }
    const target = this.target;
    const item = snapshot.queue.find((item) => item.id === target.id);
    this.sending = item?.sending ?? false;
    return item?.draft;
  }

  private mutation(epoch: number, after: CharacterId | null, deleted: readonly CharacterId[], insert: string): ComposerEdit {
    const clone = (id: CharacterId): CharacterId => Object.freeze({ ...id });
    return Object.freeze({ operation: this.options.operationId?.() ?? crypto.randomUUID(), do: "edit", target: this.target,
      epoch, after: after && clone(after), delete: Object.freeze(deleted.map(clone)), insert });
  }

  private carry(): Character[] {
    const epochs = [...new Set(this.pending.map((entry) => entry.mutation.epoch))].filter((epoch) => epoch !== this.draft.epoch);
    let carry: Character[] = [];
    for (const epoch of epochs) {
      const entries = this.pending.filter((entry) => entry.mutation.epoch === epoch);
      let characters: readonly Character[] = [...(this.history.get(epoch) ?? []), ...carry];
      const local = new Set(carry.map((character) => key(character.id)));
      for (const entry of entries) {
        if (entry.observed) continue;
        characters = overlay(characters, entry.mutation, entry.context);
        for (const character of inserted(entry.mutation)) local.add(key(character.id));
      }
      carry = characters.filter((character) => local.has(key(character.id)));
    }
    const deleted = new Set(this.pending.flatMap((entry) => entry.mutation.delete.map(key)));
    return carry.filter((character) => !deleted.has(key(character.id)));
  }

  private project(): void {
    let characters: readonly Character[] = [...this.draft.characters, ...this.carry()];
    for (const entry of this.pending) {
      if (entry.mutation.epoch === this.draft.epoch && !entry.observed) characters = overlay(characters, entry.mutation, entry.context);
    }
    this.characters = characters;
  }

  private rebase(): ReadonlyMap<string, CharacterId> {
    const renamed = new Map<string, CharacterId>();
    if (this.composing || this.flight || !this.available || (!this.retired.size && !this.pending.some((entry) => entry.mutation.epoch !== this.draft.epoch))) return renamed;
    // A transmitted request must be resolved by its original receipt before a
    // new ID is allocated. Otherwise a lost response after Send duplicates text.
    if (this.pending[0]?.attempted && !this.rejectedEpoch) return renamed;
    const carry = this.carry();
    const retired = new Set([...this.retired, ...this.pending.filter((entry) => entry.mutation.epoch !== this.draft.epoch)
      .flatMap((entry) => inserted(entry.mutation).map((character) => key(character.id)))]);
    const current = this.pending.filter((entry) => entry.mutation.epoch === this.draft.epoch);
    this.pending = [];
    let characters = this.draft.characters;
    if (carry.length) {
      const preceding = context(characters);
      const mutation = this.mutation(this.draft.epoch, preceding?.id ?? null, [], text(carry));
      const replacements = inserted(mutation);
      carry.forEach((character, index) => renamed.set(key(character.id), replacements[index].id));
      this.pending.push({ mutation, context: preceding, attempted: false, observed: false });
      characters = [...characters, ...replacements];
    }
    for (const entry of current) {
      const remap = (id: CharacterId): CharacterId => renamed.get(key(id)) ?? id;
      const ids: CharacterId[] = [];
      for (let node = entry.context; node; node = node.previous) ids.push(remap(node.id));
      const preceding = context(ids.reverse().map((id) => ({ id, text: "" })));
      const at = position(characters, entry.mutation.after && remap(entry.mutation.after), preceding);
      const deleted = entry.mutation.delete.filter((id) => !retired.has(key(id)) || renamed.has(key(id))).map(remap);
      if (!deleted.length && !entry.mutation.insert) continue;
      const anchor = entry.mutation.after;
      const after = anchor && (!retired.has(key(anchor)) || renamed.has(key(anchor))) ? remap(anchor) : characters[at - 1]?.id ?? null;
      const mutation = this.mutation(this.draft.epoch, after, deleted, entry.mutation.insert);
      inserted(entry.mutation).forEach((character, index) => renamed.set(key(character.id), { operation: mutation.operation, index }));
      this.pending.push({ mutation, context: context(characters, at), attempted: false, observed: false });
      characters = overlay(characters, mutation, preceding);
    }
    this.rejectedEpoch = false;
    this.error = null;
    this.retired.clear();
    return renamed;
  }

  private refresh(): void {
    const first = this.pending[0];
    if (this.target.kind === "queued" && !this.available && !this.flight && first && (!first.attempted || this.rejectedTarget || this.rejectedEpoch)) {
      // A vanished target cannot save new edits. Resolve transmitted requests
      // first, then rescue only unaccepted insertion identities into the input.
      if (this.rejectedTarget || this.rejectedEpoch) first.attempted = false;
      this.target = Object.freeze({ kind: "draft" });
      this.draft = this.snapshot.draft;
      this.history.set(this.draft.epoch, this.draft.characters);
      this.available = true;
      this.sending = false;
      this.error = null;
      this.rejectedTarget = false;
    }
    const renamed = this.rebase();
    this.project();
    const epochs = new Set([this.draft.epoch, ...this.pending.map((entry) => entry.mutation.epoch)]);
    for (const epoch of this.history.keys()) if (!epochs.has(epoch)) this.history.delete(epoch);
    if (!this.pending.length && !this.composing) this.retired.clear();
    this.emit(renamed);
    this.pump();
  }

  private emit(renamed: ReadonlyMap<string, CharacterId> = new Map()): void {
    const view = { ...this.view, renamed };
    for (const listener of this.listeners) {
      try { listener(view); } catch (error) { console.error("a composer observer failed", error); }
    }
  }

  private pump(): void {
    const entry = this.pending[0];
    if (this.disposed || !this.connected || this.composing || this.flight || this.waiting || this.error !== null || !entry) return;
    // Receipts precede target validation on the authority. An exact retry can
    // resolve a lost reply even after its queued item disappeared or began sending.
    if ((!this.available || this.sending) && !entry.attempted) return;
    this.flight = entry;
    const generation = this.generation;
    // Scheduling in a microtask also converts a synchronous transport exception
    // into the same retained failure state as a rejected Promise.
    void Promise.resolve().then(() => {
      if (generation !== this.generation) return;
      if (this.disposed || !this.connected || this.composing || ((!this.available || this.sending) && !entry.attempted)) {
        this.flight = undefined;
        this.refresh();
        return;
      }
      entry.attempted = true;
      return this.options.send(entry.mutation);
    }).then((updated) => {
      if (!updated || generation !== this.generation) return;
      this.flight = undefined;
      const latest = updated.composer.revision > this.snapshot.revision ? updated.composer : this.snapshot;
      const target = this.target;
      const destination = target.kind === "draft" ? latest.draft : latest.queue.find((item) => item.id === target.id)?.draft;
      // Current-epoch edits can reference a still-unconfirmed character carried
      // from an earlier epoch. If its original request actually landed there,
      // those references must be removed even after that request leaves the log.
      if (!destination || destination.epoch !== entry.mutation.epoch) {
        for (const character of inserted(entry.mutation)) this.retired.add(key(character.id));
      }
      this.pending.shift();
      this.error = null;
      this.tries = 0;
      this.rejectedEpoch = false;
      this.rejectedTarget = false;
      this.receive(updated.composer);
      this.refresh();
    }, (error: unknown) => {
      if (generation !== this.generation) return;
      this.flight = undefined;
      // No answer came, or the machine had no place for it yet: the same
      // edit goes again a while later, and the node takes it once.
      if (unanswered(error)) {
        this.tries += 1;
        this.waiting = after(this.tries + 1, () => {
          this.waiting = undefined;
          this.pump();
        });
        return;
      }
      this.error = error;
      const message = error instanceof Error ? error.message : error;
      this.rejectedEpoch = message === staleEpoch;
      this.rejectedTarget = message === "that queued message no longer exists" || message === "that queued message is being delivered";
      this.refresh();
    });
  }
}
