// The World's faces, handed to a page as bytes. A page's origin is opaque
// and its policy lets it fetch nothing, so it cannot load the faces the way
// the interface does; the host reads them once from its own origin and
// gives each page a copy through the bridge, which the page's runtime turns
// into faces of its own document. `fonts.css` is the one list of them.

import declared from "../design/fonts.css?raw";
import { invoke } from "@tauri-apps/api/core";
import { inShell } from "../world/bridge";

export interface Face {
  family: string;
  style: string;
  weight: string;
  unicodeRange?: string;
  /** The file's bytes. */
  data: ArrayBuffer;
}

interface Declared extends Omit<Face, "data"> {
  url: string;
}

function parsed(css: string): Declared[] {
  return [...css.matchAll(/@font-face\s*{([^}]*)}/g)].map(([, block]) => {
    const field = (name: string) => block!.match(new RegExp(`${name}\\s*:\\s*([^;]+);`))?.[1]?.replace(/\s+/g, " ").trim();
    const url = block!.match(/url\("([^"]+)"\)/)?.[1];
    if (!url) throw new Error(`fonts.css: a face without a url: ${block}`);
    const range = field("unicode-range");
    return {
      family: (field("font-family") ?? "").replace(/^"|"$/g, ""),
      style: field("font-style") ?? "normal",
      weight: field("font-weight") ?? "400",
      ...(range ? { unicodeRange: range } : {}),
      url,
    };
  });
}

let reading: Promise<Face[]> | null = null;

/** Every face, read once. */
export function faces(): Promise<Face[]> {
  reading ??= Promise.all(
    parsed(declared).map(async ({ url, ...face }) => {
      if (inShell()) return { ...face, data: await invoke<ArrayBuffer>("plugin:world-pages|font", { path: url }) };
      const response = await fetch(url);
      if (!response.ok) throw new Error(`reading ${url}: ${response.status}`);
      return { ...face, data: await response.arrayBuffer() };
    }),
  ).catch((error: unknown) => {
    reading = null;
    throw error;
  });
  return reading;
}
