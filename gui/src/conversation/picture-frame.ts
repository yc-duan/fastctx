// The frame a picture the user sent stands in above their words (`view.ts`,
// `attached`), from the moment the words are said. It is as large as the
// picture will be drawn, so nothing moves when the picture comes: one alone
// at the size its own shape gives it within the bounds of a picture above
// words, several as the squares they are drawn as. Until the picture has
// come the frame breathes faintly and, once it has waited a moment, the
// dots of a wait breathe in turn at its middle (`app/waiting.css`); no mark
// of a picture and no chip with its name stand in for it (the user,
// 2026-11-17: "不要那个图片的默认图标…他是一个和加载出图片之后大小一样的
// 东西，然后里面加载中的动效"). The picture then comes in where the frame
// stood.
//
// How large a picture is drawn is known once it has been seen: from the
// card it had in the input on the device that sent it, or from the small
// copy made of it, and is kept on this device by the file's digest. One
// never seen here waits in a frame of four by three, which goes to the
// picture's size as it comes rather than leaping there. That a file kept
// by the World says its picture's size itself is Open: it would let every
// device frame it rightly the first time.

import { arrive } from "../app/arrive";
import { msOf, reducedMotion } from "../motion";
import { escape } from "./prose";

type Size = readonly [number, number];

/** The bounds of a picture alone above its words, in CSS pixels: no wider,
 *  no taller, and no smaller than these. */
const WIDEST = 260;
const TALLEST = 220;
const NARROWEST = 120;
const SHORTEST = 90;
/** What a picture alone is taken to be while its size is not known. */
const UNKNOWN: Size = [160, 120];
/** Where this device keeps the sizes of pictures it has seen, and how many:
 *  the latest seen stay. */
const KEPT = "picture-sizes";
const MOST = 400;
/** A picture that came within this of its frame, in milliseconds, is there
 *  at once: it does not fade in. */
const AT_ONCE = 150;

let seen: Map<string, Size> | null = null;
/** When each picture not come yet first stood waiting, by what names it:
 *  a frame drawn again goes on waiting, it does not start over. */
const since = new Map<string, number>();

/** A file's digest names its picture on every device and is kept; anything
 *  else, such as the address of a picture the page holds, lasts the page. */
const digest = (key: string) => /^[0-9a-f]{64}$/.test(key);

function sizes(): Map<string, Size> {
  if (seen) return seen;
  seen = new Map();
  try {
    for (const [key, width, height] of JSON.parse(localStorage.getItem(KEPT) ?? "[]") as [string, number, number][]) {
      if (typeof key === "string" && width > 0 && height > 0) seen.set(key, [width, height]);
    }
  } catch {
    // Unreadable: the sizes are learned again as the pictures come.
  }
  return seen;
}

/** The size of the picture any of `keys` names, where it has been seen. */
function sizeOf(keys: readonly string[]): Size | null {
  for (const key of keys) {
    const size = sizes().get(key);
    if (size) return size;
  }
  return null;
}

/** Keeps the size of the picture `key` names, as seen just now. */
export function rememberSize(key: string | null | undefined, width: number, height: number): void {
  if (!key || !(width > 0) || !(height > 0)) return;
  const all = sizes();
  const had = all.get(key);
  // The whole picture and the small copy of it are drawn alike.
  if (had && fitted(had).join() === fitted([width, height]).join()) return;
  all.delete(key);
  all.set(key, [width, height]);
  for (const oldest of all.keys()) {
    if (all.size <= MOST) break;
    all.delete(oldest);
  }
  if (!digest(key)) return;
  try {
    localStorage.setItem(KEPT, JSON.stringify([...all].filter(([name]) => digest(name)).map(([name, size]) => [name, ...size])));
  } catch {
    // Not kept: the size holds until the page goes.
  }
}

/** The size a picture of `width` by `height` alone above its words is
 *  drawn at: its own, brought within the bounds as a picture of no set
 *  size is (CSS 2.1, 10.4), keeping its shape wherever both bounds allow. */
export function fitted([width, height]: Size): Size {
  const wide = width > WIDEST;
  const narrow = width < NARROWEST;
  const tall = height > TALLEST;
  const short = height < SHORTEST;
  let size: Size = [width, height];
  if (wide && short) size = [WIDEST, SHORTEST];
  else if (narrow && tall) size = [NARROWEST, TALLEST];
  else if (wide && tall) size = WIDEST / width <= TALLEST / height ? [WIDEST, Math.max(SHORTEST, (WIDEST * height) / width)] : [Math.max(NARROWEST, (TALLEST * width) / height), TALLEST];
  else if (narrow && short) size = NARROWEST / width <= SHORTEST / height ? [Math.min(WIDEST, (SHORTEST * width) / height), SHORTEST] : [NARROWEST, Math.min(TALLEST, (NARROWEST * height) / width)];
  else if (wide) size = [WIDEST, Math.max((WIDEST * height) / width, SHORTEST)];
  else if (narrow) size = [NARROWEST, Math.min((NARROWEST * height) / width, TALLEST)];
  else if (tall) size = [Math.max((TALLEST * width) / height, NARROWEST), TALLEST];
  else if (short) size = [Math.min((SHORTEST * width) / height, WIDEST), SHORTEST];
  return [Math.round(size[0]), Math.round(size[1])];
}

/** How a frame of `size` is written: its width, and its shape, which gives
 *  its height where a narrow window makes it narrower. */
const boxed = (size: Size) => `width:${size[0]}px;aspect-ratio:${size[0]} / ${size[1]}`;

export interface Framed {
  /** The picture's element, where there is a picture to draw or ask for;
   *  none while its words are only on their way. */
  image: string;
  /** What names the picture, the surest first: its file's digest, the
   *  address of a copy the page holds. */
  keys: readonly (string | null | undefined)[];
  /** It is the only picture above its words, drawn at its own shape. */
  alone: boolean;
  name: string;
  /** Its words did not go, or wait for a turn: no picture is coming yet,
   *  and the frame stands still with the picture's name in it. */
  still?: boolean;
}

/** A picture's frame as it is first drawn: waiting, until `pictureCame`. */
export function pictureFrame({ image, keys, alone, name, still = false }: Framed): string {
  const named = keys.filter((key): key is string => !!key);
  const style = alone ? ` style="${boxed(fitted(sizeOf(named) ?? UNKNOWN))}"` : "";
  const title = name ? ` title="${escape(name)}"` : "";
  if (still) return `<li class="bubble-picture bubble-picture-still"${style}${title}><span class="bubble-picture-name">${escape(name)}</span></li>`;
  // Without a picture of its own to say so, the frame is what waits.
  const waits = image ? "" : ` role="img" aria-busy="true" aria-label="${escape(name)}"`;
  return `<li class="bubble-picture waiting" data-named="${escape(named.join("\n"))}"${style}${title}${waits}>${image}<span class="wait-dots" aria-hidden="true"><span></span><span></span><span></span></span></li>`;
}

const namesOf = (frame: HTMLElement) => (frame.dataset.named ?? "").split("\n").filter(Boolean);

/** A waiting frame came onto the page: it has waited since its picture
 *  first did, so its dots, which come only after a moment, do not start
 *  over each time the words around it are drawn again. */
export function frameCame(frame: HTMLElement): void {
  const now = performance.now();
  const names = namesOf(frame);
  const from = names.map((name) => since.get(name)).find((at) => at !== undefined) ?? now;
  for (const name of names) if (!since.has(name)) since.set(name, from);
  for (const oldest of since.keys()) {
    if (since.size <= MOST) break;
    since.delete(oldest);
  }
  frame.dataset.since = String(from);
  frame.style.setProperty("--waited", `${Math.round(now - from)}ms`);
}

/** The picture in a frame has come: its size is kept, the frame stops
 *  waiting, and where it stood at another size than the picture's it goes
 *  to that. A picture waited for comes in; one there at once just is. */
export function pictureCame(image: HTMLImageElement): void {
  const frame = image.closest<HTMLElement>(".bubble-picture");
  if (!frame?.classList.contains("waiting") || !image.naturalWidth || !image.naturalHeight) return;
  const size: Size = [image.naturalWidth, image.naturalHeight];
  const names = namesOf(frame);
  for (const name of names) {
    rememberSize(name, ...size);
    since.delete(name);
  }
  const waited = performance.now() - Number(frame.dataset.since ?? performance.now());
  frame.classList.remove("waiting");
  const moves = !reducedMotion() && waited > AT_ONCE;
  if (frame.parentElement?.classList.contains("one")) {
    const from = frame.getBoundingClientRect();
    const to = fitted(size);
    frame.style.width = `${to[0]}px`;
    frame.style.aspectRatio = `${to[0]} / ${to[1]}`;
    const now = frame.getBoundingClientRect();
    const style = getComputedStyle(frame);
    const duration = msOf(style.getPropertyValue("--enter"));
    if (moves && duration > 0 && (Math.abs(now.width - from.width) >= 1 || Math.abs(now.height - from.height) >= 1)) {
      frame.animate(
        [
          { width: `${from.width}px`, height: `${from.height}px` },
          { width: `${now.width}px`, height: `${now.height}px` },
        ],
        { duration, easing: style.getPropertyValue("--fold-easing").trim() || "ease" },
      );
    }
  }
  if (moves) arrive(image, "quiet");
}

/** A picture's frame whose picture could not be had: it is a frame no
 *  more, and takes the shape of what is said in its place. */
export function frameFailed(frame: HTMLElement): void {
  for (const name of namesOf(frame)) since.delete(name);
  frame.classList.remove("waiting");
  frame.removeAttribute("style");
  frame.removeAttribute("role");
  frame.removeAttribute("aria-busy");
  frame.removeAttribute("aria-label");
}
