// A page's document as the interface opens it: the HTML a resident wrote,
// with the World put first in its head. First comes a policy that keeps
// the page from reaching anything outside itself (no network, no frames,
// no forms sent anywhere; what it needs comes through the bridge), then
// the runtime, which runs before anything the page wrote. The theme and
// language the page starts in are written on its root, so its first frame
// is already right; the host's context follows.
//
// What runs inside a page is `../page`; this folder is the interface's side.

import runtime from "virtual:world-page-runtime";
import type { Theme } from "../page/bridge";

export interface Composing {
  theme: Theme;
  locale: string;
}

/** The policy a page lives under. Inline script and style are the page
 *  itself; images, media and faces only as data the page holds (the
 *  World's faces arrive as bytes through the bridge). */
export const POLICY = [
  "default-src 'none'",
  "script-src 'unsafe-inline'",
  "style-src 'unsafe-inline'",
  "img-src data: blob:",
  "media-src data: blob:",
  "font-src data: blob:",
  "form-action 'none'",
  "base-uri 'none'",
].join("; ");

export function compose(html: string, composing: Composing): string {
  const page = new DOMParser().parseFromString(html, "text/html");
  const root = page.documentElement;
  root.dataset.worldTheme = composing.theme;
  root.dataset.locale = composing.locale;
  const rule = page.createElement("meta");
  rule.httpEquiv = "Content-Security-Policy";
  rule.content = POLICY;
  const script = page.createElement("script");
  script.textContent = runtime;
  page.head.prepend(rule, script);
  if (!page.head.querySelector("meta[name=viewport]")) {
    const viewport = page.createElement("meta");
    viewport.name = "viewport";
    viewport.content = "width=device-width, initial-scale=1";
    script.after(viewport);
  }
  return `<!doctype html>\n${root.outerHTML}`;
}
