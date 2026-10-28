// What the user hands the agent with their words: files pasted, dropped
// onto the conversation, or picked. Each is carried to the machine the
// agent lives on as soon as it is attached, and shown in a tray above the
// words while it goes: what it is, how big, whether it has arrived. The
// words go once everything attached has; what the agent is told of each,
// and where it was put, is the World's to say.

import { leave } from "../app/leave";
import { viewAttached } from "./attached-pictures";
import { glyph } from "./glyphs";
import { escape } from "./prose";

/** Where an attachment's bytes come from: a file on this computer, which
 *  the shell reads itself, or bytes the page holds (pasted, picked on a
 *  phone). */
export type AttachSource = { kind: "path"; path: string } | { kind: "bytes"; file: File };

export interface AttachWords {
  attach: string;
  /** Taking a photo straight into the words, on a device with a camera. */
  photo: string;
  drop(agent: string): string;
  remove: string;
  sending: string;
  /** An upload whose connection broke, waiting to go on from where it stopped. */
  resuming: string;
  failed(why: string): string;
  retry: string;
  size(bytes: number): string;
  /** Said when words are sent while something attached is still going. */
  wait: string;
  /** Said when a file other than an image is offered for an agent of
   *  another's and no computer of the user's is online to keep it: images
   *  go with the conversation, other files only to the user's own. */
  imagesOnly: string;
}

export interface Attached {
  key: string;
  name: string;
  size: number | null;
  image: boolean;
  /** A picture of it, where the page holds its bytes. */
  preview: string | null;
  /** Where its bytes come from; none for one that arrived before. */
  source: AttachSource | null;
  /** Waiting is a deferred tray's: held here until the words go. */
  state: "waiting" | "sending" | "ready" | "failed";
  /** What the World answered for it, handed back with the words. */
  ref: unknown;
  why: string | null;
}

export interface AttachSeam {
  /** Carries one attachment to the agent's machine; resolves with what
   *  the World answered for it. */
  upload(source: AttachSource, name: string, progress: (sent: number) => void): Promise<unknown>;
}

const IMAGE = /\.(png|jpe?g|gif|webp|bmp|avif|svg)$/i;

let serial = 0;

export class AttachTray {
  readonly element: HTMLElement;
  private items: Attached[] = [];
  /** The cards drawn last time, which rise in no more. */
  private drawn = new Set<string>();
  private sent = new Map<string, number>();
  /** Whether files other than images are taken now: why not, when not. */
  private refusing: (() => string | null) | null = null;
  /** Says why a file offered was not taken. */
  private refused: (why: string) => void = () => {};

  constructor(
    private readonly words: AttachWords,
    private readonly seam: AttachSeam,
    /** Something attached changed: the composer redraws what depends on it. */
    private readonly changed: () => void,
    /** Holds what is attached until `sendAll`, for words whose agent is
     *  still being chosen; otherwise each file goes as it is added. */
    private readonly deferred = false,
  ) {
    this.element = document.createElement("ul");
    this.element.className = "attach-tray";
    this.element.hidden = true;
    this.element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-remove], [data-retry]");
      if (!button) {
        viewAttached(this.element, event.target as Element);
        return;
      }
      if (button.dataset.remove) this.remove(button.dataset.remove);
      else void this.send(this.items.find((item) => item.key === button.dataset.retry)!);
    });
  }

  /** Files other than images are taken only while `refusing` gives no
   *  reason; one offered then is not taken, and `refused` hears why. */
  onlyImages(refusing: () => string | null, refused: (why: string) => void): void {
    this.refusing = refusing;
    this.refused = refused;
  }

  /** Why a file other than an image is not taken now; null when it is. */
  private refusal(image: boolean): string | null {
    return image ? null : (this.refusing?.() ?? null);
  }

  /** Files from the page: pasted, picked, or dropped in a browser. */
  addFiles(files: Iterable<File>): void {
    for (const file of files) {
      const image = file.type.startsWith("image/") || IMAGE.test(file.name);
      const why = this.refusal(image);
      if (why) {
        this.refused(why);
        continue;
      }
      this.add({
        name: file.name || this.words.attach,
        size: file.size,
        image,
        preview: image ? URL.createObjectURL(file) : null,
        source: { kind: "bytes", file },
      });
    }
  }

  /** Files on this computer, by path: dropped onto the window. */
  addPaths(paths: string[]): void {
    for (const path of paths) {
      const name = path.split(/[\\/]/).pop() ?? path;
      const image = IMAGE.test(name);
      const why = this.refusal(image);
      if (why) {
        this.refused(why);
        continue;
      }
      this.add({ name, size: null, image, preview: null, source: { kind: "path", path } });
    }
  }

  /** Attachments that arrived with words before, put back with those
   *  words: they are where they arrived and go again as they are. */
  restore(kept: { name: string; size: number; media_type: string }[]): void {
    for (const ref of kept) {
      serial += 1;
      const image = ref.media_type.startsWith("image/") || IMAGE.test(ref.name);
      this.items.push({ key: `attach-${serial}`, name: ref.name, size: ref.size, image, preview: null, source: null, state: "ready", ref, why: null });
    }
    this.draw();
    this.changed();
  }

  /** Whether anything is still on its way. */
  get sending(): boolean {
    return this.items.some((item) => item.state === "sending");
  }

  get failed(): boolean {
    return this.items.some((item) => item.state === "failed");
  }

  get count(): number {
    return this.items.length;
  }

  /** How many of what is held are files other than images: those go where
   *  the agent works, not where it lives. */
  get files(): number {
    return this.items.filter((item) => !item.image).length;
  }

  /** What the World answered for each attached file that has arrived. */
  get arrived(): unknown[] {
    return this.items.filter((item) => item.state === "ready").map((item) => item.ref);
  }

  /** Sends what is held, one after another, since the first may make the
   *  place the rest go to; resolves when all have arrived, or rejects with
   *  the first that did not. */
  async sendAll(): Promise<void> {
    for (const item of [...this.items]) {
      // One taken out while the others went is not sent.
      if (item.state === "ready" || !this.items.includes(item)) continue;
      await this.send(item);
      if (item.state === "failed") throw new Error(item.why ?? item.name);
    }
  }

  /** What was sent no longer fits where the words will go (another
   *  machine, agent or project was chosen): all of it waits again. */
  reset(): void {
    if (!this.deferred) return;
    for (const item of this.items) {
      item.state = "waiting";
      item.ref = null;
      item.why = null;
    }
    this.draw();
  }

  /** What is held, as it is shown. */
  get held(): readonly Attached[] {
    return this.items;
  }

  /** What arrived, for sending with the words; the tray is emptied. */
  take(): { refs: unknown[]; shown: Attached[] } {
    const ready = this.items.filter((item) => item.state === "ready");
    this.items = [];
    this.draw();
    return { refs: ready.map((item) => item.ref!), shown: ready };
  }

  private add(fields: Omit<Attached, "key" | "state" | "ref" | "why">): void {
    serial += 1;
    const item: Attached = { ...fields, key: `attach-${serial}`, state: this.deferred ? "waiting" : "sending", ref: null, why: null };
    this.items.push(item);
    if (this.deferred) this.draw();
    else void this.send(item);
  }

  private send(item: Attached): Promise<void> {
    const source = item.source;
    if (!source) return Promise.resolve();
    item.state = "sending";
    item.why = null;
    this.sent.set(item.key, 0);
    this.draw();
    return this.seam
      .upload(source, item.name, (sent) => {
        this.sent.set(item.key, sent);
        this.progress(item);
      })
      .then((ref) => {
        item.ref = ref;
        item.state = "ready";
      })
      .catch((error: unknown) => {
        item.state = "failed";
        item.why = String(error);
      })
      .finally(() => {
        if (this.items.includes(item)) this.draw();
      });
  }

  private remove(key: string): void {
    const gone = () => {
      const item = this.items.find((candidate) => candidate.key === key);
      if (!item) return;
      if (item.preview) URL.revokeObjectURL(item.preview);
      this.items = this.items.filter((candidate) => candidate !== item);
      this.draw();
    };
    // It sinks out of the tray before it goes.
    const card = this.element.querySelector<HTMLElement>(`[data-item="${key}"]`);
    if (card) leave(card, gone);
    else gone();
  }

  private progress(item: Attached): void {
    const bar = this.element.querySelector<HTMLElement>(`[data-item="${item.key}"] .attach-progress`);
    if (bar && item.size) bar.style.width = `${Math.min(100, (100 * (this.sent.get(item.key) ?? 0)) / item.size)}%`;
  }

  private draw(): void {
    const { words } = this;
    this.element.hidden = this.items.length === 0;
    // A card rises into the tray the first time it is drawn, never again.
    const drawn = this.drawn;
    this.drawn = new Set(this.items.map((item) => item.key));
    this.element.innerHTML = this.items
      .map((item) => {
        const picture = item.preview
          ? `<img class="attach-thumb" src="${escape(item.preview)}" alt="${escape(item.name)}">`
          : `<span class="attach-thumb attach-glyph">${glyph(item.image ? "image" : "file", 18)}</span>`;
        const fact =
          item.state === "failed"
            ? `<span class="attach-fact wrong" title="${escape(item.why ?? "")}">${escape(words.failed(item.why ?? ""))}</span>`
            : item.state === "sending"
              ? `<span class="attach-fact">${escape(words.sending)}</span>`
              : item.size !== null
                ? `<span class="attach-fact">${escape(words.size(item.size))}</span>`
                : "";
        const retry =
          item.state === "failed" ? `<button class="attach-act" type="button" data-retry="${item.key}">${escape(words.retry)}</button>` : "";
        return `<li class="attach-item ${item.state}${drawn.has(item.key) ? "" : " fresh"}" data-item="${item.key}">${picture}<span class="attach-text"><span class="attach-name" title="${escape(item.name)}">${escape(item.name)}</span>${fact}</span>${retry}<button class="attach-remove" type="button" data-remove="${item.key}" aria-label="${escape(words.remove)}" title="${escape(words.remove)}">${glyph("close", 14)}</button>${item.state === "sending" && item.size ? `<span class="attach-progress"></span>` : ""}</li>`;
      })
      .join("");
    for (const item of this.items) this.progress(item);
    this.changed();
  }
}

/** A size as people say it: 840 B, 12 KB, 3.4 MB. */
export function sizeOf(bytes: number, units: [string, string, string, string]): string {
  if (bytes < 1024) return `${bytes} ${units[0]}`;
  if (bytes < 1024 ** 2) return `${Math.round(bytes / 1024)} ${units[1]}`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} ${units[2]}`;
  return `${(bytes / 1024 ** 3).toFixed(1)} ${units[3]}`;
}
