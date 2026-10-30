// The pictures of a conversation. Small ones are drawn in their place as
// they come into view, made by the machine holding each and never kept
// there; pressed, one opens in a card over the page, where it is dragged,
// zoomed with Ctrl and the wheel or two fingers, and the conversation's
// other pictures are a key or a swipe away. A picture is named by what its
// element carries: a file the user sent (`data-attachment`), a file on a
// machine (`data-path`), or bytes already in the page (`src`). A whole
// picture read for a closer look is let go of when the viewer moves on; one
// read to be saved goes on through breaks while the page is open, its bar
// saying how it comes.

import { onShade, watchShade } from "../app/shade-press";
import { layerOpened } from "../app/back";
import { CanvasCamera } from "../map-canvas";
import type { MachineName } from "../world/events";
import type { AttachmentSource } from "../world/files";
import { HeldReads, LetGo, shareOf, type HeldRead, type ReadProgress } from "../world/reads";
import { glyph } from "./glyphs";
import { escape, naturalPath } from "./prose";

/** The longer side of the small copy a conversation keeps of a picture
 *  sent from another machine. */
const PREVIEW = 320;

export interface PictureWords {
  close: string;
  previous: string;
  next: string;
  copyImage: string;
  saveAs: string;
  copyPath: string;
  reveal: string;
  copied: string;
  saved: string;
  /** The original is nowhere to be had. */
  gone: string;
  failed(why: string): string;
  /** Which of how many, over the picture. */
  counter(at: number, of: number): string;
  /** A picture being saved that takes a while to come. */
  fetching(share: number | null): string;
  /** Its way broke; it comes on once the way is back. */
  interrupted: string;
}

/** A picture as the seams are asked for it: a file the user sent (as the
 *  World kept it, or as its host recorded it), or a file on a machine. */
export interface PictureSource {
  name: string;
  machine: MachineName;
  path: string | null;
  attachment: AttachmentSource | null;
}

export interface PictureSeams {
  /** A file on `machine`, made at most `max` pixels on its longer side by
   *  the machine holding it, as an address an `<img>` takes. */
  thumbnail(machine: MachineName, path: string, max: number): Promise<string>;
  /** The same for a file the user sent, which an agent assisting the user
   *  may show too. A failure that carries `gone` says whether the picture
   *  is nowhere to be had. */
  attachmentThumbnail(attachment: AttachmentSource, max: number): Promise<string>;
  /** The whole picture, `max` pixels being what it is known to hold, for
   *  looking closer than a screen's worth, as the read `held`. */
  original(picture: PictureSource, max: number, held: HeldRead): Promise<Blob>;
  /** Saves a copy where the user picks on this device, brought here as
   *  the read `held`; false when they changed their mind. Absent where
   *  this device has no way to. */
  save?: ((picture: PictureSource, held: HeldRead) => Promise<boolean>) | undefined;
  /** Shows a file on this computer in its folder; absent on a phone. */
  reveal?: ((path: string) => Promise<void>) | undefined;
}

export interface PicturesOptions {
  words: PictureWords;
  /** This computer. */
  here: MachineName;
  /** The conversation's own machine, whose files the agent's words name
   *  where nothing else says. */
  machine: MachineName;
  /** The machine the file `path` that `element` shows is on, as the turn
   *  its words are in says; absent where it says nothing. */
  machineFor?: ((element: HTMLElement, path: string) => MachineName | null) | undefined;
  seams: PictureSeams;
  /** A line beside the composer; `detail`, the original of a failure, on
   *  hover. */
  say(text: string, detail?: string): void;
  /** For how long the way to a picture has been broken, since `since`
   *  (milliseconds since the epoch), and why. */
  broken(since: number, why: string): string;
}

/** One picture as the viewer knows it. */
interface Picture extends PictureSource {
  element: HTMLImageElement;
}

/** Pixels on the longer side of a picture in the conversation: twice what
 *  the largest of them is drawn at (`.bubble-picture img`, 260 CSS pixels),
 *  for screens of twice the density. */
const SMALL = 520;
/** The most pictures kept made at once: the one asked for longest ago goes
 *  first, and is asked of its machine again if it comes back into view. */
const KEPT = 200;

/** Whether a failure says the picture is nowhere to be had: a file the user
 *  sent says so itself; a file on a machine is gone when the system says
 *  it is not there. */
const isGone = (error: unknown, message: string): boolean =>
  error && typeof error === "object" && "gone" in error
    ? error.gone === true
    : /not found|no such file|cannot find|does not exist|is gone|os error 2\b/i.test(message);

export class Pictures {
  private readonly made = new Map<string, Promise<string>>();
  private readonly watching: IntersectionObserver;
  private readonly arriving: MutationObserver;
  private viewer: PictureViewer | null = null;
  /** Pictures being saved, which outlive the viewer they were saved from. */
  readonly saves = new HeldReads();

  constructor(
    private readonly root: HTMLElement,
    private readonly options: PicturesOptions,
  ) {
    this.watching = new IntersectionObserver(
      (seen) => {
        for (const one of seen) {
          if (!one.isIntersecting) continue;
          this.watching.unobserve(one.target);
          void this.draw(one.target as HTMLImageElement);
        }
      },
      // Asked for a little before they scroll in, so they are there when
      // they arrive.
      { root: null, rootMargin: "400px 0px" },
    );
    const look = (within: ParentNode) => {
      for (const image of within.querySelectorAll<HTMLImageElement>("img[data-picture]:not([src])")) this.watching.observe(image);
    };
    this.arriving = new MutationObserver((changes) => {
      for (const change of changes) for (const node of change.addedNodes) if (node instanceof Element) look(node.parentElement ?? node);
    });
    this.arriving.observe(root, { childList: true, subtree: true });
    look(root);
    root.addEventListener("click", (event) => {
      const image = (event.target as Element).closest<HTMLImageElement>("img[data-picture]");
      if (!image || !image.src) return;
      event.preventDefault();
      this.open(image);
    });
  }

  private pictureOf(element: HTMLImageElement): Picture {
    const attachment = element.dataset.attachment ? (JSON.parse(element.dataset.attachment) as AttachmentSource) : null;
    const kept = attachment && "path" in attachment ? attachment : null;
    return {
      element,
      name: element.alt || kept?.name || element.dataset.path?.split(/[\\/]/).pop() || "",
      machine: attachment?.owner.machine ?? element.dataset.machine ?? (element.dataset.path ? this.options.machineFor?.(element, element.dataset.path) : null) ?? this.options.machine,
      path: kept?.path ?? element.dataset.path ?? null,
      attachment,
    };
  }

  /** A picture of `max` pixels, asked of the machine holding it once. */
  sized(picture: Picture, max: number): Promise<string> | null {
    const key = picture.attachment ? `a:${JSON.stringify(picture.attachment)}:${max}` : picture.path ? `f:${picture.machine}:${picture.path}:${max}` : null;
    if (!key) return null;
    const kept = this.made.get(key);
    if (kept) {
      // The latest asked for is the last to go.
      this.made.delete(key);
      this.made.set(key, kept);
      return kept;
    }
    const made = picture.attachment
      ? this.options.seams.attachmentThumbnail(picture.attachment, max)
      : this.options.seams.thumbnail(picture.machine, picture.path!, max);
    // A failure is not kept: the next look asks again.
    made.catch(() => {
      if (this.made.get(key) === made) this.made.delete(key);
    });
    this.made.set(key, made);
    for (const oldest of this.made.keys()) {
      if (this.made.size <= KEPT) break;
      this.made.delete(oldest);
    }
    return made;
  }

  private async draw(image: HTMLImageElement): Promise<void> {
    const picture = this.pictureOf(image);
    const made = this.sized(picture, SMALL);
    if (!made) return;
    try {
      image.src = await made;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      // Said where the picture would be: in its own frame, or in a note
      // put where the picture was.
      let place = image.closest<HTMLElement>(".bubble-picture, .prose-picture, .dd-picture");
      if (!place) {
        place = document.createElement("span");
        image.replaceWith(place);
      }
      place.classList.add("picture-missing");
      place.title = message;
      place.innerHTML = `${glyph("image", 16)}<span>${escape(picture.name)}</span><span class="picture-why">${escape(isGone(error, message) ? this.options.words.gone : this.options.words.failed(message))}</span>`;
    }
  }

  /** The page went: what is still coming is not wanted. */
  /** The page is gone for good: what it watched and the pictures it made
   *  are let go. */
  release(): void {
    this.viewer?.close();
    this.saves.release();
    this.watching.disconnect();
    this.arriving.disconnect();
    this.made.clear();
  }

  private open(image: HTMLImageElement): void {
    const all = [...this.root.querySelectorAll<HTMLImageElement>("img[data-picture][src]")];
    this.viewer?.close();
    this.viewer = new PictureViewer(
      all.map((element) => this.pictureOf(element)),
      Math.max(0, all.indexOf(image)),
      this,
      this.options,
      () => {
        this.viewer = null;
        image.focus({ preventScroll: true });
      },
    );
  }
}

/** Pictures the page itself holds, being attached and not sent yet, in the
 *  same viewer: `index` of `images` first, the others a key or a swipe
 *  away. Nothing of them is on a machine yet, so the viewer only shows. */
export function viewPictures(images: HTMLImageElement[], index: number, words: PictureWords): void {
  const nowhere = (): Promise<never> => Promise.reject(new Error("a picture being attached is not on a machine yet"));
  const options: PicturesOptions = {
    words,
    here: "",
    machine: "",
    seams: { thumbnail: nowhere, attachmentThumbnail: nowhere, original: nowhere },
    say: () => {},
    broken: () => "",
  };
  const pictures = images.map((element) => ({ element, name: element.alt, machine: "", path: null, attachment: null }));
  const opener = images[index];
  new PictureViewer(pictures, index, null, options, () => opener?.focus({ preventScroll: true }));
}

/** A card over the page given to one picture at a time. */
class PictureViewer {
  private readonly stage: HTMLElement;
  private readonly image: HTMLImageElement;
  private camera: CanvasCamera | null = null;
  private size: [number, number] = [1, 1];
  private fitted = 1;
  private index: number;
  /** Which picture the loads under way are for: one that comes back after
   *  the user moved on is let go. */
  private shown = 0;
  private original: string | null = null;
  /** The whole picture read for a closer look, let go of with the picture. */
  private readonly looks = new HeldReads();
  /** How the save under way comes, said in the bar. */
  private readonly note: HTMLElement;
  private broken: { since: number; why: string } | null = null;
  private readonly pointers = new Map<number, [number, number]>();
  private swipe: { start: [number, number]; fitted: boolean } | null = null;
  private readonly resize: ResizeObserver;
  private readonly keys: (event: KeyboardEvent) => void;
  private readonly element: HTMLDialogElement;

  constructor(
    private readonly pictures: Picture[],
    index: number,
    /** Where a larger picture is asked for; none for pictures the page holds. */
    private readonly maker: Pictures | null,
    private readonly options: PicturesOptions,
    private readonly closed: () => void,
  ) {
    this.index = index;
    const { words } = options;
    // A card over the page, as long text is viewed: the window's own bar and
    // the page's edges stay where they are, and a press beside it closes it.
    this.element = document.createElement("dialog");
    this.element.className = "picture-viewer";
    this.element.setAttribute("aria-label", words.close);
    const button = (act: string, icon: Parameters<typeof glyph>[0], label: string) =>
      `<button class="icon-button pv-act" type="button" data-pv="${act}" aria-label="${escape(label)}" title="${escape(label)}">${glyph(icon, 18)}</button>`;
    this.element.innerHTML = `<div class="pv-card">
      <div class="pv-bar"><span class="pv-name"></span><span class="pv-counter"></span><span class="pv-note" aria-live="polite" hidden></span><span class="pv-acts">${
        typeof ClipboardItem === "undefined" ? "" : button("copy-image", "copy", words.copyImage)
      }${options.seams.save ? button("save", "save", words.saveAs) : ""}${button("copy-path", "link", words.copyPath)}${
        options.seams.reveal ? button("reveal", "folder", words.reveal) : ""
      }${button("close", "close", words.close)}</span></div>
      <div class="pv-stage"><img class="pv-image" alt="" draggable="false">
      <button class="pv-side pv-previous" type="button" data-pv="previous" aria-label="${escape(words.previous)}" title="${escape(words.previous)}">${glyph("back", 22)}</button>
      <button class="pv-side pv-next" type="button" data-pv="next" aria-label="${escape(words.next)}" title="${escape(words.next)}">${glyph("back", 22)}</button></div>
    </div>`;
    this.stage = this.element.querySelector(".pv-stage")!;
    this.image = this.element.querySelector(".pv-image")!;
    this.note = this.element.querySelector(".pv-note")!;
    // How long it has been broken is as of when the pointer comes to it.
    this.note.addEventListener("pointerover", () => {
      if (this.broken) this.note.title = this.options.broken(this.broken.since, this.broken.why);
    });
    document.body.append(this.element);
    watchShade(this.element);
    this.element.addEventListener("click", (event) => {
      // The dialog itself is pressed only beside the card.
      if (onShade(this.element, event)) {
        this.close();
        return;
      }
      const act = (event.target as Element).closest<HTMLElement>("[data-pv]")?.dataset.pv;
      if (act) void this.act(act);
    });
    this.element.addEventListener("cancel", (event) => {
      event.preventDefault();
      this.close();
    });
    this.keys = (event) => {
      if (event.key === "Escape") this.close();
      else if (event.key === "ArrowLeft") this.go(-1);
      else if (event.key === "ArrowRight") this.go(1);
      else if (event.key === "+" || event.key === "=") this.zoom(1.25);
      else if (event.key === "-") this.zoom(0.8);
      else if (event.key === "0") this.fit();
      else return;
      event.preventDefault();
      event.stopPropagation();
    };
    document.addEventListener("keydown", this.keys, true);
    this.stage.addEventListener("pointerdown", (event) => this.down(event));
    this.stage.addEventListener("pointermove", (event) => this.move(event));
    this.stage.addEventListener("pointerup", (event) => this.up(event));
    this.stage.addEventListener("pointercancel", (event) => this.up(event));
    this.stage.addEventListener("wheel", (event) => this.wheel(event), { passive: false });
    this.stage.addEventListener("dblclick", (event) => {
      const at = this.local(event);
      if (this.camera && this.camera.view.scale > this.fitted * 1.01) this.fit();
      else this.zoom(2.5, at);
    });
    this.resize = new ResizeObserver(() => {
      if (!this.camera) return;
      const wasFitted = Math.abs(this.camera.view.scale - this.fitted) < this.fitted * 0.01;
      this.camera.resize(this.stage.clientWidth, this.stage.clientHeight);
      this.fitted = this.fitScale();
      if (wasFitted) this.fit();
      else this.paint();
    });
    this.resize.observe(this.stage);
    this.element.showModal();
    // The system back closes it, as Esc does.
    layerOpened({ isOpen: () => this.element.isConnected, close: () => this.close() });
    this.show();
    this.element.querySelector<HTMLElement>("[data-pv=close]")!.focus({ preventScroll: true });
  }

  close(): void {
    if (!this.element.isConnected) return;
    this.looks.release();
    document.removeEventListener("keydown", this.keys, true);
    this.resize.disconnect();
    if (this.original) URL.revokeObjectURL(this.original);
    this.element.close();
    this.element.remove();
    this.closed();
  }

  private get picture(): Picture {
    return this.pictures[this.index]!;
  }

  private go(step: number): void {
    const next = this.index + step;
    if (next < 0 || next >= this.pictures.length) return;
    this.index = next;
    this.show();
  }

  /** The picture as it is in the page at once, then one the size of the
   *  screen from the machine holding it. */
  private show(): void {
    const picture = this.picture;
    const shown = ++this.shown;
    this.looks.release();
    if (this.original) URL.revokeObjectURL(this.original);
    this.original = null;
    this.camera = null;
    this.element.querySelector(".pv-name")!.textContent = picture.name;
    this.element.querySelector(".pv-counter")!.textContent = this.pictures.length > 1 ? this.options.words.counter(this.index + 1, this.pictures.length) : "";
    this.element.querySelector<HTMLElement>(".pv-previous")!.hidden = this.index === 0;
    this.element.querySelector<HTMLElement>(".pv-next")!.hidden = this.index === this.pictures.length - 1;
    for (const act of ["copy-path", "save", "reveal"]) {
      const button = this.element.querySelector<HTMLElement>(`[data-pv=${act}]`);
      // Saving takes a file the user sent even where only its host's record
      // holds it; its path and folder are the World's copy's.
      if (button) button.hidden = act === "save" ? !picture.path && !picture.attachment : !picture.path || (act === "reveal" && picture.machine !== this.options.here);
    }
    this.place(picture.element.src, shown);
    const screen = Math.round(Math.max(screen_.width, screen_.height) * devicePixelRatio);
    // Without the larger one the picture stays as the page had it.
    void this.maker?.sized(picture, screen)?.then(
      (larger) => {
        this.place(larger, shown);
        // Made no larger than the small copy a conversation keeps of what
        // was sent from another machine, whose sender alone keeps it whole:
        // the whole one is asked for now, not once the user zooms in.
        const probe = new Image();
        probe.onload = () => {
          if (shown === this.shown && screen > PREVIEW && Math.max(probe.naturalWidth, probe.naturalHeight) <= PREVIEW) this.whole(Math.max(probe.naturalWidth, probe.naturalHeight));
        };
        probe.src = larger;
      },
      (error: unknown) => console.warn("a larger picture could not be had", error),
    );
  }

  /** Puts `source` in, keeping where the user looks: the first picture of
   *  a page sets its size on the drawing, a finer one fills the same. */
  private place(source: string, shown: number): void {
    if (shown !== this.shown) return;
    const probe = new Image();
    probe.onload = () => {
      if (shown !== this.shown) return;
      if (!this.camera) {
        this.size = [probe.naturalWidth || 1, probe.naturalHeight || 1];
        this.image.style.width = `${this.size[0]}px`;
        this.image.style.height = `${this.size[1]}px`;
        this.camera = new CanvasCamera({ minScale: 0.01, maxScale: 64, levels: [1] });
        this.camera.resize(this.stage.clientWidth, this.stage.clientHeight);
        this.fitted = this.fitScale();
        this.fit();
      }
      this.image.src = source;
      this.image.dataset.natural = String(probe.naturalWidth);
    };
    probe.src = source;
  }

  private fitScale(): number {
    const padding = Math.max(12, Math.min(40, Math.min(this.stage.clientWidth, this.stage.clientHeight) * 0.04));
    return Math.min((this.stage.clientWidth - padding * 2) / this.size[0], (this.stage.clientHeight - padding * 2) / this.size[1], 4);
  }

  private fit(): void {
    if (!this.camera) return;
    this.fitted = this.fitScale();
    const view = this.camera.view;
    const scale = this.fitted;
    this.camera.set({ ...view, scale, x: (view.width - this.size[0] * scale) / 2, y: (view.height - this.size[1] * scale) / 2 });
    this.paint();
  }

  private zoom(factor: number, at?: [number, number]): void {
    if (!this.camera) return;
    const scale = this.camera.view.scale * factor;
    if (scale < this.fitted * 0.5 || scale > Math.max(this.fitted * 16, 8)) return;
    this.camera.zoom(factor, at);
    this.paint();
    this.closer();
  }

  /** Closer than the screen-sized picture holds: the whole one is asked
   *  for, once. */
  private closer(): void {
    if (!this.camera) return;
    const natural = Number(this.image.dataset.natural || this.size[0]);
    if ((this.camera.view.scale * this.size[0] * devicePixelRatio) / natural <= 1.05) return;
    this.whole(natural);
  }

  /** The whole picture, asked for once; `natural` pixels being what the
   *  one shown holds. */
  private whole(natural: number): void {
    const picture = this.picture;
    if (this.original || (!picture.path && !picture.attachment)) return;
    const shown = this.shown;
    this.original = "asked";
    this.looks.hold((held) => this.options.seams.original(picture, natural, held)).then(
      (blob) => {
        if (shown !== this.shown) return;
        this.original = URL.createObjectURL(blob);
        this.place(this.original, shown);
      },
      // An account an agent assists may not read the original itself;
      // the screen-sized one stays.
      () => {
        if (shown === this.shown) this.original = null;
      },
    );
  }

  private paint(): void {
    if (!this.camera) return;
    const { x, y, scale } = this.camera.view;
    this.image.style.transform = `translate(${x}px, ${y}px) scale(${scale})`;
  }

  private local(event: MouseEvent): [number, number] {
    const box = this.stage.getBoundingClientRect();
    return [event.clientX - box.left, event.clientY - box.top];
  }

  private down(event: PointerEvent): void {
    if (event.button !== 0 && event.pointerType === "mouse") return;
    this.stage.setPointerCapture(event.pointerId);
    this.pointers.set(event.pointerId, this.local(event));
    this.swipe =
      this.pointers.size === 1 && this.camera ? { start: this.local(event), fitted: this.camera.view.scale <= this.fitted * 1.01 } : null;
  }

  private move(event: PointerEvent): void {
    const before = this.pointers.get(event.pointerId);
    if (!before || !this.camera) return;
    const now = this.local(event);
    if (this.pointers.size === 1) {
      // At its fitted size a picture turns over rather than moving.
      if (!this.swipe?.fitted) this.camera.pan(now[0] - before[0], now[1] - before[1]);
    } else if (this.pointers.size === 2) {
      const other = [...this.pointers].find(([id]) => id !== event.pointerId)![1];
      const apart = Math.hypot(before[0] - other[0], before[1] - other[1]) || 1;
      const factor = Math.hypot(now[0] - other[0], now[1] - other[1]) / apart;
      const middle: [number, number] = [(now[0] + other[0]) / 2, (now[1] + other[1]) / 2];
      this.camera.pan((now[0] - before[0]) / 2, (now[1] - before[1]) / 2);
      const scale = this.camera.view.scale * factor;
      if (scale >= this.fitted * 0.5 && scale <= Math.max(this.fitted * 16, 8)) this.camera.zoom(factor, middle);
      this.swipe = null;
    }
    this.pointers.set(event.pointerId, now);
    this.paint();
  }

  private up(event: PointerEvent): void {
    const at = this.pointers.get(event.pointerId);
    this.pointers.delete(event.pointerId);
    if (this.swipe?.fitted && at && this.pointers.size === 0) {
      const dx = at[0] - this.swipe.start[0];
      const dy = at[1] - this.swipe.start[1];
      if (Math.abs(dx) > 64 && Math.abs(dx) > Math.abs(dy) * 1.5) this.go(dx < 0 ? 1 : -1);
    }
    if (this.pointers.size === 0) {
      this.swipe = null;
      this.closer();
    }
  }

  private wheel(event: WheelEvent): void {
    if (!this.camera) return;
    event.preventDefault();
    // A trackpad's pinch comes as the wheel with Ctrl.
    if (event.ctrlKey) this.zoom(Math.exp(-event.deltaY / 240), this.local(event));
    else {
      this.camera.pan(-event.deltaX, -event.deltaY);
      this.paint();
    }
  }

  private async act(act: string): Promise<void> {
    const { words, seams, say } = this.options;
    const picture = this.picture;
    try {
      if (act === "close") this.close();
      else if (act === "previous") this.go(-1);
      else if (act === "next") this.go(1);
      else if (act === "copy-path" && picture.path) {
        await navigator.clipboard.writeText(naturalPath(picture.path));
        say(words.copied);
      } else if (act === "reveal" && picture.path && seams.reveal) await seams.reveal(picture.path);
      else if (act === "save" && this.maker && (picture.path || picture.attachment) && seams.save) {
        const [save, saves] = [seams.save, this.maker.saves];
        try {
          if (await saves.hold((held) => save(picture, held), (progress) => this.tell(progress))) say(words.saved);
        } finally {
          this.tell(null);
        }
      } else if (act === "copy-image") {
        await navigator.clipboard.write([new ClipboardItem({ "image/png": pngOf(this.image) })]);
        say(words.copied);
      }
    } catch (error) {
      if (error instanceof LetGo) return;
      const why = error instanceof Error ? error.message : String(error);
      say(words.failed(why), why);
    }
  }

  /** How a picture being saved comes, in the bar: once it is large enough
   *  to take a while, or its way broke. */
  private tell(progress: ReadProgress | null): void {
    const { words } = this.options;
    this.broken = progress?.interrupted ?? null;
    const share = shareOf(progress);
    const text = this.broken ? words.interrupted : share === null ? "" : words.fetching(share);
    this.note.textContent = text;
    this.note.hidden = !text;
    if (!this.broken) this.note.removeAttribute("title");
  }
}

/** The screen the window is on; named so it does not stand for the
 *  viewer's own sizes. */
const screen_ = window.screen;

/** A picture as PNG, the one kind every clipboard takes. */
function pngOf(image: HTMLImageElement): Promise<Blob> {
  const canvas = document.createElement("canvas");
  canvas.width = image.naturalWidth;
  canvas.height = image.naturalHeight;
  canvas.getContext("2d")!.drawImage(image, 0, 0);
  return new Promise((resolve, reject) => canvas.toBlob((blob) => (blob ? resolve(blob) : reject(new Error("the picture could not be copied"))), "image/png"));
}
