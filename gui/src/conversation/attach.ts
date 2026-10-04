// What the user hands the agent with their words: files pasted, dropped
// onto the conversation, or picked. Each is carried to the machine the
// agent lives on as soon as it is attached, and shown in a tray above the
// words while it goes: what it is, how big, whether it has arrived. The
// words go once everything attached has; what the agent is told of each,
// and where it was put, is the World's to say.

import type { JsonValue } from "../world/wire";
import { glyph } from "./glyphs";
import { escape } from "./prose";

/** Where an attachment's bytes come from: a file on this computer, which
 *  the shell reads itself, or bytes the page holds (pasted, picked on a
 *  phone). */
export type AttachSource = { kind: "path"; path: string } | { kind: "bytes"; file: File };

export interface AttachWords {
  attach: string;
  drop(agent: string): string;
  remove: string;
  sending: string;
  failed(why: string): string;
  retry: string;
  size(bytes: number): string;
  /** Said when words are sent while something attached is still going. */
  wait: string;
}

export interface Attached {
  key: string;
  name: string;
  size: number | null;
  image: boolean;
  /** A picture of it, where the page holds its bytes. */
  preview: string | null;
  source: AttachSource;
  state: "sending" | "ready" | "failed";
  /** What the World answered once it arrived, handed back with the words. */
  ref: JsonValue | null;
  why: string | null;
}

export interface AttachSeam {
  /** Carries one attachment to the agent's machine; resolves with what
   *  the World answered for it. */
  upload(source: AttachSource, name: string, progress: (sent: number) => void): Promise<JsonValue>;
}

const IMAGE = /\.(png|jpe?g|gif|webp|bmp|avif|svg)$/i;

let serial = 0;

export class AttachTray {
  readonly element: HTMLElement;
  private items: Attached[] = [];
  private sent = new Map<string, number>();

  constructor(
    private readonly words: AttachWords,
    private readonly seam: AttachSeam,
    /** Something attached changed: the composer redraws what depends on it. */
    private readonly changed: () => void,
  ) {
    this.element = document.createElement("ul");
    this.element.className = "attach-tray";
    this.element.hidden = true;
    this.element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-remove], [data-retry]");
      if (!button) return;
      if (button.dataset.remove) this.remove(button.dataset.remove);
      else this.send(this.items.find((item) => item.key === button.dataset.retry)!);
    });
  }

  /** Files from the page: pasted, picked, or dropped in a browser. */
  addFiles(files: Iterable<File>): void {
    for (const file of files) {
      const image = file.type.startsWith("image/") || IMAGE.test(file.name);
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
      this.add({ name, size: null, image: IMAGE.test(name), preview: null, source: { kind: "path", path } });
    }
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

  /** What arrived, for sending with the words; the tray is emptied. */
  take(): { refs: JsonValue[]; shown: Attached[] } {
    const ready = this.items.filter((item) => item.state === "ready");
    this.items = [];
    this.draw();
    return { refs: ready.map((item) => item.ref!), shown: ready };
  }

  private add(fields: Omit<Attached, "key" | "state" | "ref" | "why">): void {
    serial += 1;
    const item: Attached = { ...fields, key: `attach-${serial}`, state: "sending", ref: null, why: null };
    this.items.push(item);
    this.send(item);
  }

  private send(item: Attached): void {
    item.state = "sending";
    item.why = null;
    this.sent.set(item.key, 0);
    this.draw();
    this.seam
      .upload(item.source, item.name, (sent) => {
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
    const item = this.items.find((candidate) => candidate.key === key);
    if (item?.preview) URL.revokeObjectURL(item.preview);
    this.items = this.items.filter((candidate) => candidate.key !== key);
    this.draw();
  }

  private progress(item: Attached): void {
    const bar = this.element.querySelector<HTMLElement>(`[data-item="${item.key}"] .attach-progress`);
    if (bar && item.size) bar.style.width = `${Math.min(100, (100 * (this.sent.get(item.key) ?? 0)) / item.size)}%`;
  }

  private draw(): void {
    const { words } = this;
    this.element.hidden = this.items.length === 0;
    this.element.innerHTML = this.items
      .map((item) => {
        const picture = item.preview
          ? `<img class="attach-thumb" src="${escape(item.preview)}" alt="">`
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
        return `<li class="attach-item ${item.state}" data-item="${item.key}">${picture}<span class="attach-text"><span class="attach-name" title="${escape(item.name)}">${escape(item.name)}</span>${fact}</span>${retry}<button class="attach-remove" type="button" data-remove="${item.key}" aria-label="${escape(words.remove)}" title="${escape(words.remove)}">${glyph("close", 14)}</button>${item.state === "sending" && item.size ? `<span class="attach-progress"></span>` : ""}</li>`;
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
