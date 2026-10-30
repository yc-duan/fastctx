// What is attached to the shared words, as the node of the agent's machine
// keeps them (`world-gui.md`, 对话: the input is one for every device). Every
// device shows the same files; a file goes to that machine as soon as it is
// attached, and any device can take one out. What this device is still
// sending, before the node lists it, is shown too, with its picture where
// the page holds the bytes; once listed, it is one row, as this device's
// own upload has it. One whose connection broke waits to go on from where
// it stopped (`world-network.md`, 断开不是结束).
//
// For an agent assisting the user, files other than images go to one of
// the user's machines (`world-run.md`, 协助的智能体没有「这里」), without
// asking.
// The node keeps that machine with the words, the one the first device to
// send them gave, so they go to one machine whichever device sends.

import type { ComposerSession, ComposerSessionState } from "./composer";
import type { ComposerDocument } from "./composer";
import type { AttachWords } from "./attach";
import { viewAttached } from "./attached-pictures";
import { glyph } from "./glyphs";
import { escape } from "./prose";

/** Something could not be done, said by the composer: the World's own
 *  words, kept for the pointer's rest, and the interface's words for it
 *  where the one that failed has them. */
export type Failed = (why: string, said?: string) => void;

const reason = (error: unknown): string => String(error instanceof Error ? error.message : error);

const IMAGE = /\.(png|jpe?g|gif|webp|bmp|avif|svg)$/i;

interface Sending {
  name: string;
  size: number | null;
  image: boolean;
  preview: string | null;
}

/** For an agent assisting the user, which has no machine it works on:
 *  where its files other than images go. */
export interface AssistantFiles {
  /** Whether the agent assists the user. */
  assisting(): boolean;
  /** The user's machine they go to, as written in the World, or null when
   *  none can take them. */
  place(): string | null;
  /** Said when none can. */
  nowhere: string;
}

export class NodeTray {
  readonly element: HTMLUListElement;
  /** What this device started sending, by its upload's operation. */
  private readonly sending = new Map<string, Sending>();
  /** Pictures of the files this device attached, by name and size. */
  private readonly previews = new Map<string, string>();
  private state: ComposerSessionState | null = null;

  constructor(
    private readonly words: AttachWords,
    private readonly session: ComposerSession,
    private readonly document: ComposerDocument,
    private readonly failed: Failed,
    private readonly assistantFiles: AssistantFiles | null = null,
  ) {
    this.element = window.document.createElement("ul");
    this.element.className = "attach-tray";
    this.element.hidden = true;
    this.element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-remove], [data-retry]");
      if (!button) {
        viewAttached(this.element, event.target as Element);
        return;
      }
      if (button.dataset.retry) {
        this.session.retryOperation(button.dataset.retry).catch((error: unknown) => this.failed(reason(error), this.words.failed(reason(error))));
        return;
      }
      const id = button.dataset.remove!;
      if (this.sending.has(id)) {
        this.forget(id);
        this.draw();
        return;
      }
      // This device's own upload goes no further once taken out.
      const operation = this.draft()?.uploads.find((one) => one.id === id)?.operation;
      if (operation && this.sending.has(operation)) this.forget(operation);
      this.session.detach(id, this.document).catch((error: unknown) => this.failed(reason(error)));
    });
  }

  private forget(operation: string): void {
    this.sending.delete(operation);
    try {
      this.session.forgetOperation(operation);
    } catch {
      // Still on its way: it is listed again once the node has it.
    }
  }

  /** The words this tray holds the files of: the input's, without those
   *  that went with words still on their way, or a queued one's being
   *  changed. */
  private draft(state: ComposerSessionState | null = this.state) {
    const target = this.document.view.state.target;
    if (!state) return null;
    if (target.kind !== "draft") return state.composer.queue.find((item) => item.id === target.id)?.draft ?? null;
    const draft = state.composer.draft;
    const going = state.attachmentsOnTheirWay;
    return going.size ? { ...draft, attachments: draft.attachments.filter((one) => !going.has(one.id)) } : draft;
  }

  /** Whether files are attached or on their way. */
  get count(): number {
    const draft = this.draft();
    return (draft?.attachments.length ?? 0) + (draft?.uploads.length ?? 0) + this.sending.size;
  }

  /** Files other than images among those attached. */
  get files(): number {
    const draft = this.draft();
    return (draft?.attachments ?? []).filter((one) => !one.media_type.startsWith("image/") && !IMAGE.test(one.name)).length;
  }

  /** Whether files other than images go with the words to an assisting
   *  agent, and so need a machine of the user's. */
  get placing(): boolean {
    return !!this.assistantFiles?.assisting() && this.files > 0;
  }

  /** Where the files other than images go: the machine kept with the
   *  words, else the one they would go to now. Null when they need none,
   *  or none can take them. */
  get chosen(): string | null {
    if (!this.placing) return null;
    return this.draft()?.files_on ?? this.assistantFiles!.place();
  }

  /** Gives the node the machine, when it does not have it yet: just before
   *  the words are sent, so every device sends them to that one. */
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
      const preview = image ? URL.createObjectURL(file) : null;
      if (preview) this.previews.set(`${file.name}:${file.size}`, preview);
      this.start({ name: file.name || this.words.attach, size: file.size, image, preview }, () => this.session.attachFile(file, this.document), placed);
    }
  }

  addPaths(paths: string[]): void {
    for (const path of paths) {
      const name = path.split(/[\\/]/).pop() ?? path;
      const image = IMAGE.test(name);
      const placed = this.placed(image);
      if (placed === null) continue;
      this.start({ name, size: null, image, preview: null }, () => this.session.attachPath(path, () => {}, this.document), placed);
    }
  }

  /** What has to come before a file goes to an agent assisting the user: a
   *  file other than an image goes straight to a machine of the user's, so
   *  the node is told which first. Null where none can take it, which is
   *  then not attached and said so; nothing to do for any other file. */
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

  private start(file: Sending, upload: () => ReturnType<ComposerSession["attachFile"]>, first?: () => Promise<unknown>): void {
    if (first) {
      first().then(
        () => this.start(file, upload),
        (error: unknown) => this.failed(reason(error), this.words.failed(reason(error))),
      );
      return;
    }
    const before = new Set(this.session.state.operations.map((operation) => operation.id));
    const done = upload();
    const operation = this.session.state.operations.find((one) => !before.has(one.id));
    if (operation) this.sending.set(operation.id, file);
    this.draw();
    // One the user took out says nothing more.
    const kept = () => !operation || this.sending.has(operation.id);
    done.then(
      (added) => {
        if (kept() && "Err" in added.updated) this.failed(added.updated.Err, this.words.failed(added.updated.Err));
        if (operation) this.sending.delete(operation.id);
        this.draw();
      },
      (error: unknown) => {
        if (kept()) this.failed(reason(error), this.words.failed(reason(error)));
        this.draw();
      },
    );
  }

  draw(state: ComposerSessionState | null = this.state): void {
    this.state = state;
    const { words } = this;
    const draft = this.draft(state);
    const operations = new Map((state?.operations ?? []).map((operation) => [operation.id, operation]));
    const item = (key: string, name: string, image: boolean, preview: string | null, fact: string, failed: string | null, retry: string | null, why: string | null = null) => {
      const picture = preview ? `<img class="attach-thumb" src="${escape(preview)}" alt="${escape(name)}">` : `<span class="attach-thumb attach-glyph">${glyph(image ? "image" : "file", 18)}</span>`;
      const said = failed ? `<span class="attach-fact wrong" title="${escape(failed)}">${escape(words.failed(failed))}</span>` : fact ? `<span class="attach-fact"${why ? ` title="${escape(why)}"` : ""}>${escape(fact)}</span>` : "";
      const again = retry ? `<button class="attach-act" type="button" data-retry="${escape(retry)}">${escape(words.retry)}</button>` : "";
      return `<li class="attach-item ${failed ? "failed" : fact === words.sending || fact === words.resuming ? "sending" : "ready"}">${picture}<span class="attach-text"><span class="attach-name" title="${escape(name)}">${escape(name)}</span>${said}</span>${again}<button class="attach-remove" type="button" data-remove="${escape(key)}" aria-label="${escape(words.remove)}" title="${escape(words.remove)}">${glyph("close", 14)}</button></li>`;
    };
    // How an upload stands: as this device's own operation has it while it
    // has one, else as the node says.
    const standing = (operation: string | null, error: string | null) => {
      const own = operation === null ? undefined : operations.get(operation);
      if (own?.status === "failed" && own.broken) return { fact: words.resuming, failed: null, retry: null, why: own.error?.message ?? null };
      if (own?.status === "failed") return { fact: words.sending, failed: own.error?.message ?? "", retry: operation, why: null };
      if (own) return { fact: words.sending, failed: null, retry: null, why: null };
      return { fact: words.sending, failed: error, retry: null, why: null };
    };
    const listed = new Set((draft?.uploads ?? []).flatMap((one) => (one.operation ? [one.operation] : [])));
    const rows = [
      ...(draft?.attachments ?? []).map((one) =>
        item(one.id, one.name, one.media_type.startsWith("image/") || IMAGE.test(one.name), this.previews.get(`${one.name}:${one.size}`) ?? null, words.size(one.size), null, null),
      ),
      ...(draft?.uploads ?? []).map((one) => {
        const operation = one.operation && this.sending.has(one.operation) ? one.operation : null;
        const now = standing(operation, one.error);
        return item(one.id, one.file.name, one.file.media_type.startsWith("image/"), this.previews.get(`${one.file.name}:${one.file.size}`) ?? null, now.fact, now.failed, now.retry, now.why);
      }),
      ...[...this.sending].filter(([id]) => !listed.has(id)).map(([id, file]) => {
        const now = standing(id, null);
        return item(id, file.name, file.image, file.preview, now.fact, now.failed, now.retry, now.why);
      }),
    ];
    this.element.hidden = rows.length === 0;
    const html = rows.join("");
    if (this.element.innerHTML !== html) this.element.innerHTML = html;
  }
}
