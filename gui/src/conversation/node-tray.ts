// What is attached to the shared words, as the node of the agent's machine
// keeps them (`world-gui.md`, 对话: the input is one for every device). Every
// device shows the same files; a file goes to that machine as soon as it is
// attached, and any device can take one out. What this device is still
// sending, before the node lists it, is shown too, with its picture where
// the page holds the bytes.

import type { ComposerSession, ComposerSessionState } from "./composer";
import type { ComposerDocument } from "./composer";
import type { AttachWords } from "./attach";
import { glyph } from "./glyphs";
import { escape } from "./prose";

const IMAGE = /\.(png|jpe?g|gif|webp|bmp|avif|svg)$/i;

interface Sending {
  name: string;
  size: number | null;
  image: boolean;
  preview: string | null;
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
    /** Something could not be attached or taken out: said by the composer. */
    private readonly failed: (why: string) => void,
  ) {
    this.element = window.document.createElement("ul");
    this.element.className = "attach-tray";
    this.element.hidden = true;
    this.element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-remove], [data-retry]");
      if (!button) return;
      if (button.dataset.retry) {
        this.session.retryOperation(button.dataset.retry).catch((error: unknown) => this.failed(String(error)));
        return;
      }
      const id = button.dataset.remove!;
      if (this.sending.has(id)) {
        this.sending.delete(id);
        try {
          this.session.forgetOperation(id);
        } catch {
          // Still on its way: it is listed again once the node has it.
        }
        this.draw();
        return;
      }
      this.session.detach(id, this.document).catch((error: unknown) => this.failed(String(error)));
    });
  }

  /** The words this tray holds the files of: the input's, or a queued
   *  one's being changed. */
  private draft(state: ComposerSessionState | null = this.state) {
    const target = this.document.view.state.target;
    if (!state) return null;
    return target.kind === "draft" ? state.composer.draft : (state.composer.queue.find((item) => item.id === target.id)?.draft ?? null);
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

  addFiles(files: Iterable<File>): void {
    for (const file of files) {
      const image = file.type.startsWith("image/") || IMAGE.test(file.name);
      const preview = image ? URL.createObjectURL(file) : null;
      if (preview) this.previews.set(`${file.name}:${file.size}`, preview);
      this.start({ name: file.name || this.words.attach, size: file.size, image, preview }, () => this.session.attachFile(file, this.document));
    }
  }

  addPaths(paths: string[]): void {
    for (const path of paths) {
      const name = path.split(/[\\/]/).pop() ?? path;
      this.start({ name, size: null, image: IMAGE.test(name), preview: null }, () => this.session.attachPath(path, () => {}, this.document));
    }
  }

  private start(file: Sending, upload: () => ReturnType<ComposerSession["attachFile"]>): void {
    const before = new Set(this.session.state.operations.map((operation) => operation.id));
    const done = upload();
    const operation = this.session.state.operations.find((one) => !before.has(one.id));
    if (operation) this.sending.set(operation.id, file);
    this.draw();
    done.then(
      (added) => {
        if (operation) this.sending.delete(operation.id);
        if ("Err" in added.updated) this.failed(this.words.failed(added.updated.Err));
        this.draw();
      },
      (error: unknown) => {
        this.failed(this.words.failed(String(error)));
        this.draw();
      },
    );
  }

  draw(state: ComposerSessionState | null = this.state): void {
    this.state = state;
    const { words } = this;
    const draft = this.draft(state);
    const operations = new Map((state?.operations ?? []).map((operation) => [operation.id, operation]));
    const item = (key: string, name: string, image: boolean, preview: string | null, fact: string, failed: string | null, retry: string | null) => {
      const picture = preview ? `<img class="attach-thumb" src="${escape(preview)}" alt="">` : `<span class="attach-thumb attach-glyph">${glyph(image ? "image" : "file", 18)}</span>`;
      const said = failed ? `<span class="attach-fact wrong" title="${escape(failed)}">${escape(words.failed(failed))}</span>` : fact ? `<span class="attach-fact">${escape(fact)}</span>` : "";
      const again = retry ? `<button class="attach-act" type="button" data-retry="${escape(retry)}">${escape(words.retry)}</button>` : "";
      return `<li class="attach-item ${failed ? "failed" : fact === words.sending ? "sending" : "ready"}">${picture}<span class="attach-text"><span class="attach-name" title="${escape(name)}">${escape(name)}</span>${said}</span>${again}<button class="attach-remove" type="button" data-remove="${escape(key)}" aria-label="${escape(words.remove)}" title="${escape(words.remove)}">${glyph("close", 14)}</button></li>`;
    };
    const rows = [
      ...(draft?.attachments ?? []).map((one) =>
        item(one.id, one.name, one.media_type.startsWith("image/") || IMAGE.test(one.name), this.previews.get(`${one.name}:${one.size}`) ?? null, words.size(one.size), null, null),
      ),
      ...(draft?.uploads ?? []).map((one) =>
        item(one.id, one.file.name, one.file.media_type.startsWith("image/"), this.previews.get(`${one.file.name}:${one.file.size}`) ?? null, words.sending, one.error, null),
      ),
      ...[...this.sending].map(([id, file]) => {
        const operation = operations.get(id);
        const failed = operation?.status === "failed" ? (operation.error?.message ?? "") : null;
        return item(id, file.name, file.image, file.preview, words.sending, failed, failed !== null ? id : null);
      }),
    ];
    this.element.hidden = rows.length === 0;
    const html = rows.join("");
    if (this.element.innerHTML !== html) this.element.innerHTML = html;
  }
}
