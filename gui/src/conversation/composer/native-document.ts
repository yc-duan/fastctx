import type { Character, ComposerSnapshot, ComposerTarget } from "../../world/composer";
import type { ComposerDocumentOptions } from "./document";
import type { ComposerJournal, ComposerView, Seal } from "./document";
import type { DocumentSend } from "../../world/composer-sync";
import { ComposerDocument } from "./document";
import { NativeComposer, encode } from "./native";
import { key } from "./characters";
import { MAX_DOCUMENT_CHARACTERS } from "./limit";
const DOCUMENT_TOO_LONG = "the text is longer than this input holds";

/** The existing seal/IME/attachment projection is shared with old clients.
 * Native typing replaces only the legacy identity-edit outbox underneath it. */
export class NativeComposerDocument extends ComposerDocument {
  document: string;
  nativeActive = true;
  useLegacy(snapshot: ComposerSnapshot, send: ComposerDocumentOptions["send"]): void {
    this.nativeActive = false; this.setSender(send); super.receive(snapshot, true);
  }
  private readonly native: NativeComposer;
  private anchors: { seal: Seal; tail: Character | undefined; token: string }[] = [];
  private readonly captures = new WeakMap<Seal, Omit<DocumentSend, "operation" | "epoch">>();

  constructor(native: NativeComposer, target: ComposerTarget, saved?: ComposerJournal) {
    const authority = native.snapshot();
    const available = target.kind === "draft" || authority.queue.some((entry) => entry.id === target.id);
    const document = available ? native.document(target) : saved?.native?.document;
    if (!document) throw new Error("the retained composer document is absent");
    let snapshot = authority;
    if (!available && target.kind === "queued") {
      // A removed queue item must leave its unsent local words visible for the
      // existing preservation flow, rather than silently hiding its outbox.
      const previous = saved?.snapshot.queue.find((entry) => entry.id === target.id);
      const replica = native.replica(document);
      snapshot = { ...authority, queue: [...authority.queue, { id: target.id, from: previous?.from ?? "", editing: false, sending: false, error: previous?.error ?? null,
        draft: { epoch: previous?.draft.epoch ?? 0, attachments: previous?.draft.attachments ?? [], uploads: previous?.draft.uploads ?? [], files_on: previous?.draft.files_on ?? null,
          text: replica.content, characters: [...replica.characters] } }] };
    }
    super({ snapshot, target, send: async () => { throw new Error("native typing cannot send a legacy edit"); } });
    this.native = native; this.document = document;
  }
  override seal(): Seal {
    if (!this.nativeActive) return super.seal();
    this.pruneAnchors();
    const tail = this.view.characters.at(-1);
    const selected = this.native.replica(this.document).capture(this.view.characters);
    const capture = { do: "send_document" as const, document: this.document,
      snapshot: encode(selected.snapshot), take: encode(selected.take), attachments: this.attachments.map((one) => one.id) };
    const seal = super.seal();
    this.captures.set(seal, capture);
    const token = this.native.hold(this.document, this.view.state.epoch + this.anchors.length);
    this.anchors.push({ seal, tail, token });
    return seal;
  }
  private pruneAnchors(): void {
    if (!this.nativeActive) return;
    this.anchors = this.anchors.filter(({ seal, token }) => {
      if (this.sealed(seal)) return true;
      this.native.release(this.document, token);
      return false;
    });
  }
  override taken(seal: Seal): void { super.taken(seal); this.pruneAnchors(); }
  override unseal(seal: Seal): void { super.unseal(seal); this.pruneAnchors(); }
  flushCaptured(seal: Seal): Promise<void> {
    const anchor = this.anchors.find((anchor) => anchor.seal === seal);
    if (!anchor) return Promise.reject(new Error("the send no longer has its captured document"));
    return this.native.flushThrough(this.document, anchor.token);
  }
  captured(seal: Seal, epoch: number, operation: string): Omit<DocumentSend, "operation"> {
    const captured = this.captures.get(seal);
    if (!captured) throw new Error("the send no longer has its captured document");
    const anchor = this.anchors.find((anchor) => anchor.seal === seal);
    if (anchor) this.native.bind(this.document, anchor.token, operation);
    return { ...captured, epoch, attachments: this.selectedAttachments(seal) };
  }
  override get view(): ComposerView {
    const view = super.view;
    if (!this.native || !this.nativeActive) return view;
    const status = this.native.status(this.document);
    return { ...view, state: { ...view.state, pending: status.pending, error: view.state.error ?? status.error, canSend: view.state.canSend && !status.pending && !status.error } };
  }
  override retry(): void { if (this.nativeActive) this.native.retry(); super.retry(); }
  override beginRecovery(): string | null { return this.nativeActive ? super.beginRecovery(this.native.status(this.document).error) : super.beginRecovery(); }
  override discardPending(): void {
    if (!this.nativeActive) { super.discardPending(); return; }
    if (!this.view.state.recovering) throw new Error("the input text has not been preserved");
    this.native.discard(this.document);
    super.discardPending();
    this.document = this.native.document(this.view.state.target);
    super.receive(this.native.snapshot(), true);
  }
  override checkpoint(): ComposerJournal | null {
    if (!this.nativeActive) return super.checkpoint();
    return { snapshot: this.native.snapshot(false), target: this.view.state.target, pending: [], contexts: [], history: [], retired: [], native: this.native.journal(this.document) };
  }
  override receive(snapshot: ComposerSnapshot, reset = false): void { super.receive(snapshot, reset); this.pruneAnchors(); }
  override edit(start: number, end: number, insert: string, base = this.view.characters, _epoch = this.view.state.epoch): Character[] | undefined {
    if (!this.nativeActive) return super.edit(start, end, insert, base, _epoch);
    const state = this.view.state;
    if (!state.targetAvailable || state.closing || state.recovering || state.sending) throw new Error("the composer document is unavailable");
    const replica = this.native.replica(this.document);
    this.native.assertEditable(this.document);
    const removed = new Set(base.slice(start, end).map((one) => key(one.id)));
    const visible = this.view.characters;
    const deleted = !removed.size ? 0 : base === visible ? end - start : visible.filter((one) => removed.has(key(one.id))).length;
    const length = visible.length - deleted + [...insert].length;
    if (length > MAX_DOCUMENT_CHARACTERS && length > visible.length) throw new Error(DOCUMENT_TOO_LONG);
    this.pruneAnchors();
    const tail = !base.length && start === 0 ? [...this.anchors].reverse().find((anchor) => anchor.tail)?.tail : undefined;
    // An empty projection after Send is still after the sealed words. Anchoring
    // there keeps withdrawal and refusal ordered ahead of subsequent typing.
    const inserted = tail ? replica.edit(1, 1, insert, [tail]) : replica.edit(start, end, insert, base);
    // The replica observer already updated the session projection synchronously.
    return [...inserted];
  }
}
