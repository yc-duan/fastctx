// Attachments belong to the node's draft. Local uploads keep one identity
// until its receipt arrives, including retries and removal during transfer.

import type { ComposerSession, ComposerSessionState, ComposerDocument } from "./composer";
import type { AttachWords, Attached } from "./attach";
import type { Attachment } from "../world/attachments";
import type { ComposerAttachmentAdded, ComposerUpdated } from "../world/composer";
import { viewAttached } from "./attached-pictures";
import { AttachmentRows, type AttachmentRow } from "./attachment-rows";

/** A failure in the World's words, with interface wording where available. */
export type Failed = (why: string, said?: string) => void;
const reason = (error: unknown): string => String(error instanceof Error ? error.message : error);
const IMAGE = /\.(png|jpe?g|gif|webp|bmp|avif|svg)$/i;

interface Upload {
  key: string;
  name: string;
  size: number | null;
  image: boolean;
  preview: string | null;
  operation: string | null;
  attachment: string | null;
  settled: boolean;
  error: string | null;
  removed: boolean;
  detaching: boolean;
  begin: (() => Promise<ComposerAttachmentAdded>) | null;
  first?: (() => Promise<unknown>) | undefined;
  flight?: Promise<void>;
}

/** Where a file other than an image goes for an assisting agent. */
export interface AssistantFiles {
  assisting(): boolean;
  place(): string | null;
  nowhere: string;
}

export class NodeTray {
  readonly element: HTMLUListElement;
  private readonly rows: AttachmentRows;
  private readonly uploads = new Map<string, Upload>();
  private readonly removing = new Set<string>();
  private readonly removals = new Set<Promise<void>>();
  private state: ComposerSessionState | null = null;
  private closed = false;

  constructor(
    private readonly words: AttachWords,
    private readonly session: ComposerSession,
    private readonly document: ComposerDocument,
    private readonly failed: Failed,
    private readonly assistantFiles: AssistantFiles | null = null,
    private readonly changed: () => void = () => {},
  ) {
    this.element = window.document.createElement("ul");
    this.element.className = "attach-tray";
    this.element.hidden = true;
    this.rows = new AttachmentRows(this.element, words);
    this.element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-remove], [data-retry]");
      if (!button) {
        viewAttached(this.element, event.target as Element);
        return;
      }
      const upload = this.uploads.get(button.dataset.retry ?? button.dataset.remove ?? "");
      if (button.dataset.retry) {
        if (upload) void this.send(upload);
      } else if (upload) {
        upload.removed = true;
        this.draw();
      } else if (button.dataset.remove) {
        void this.detach(button.dataset.remove);
      }
    });
  }

  private draft(state = this.state) {
    const target = this.document.view.state.target;
    if (!state) return null;
    if (target.kind !== "draft") return state.composer.queue.find((item) => item.id === target.id)?.draft ?? null;
    const draft = state.composer.draft;
    const going = state.attachmentsOnTheirWay;
    return going.size ? { ...draft, attachments: draft.attachments.filter((one) => !going.has(one.id)) } : draft;
  }

  get count(): number { return this.items().length; }
  get uploading(): boolean { return this.items().some((item) => item.state !== "ready"); }
  get files(): number { return this.items().filter((item) => !item.image).length; }
  get placing(): boolean { return !!this.assistantFiles?.assisting() && this.files > 0; }
  get chosen(): string | null { return this.placing ? this.draft()?.files_on ?? this.assistantFiles!.place() : null; }

  settle(): void {
    const chosen = this.chosen;
    if (chosen === null || chosen === (this.draft()?.files_on ?? null)) return;
    this.session.setFilesOn(chosen, this.document).catch((error: unknown) => this.failed(reason(error), this.words.failed(reason(error))));
  }

  addFiles(files: Iterable<File>): void {
    for (const file of files) {
      const image = file.type.startsWith("image/") || IMAGE.test(file.name);
      const placed = this.placed(image);
      if (placed === null) continue;
      this.start(file.name || this.words.attach, file.size, image, image ? URL.createObjectURL(file) : null,
        () => this.session.attachFile(file, this.document), placed);
    }
  }

  addPaths(paths: string[]): void {
    for (const path of paths) {
      const name = path.split(/[\\/]/).pop() ?? path;
      const image = IMAGE.test(name);
      const placed = this.placed(image);
      if (placed === null) continue;
      this.start(name, null, image, null, () => this.session.attachPath(path, () => {}, this.document), placed);
    }
  }

  /** Takes files held locally while this shared input was opening. */
  adopt(held: readonly Attached[]): void {
    for (const item of held) {
      const placed = this.placed(item.image);
      if (placed === null) {
        if (item.preview) URL.revokeObjectURL(item.preview);
        continue;
      }
      const source = item.source;
      const receipt = source ? null : item.ref as Attachment;
      const begin = source?.kind === "bytes" ? () => this.session.attachFile(source.file, this.document)
        : source?.kind === "path" ? () => this.session.attachPath(source.path, () => {}, this.document)
        : async () => ({ attachment: receipt!, updated: { Ok: await this.session.attach(receipt!, this.document) } });
      this.start(item.name, item.size, item.image, item.preview, begin, placed, receipt?.id);
    }
  }

  private placed(image: boolean): (() => Promise<unknown>) | null | undefined {
    if (image || !this.assistantFiles?.assisting()) return undefined;
    const kept = this.draft()?.files_on ?? null;
    const place = kept ?? this.assistantFiles.place();
    if (place === null) {
      this.failed("", this.words.imagesOnly);
      return null;
    }
    return place === kept ? undefined : () => this.session.setFilesOn(place, this.document);
  }

  private start(name: string, size: number | null, image: boolean, preview: string | null,
    begin: () => Promise<ComposerAttachmentAdded>, first?: () => Promise<unknown>, attachment?: string): void {
    const upload: Upload = { key: crypto.randomUUID(), name, size, image, preview, operation: null,
      attachment: attachment ?? null, settled: false, error: null, removed: false, detaching: false, begin, first };
    this.uploads.set(upload.key, upload);
    this.draw();
    void this.send(upload);
  }

  private send(upload: Upload): Promise<void> {
    if (upload.flight) return upload.flight;
    const flight = this.perform(upload);
    upload.flight = flight;
    const finished = () => { delete upload.flight; };
    void flight.then(finished, finished);
    return flight;
  }

  private async perform(upload: Upload): Promise<void> {
    upload.error = null;
    try {
      if (upload.first) {
        await upload.first();
        upload.first = undefined;
      }
      if (upload.removed && !upload.operation) { this.forget(upload); this.draw(); return; }
      let done: Promise<ComposerAttachmentAdded | ComposerUpdated>;
      if (upload.operation) done = this.session.retryOperation(upload.operation);
      else {
        const before = new Set(this.session.state.operations.map((one) => one.id));
        done = upload.begin!();
        upload.operation = this.session.state.operations.find((one) => !before.has(one.id))?.id ?? null;
      }
      this.draw(this.session.state);
      const added = await done;
      // Attaching a receipt is an ordinary mutation, while uploading returns
      // both the receipt and the result of attaching it to the draft.
      if ("attachment" in added) {
        upload.attachment = added.attachment.id;
        if ("Err" in added.updated && !upload.removed) this.failed(added.updated.Err, this.words.failed(added.updated.Err));
      }
      upload.settled = true;
      upload.begin = null;
      if (upload.removed && upload.attachment && !upload.detaching) await this.detach(upload.attachment, upload);
    } catch (error) {
      upload.error = reason(error);
      if (!upload.removed && !this.closed) this.failed(upload.error, this.words.failed(upload.error));
    }
    this.draw(this.session.state);
  }

  private forget(upload: Upload): void {
    if (upload.preview) URL.revokeObjectURL(upload.preview);
    this.uploads.delete(upload.key);
  }

  private detach(id: string, upload?: Upload): Promise<void> {
    if (this.removing.has(id)) return Promise.resolve();
    this.removing.add(id);
    if (upload) upload.detaching = true;
    const done = this.session.detach(id, this.document).then(() => {
      if (upload) this.forget(upload);
    }, (error: unknown) => {
      if (upload) { upload.removed = false; upload.detaching = false; }
      this.failed(reason(error), this.words.failed(reason(error)));
    }).finally(() => {
      this.removing.delete(id);
      this.draw(this.session.state);
    });
    this.removals.add(done);
    const finished = () => { this.removals.delete(done); };
    void done.then(finished, finished);
    return done;
  }

  private reconcile(state: ComposerSessionState): void {
    const drafts = [state.composer.draft, ...state.composer.queue.map((one) => one.draft), ...state.composer.unobserved.map((one) => one.draft)];
    const held = new Set(drafts.flatMap((draft) => [...draft.attachments.map((one) => one.id), ...draft.uploads.map((one) => one.id)]));
    const pending = new Map(drafts.flatMap((draft) => draft.uploads.flatMap((one) => one.operation ? [[one.operation, one.id] as const] : [])));
    const operations = new Map(state.operations.map((one) => [one.id, one]));
    for (const upload of this.uploads.values()) {
      const operation = upload.operation ? operations.get(upload.operation) : undefined;
      upload.attachment = (upload.operation ? pending.get(upload.operation) : undefined) ?? upload.attachment;
      if (operation?.result && "attachment" in operation.result) upload.attachment = operation.result.attachment.id;
      if (operation?.status === "completed") {
        upload.settled = true;
        upload.begin = null;
      }
      if (upload.removed) {
        if (upload.attachment && !upload.detaching) void this.detach(upload.attachment, upload);
        else if (!upload.attachment && (upload.settled || operation?.status === "failed" || (!upload.operation && upload.error !== null))) {
          this.forget(upload);
          if (upload.operation) this.session.forgetOperation(upload.operation);
        }
      } else if (upload.settled && upload.attachment && !held.has(upload.attachment)) this.forget(upload);
    }
  }

  private items(): AttachmentRow[] {
    const draft = this.draft();
    const operations = new Map((this.state?.operations ?? []).map((one) => [one.id, one]));
    const local = new Map([...this.uploads.values()].flatMap((one) => one.attachment ? [[one.attachment, one] as const] : []));
    const visible = new Set([...(draft?.attachments ?? []).map((one) => one.id), ...(draft?.uploads ?? []).map((one) => one.id)]);
    const uploading = (upload: Upload | undefined, error: string | null) => {
      const operation = upload?.operation ? operations.get(upload.operation) : undefined;
      if (upload?.removed) return { state: "sending" as const, fact: this.words.removing, retry: null, why: null };
      if (operation?.status === "failed" && operation.broken) return { state: "sending" as const, fact: this.words.resuming, retry: null, why: operation.error?.message ?? null };
      const why = operation?.status === "failed" ? operation.error?.message : upload?.error ?? error;
      return why != null ? { state: "failed" as const, fact: this.words.failed(why), retry: upload?.key ?? null, why }
        : { state: "sending" as const, fact: this.words.sending, retry: null, why: null };
    };
    return [
      ...(draft?.attachments ?? []).map((one): AttachmentRow => {
        const own = local.get(one.id);
        const removing = own?.removed || this.removing.has(one.id);
        return { key: own?.key ?? one.id, name: one.name, image: one.media_type.startsWith("image/") || IMAGE.test(one.name),
          preview: own?.preview ?? null, state: removing ? "sending" : "ready", fact: removing ? this.words.removing : this.words.size(one.size), remove: own?.key ?? one.id };
      }),
      ...(draft?.uploads ?? []).map((one): AttachmentRow => {
        const own = local.get(one.id);
        return { key: own?.key ?? one.id, name: one.file.name, image: one.file.media_type.startsWith("image/") || IMAGE.test(one.file.name),
          preview: own?.preview ?? null, ...uploading(own, one.error), remove: own?.key ?? one.id };
      }),
      ...[...this.uploads.values()].filter((one) => !one.settled && (!one.attachment || !visible.has(one.attachment))).map((one): AttachmentRow => ({
        key: one.key, name: one.name, image: one.image, preview: one.preview, ...uploading(one, null), remove: one.key,
      })),
    ];
  }

  /** Removal requested here finishes before its editor or session closes. */
  async settledRemovals(): Promise<void> {
    const uploads = [...this.uploads.values()].filter((one) => one.removed).flatMap((one) => one.flight ? [one.flight] : []);
    await Promise.allSettled([...this.removals, ...uploads]);
  }

  close(): Promise<void> {
    const settled = this.settledRemovals();
    this.closed = true;
    for (const upload of this.uploads.values()) {
      if (upload.preview) URL.revokeObjectURL(upload.preview);
      upload.preview = null;
    }
    this.uploads.clear();
    return settled;
  }

  draw(state: ComposerSessionState | null = this.state): void {
    if (this.closed) return;
    this.state = state;
    if (state) this.reconcile(state);
    this.rows.draw(this.items());
    this.changed();
  }
}
