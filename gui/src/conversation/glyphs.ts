// The activity rail's glyphs: 20px, drawn on a 20-unit grid with a 1.5
// stroke, in the colour of the text around them.

import type { Glyph } from "./tool";

type Named = Glyph | "thinking" | "chevron" | "copy" | "check" | "expand" | "stop" | "send" | "attach" | "file" | Frame;

/** The frame's own glyphs: the sidebar, new conversations, settings, and
 *  what can be done to an agent (sleep, wake, move to a project, remove,
 *  which archives it). */
type Frame = "menu" | "plus" | "settings" | "sidebar" | "machine" | "close" | "more" | "back" | "sleep" | "wake" | "move" | "archive" | "reread" | "scan";

const paths: Record<Named, string> = {
  menu: '<path d="M4 7h12M4 13h8"/>',
  plus: '<path d="M10 4.5v11M4.5 10h11"/>',
  settings: '<circle cx="10" cy="10" r="2.4"/><path d="M10 3.5v1.6M10 14.9v1.6M16.5 10h-1.6M5.1 10H3.5M14.6 5.4l-1.1 1.1M6.5 13.5l-1.1 1.1M14.6 14.6l-1.1-1.1M6.5 6.5L5.4 5.4"/>',
  sidebar: '<rect x="3.75" y="4.75" width="12.5" height="10.5" rx="1.5"/><path d="M8.25 4.75v10.5"/>',
  machine: '<rect x="3.75" y="4.25" width="12.5" height="8.75" rx="1.25"/><path d="M7.5 15.75h5M10 13v2.75"/>',
  close: '<path d="M5.5 5.5l9 9M14.5 5.5l-9 9"/>',
  more: '<circle cx="5.5" cy="10" r="0.6" fill="currentColor"/><circle cx="10" cy="10" r="0.6" fill="currentColor"/><circle cx="14.5" cy="10" r="0.6" fill="currentColor"/>',
  back: '<path d="M11.5 5.5L7 10l4.5 4.5"/>',
  sleep: '<path d="M15 12.4A5.75 5.75 0 0 1 7.6 5a5.75 5.75 0 1 0 7.4 7.4z"/>',
  wake: '<path d="M3.75 14.75h12.5"/><path d="M6.5 14.75a3.5 3.5 0 0 1 7 0"/><path d="M10 6v2M5.4 8.4l1.2 1.2M14.6 8.4l-1.2 1.2"/>',
  move: '<path d="M3.75 6.25a1 1 0 0 1 1-1h3.2l1.5 1.5h5.8a1 1 0 0 1 1 1v6.75a1 1 0 0 1-1 1H4.75a1 1 0 0 1-1-1z"/><path d="M7.75 11h4.5M10.5 9.25L12.25 11l-1.75 1.75"/>',
  reread: '<path d="M15.25 10a5.25 5.25 0 1 1-1.54-3.71"/><path d="M14.25 3.75v2.75H11.5"/>',
  scan: '<path d="M3.75 7V4.75a1 1 0 0 1 1-1H7M13 3.75h2.25a1 1 0 0 1 1 1V7M16.25 13v2.25a1 1 0 0 1-1 1H13M7 16.25H4.75a1 1 0 0 1-1-1V13"/><path d="M6.25 10h7.5"/>',
  archive: '<rect x="3.75" y="4.75" width="12.5" height="3" rx="0.75"/><path d="M5 7.75v6.5a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1v-6.5"/><path d="M8.25 10.75h3.5"/>',
  read: '<path d="M4.5 4.5h7l4 4v7a1 1 0 0 1-1 1h-10a1 1 0 0 1-1-1v-10a1 1 0 0 1 1-1z"/><path d="M11.5 4.5v4h4"/><path d="M7 11h6M7 13.5h4"/>',
  edit: '<path d="M12.6 4.6l2.8 2.8-8.2 8.2H4.4v-2.8z"/><path d="M11 6.2l2.8 2.8"/>',
  command: '<path d="M4.5 6l3.5 4-3.5 4"/><path d="M10 14.5h5.5"/>',
  search: '<circle cx="9" cy="9" r="4.25"/><path d="M12.2 12.2l3.6 3.6"/>',
  image: '<rect x="3.75" y="4.75" width="12.5" height="10.5" rx="1.5"/><circle cx="7.75" cy="8.5" r="1.25"/><path d="M4 13.5l3.5-3 2.8 2.4 2.2-1.9 3.5 2.8"/>',
  colleague: '<circle cx="7.5" cy="7.5" r="2.5"/><path d="M3.5 15.5c.6-2.4 2.1-3.6 4-3.6s3.4 1.2 4 3.6"/><circle cx="13.5" cy="8.5" r="2"/><path d="M13.2 12.1c1.6 0 2.8.9 3.3 2.9"/>',
  web: '<circle cx="10" cy="10" r="6.25"/><path d="M3.75 10h12.5M10 3.75c1.8 1.8 2.6 3.9 2.6 6.25S11.8 14.45 10 16.25C8.2 14.45 7.4 12.35 7.4 10S8.2 5.55 10 3.75z"/>',
  tasks: '<path d="M4.5 6l1.2 1.2L8 5"/><path d="M10.5 6.2h5"/><path d="M4.5 12l1.2 1.2L8 11"/><path d="M10.5 12.2h5"/>',
  tool: '<path d="M12.4 4.2a3.4 3.4 0 0 0-4 4.4l-4.2 4.2 2.9 2.9 4.2-4.2a3.4 3.4 0 0 0 4.4-4l-2 2-1.9-.5-.5-1.9z"/>',
  thinking: '<path d="M10 4.5c-2.9 0-5 1.9-5 4.4 0 1.5.8 2.8 2 3.6v2.5l2.4-1.6c.2 0 .4.1.6.1 2.9 0 5-1.9 5-4.6S12.9 4.5 10 4.5z"/>',
  chevron: '<path d="M7.5 8.5l2.5 2.5 2.5-2.5"/>',
  copy: '<rect x="7" y="7" width="8.5" height="8.5" rx="1.5"/><path d="M5 12.5h-.5a1 1 0 0 1-1-1v-7a1 1 0 0 1 1-1h7a1 1 0 0 1 1 1V5"/>',
  check: '<path d="M5 10.5l3.2 3 6.8-7"/>',
  expand: '<path d="M11.5 4.5h4v4M8.5 15.5h-4v-4M15.5 4.5L11 9M4.5 15.5L9 11"/>',
  stop: '<rect x="6" y="6" width="8" height="8" rx="1.5" fill="currentColor" stroke="none"/>',
  send: '<path d="M10 15.5v-11M5.5 9L10 4.5 14.5 9"/>',
  attach: '<path d="M14.2 9.6l-4.6 4.6a3.1 3.1 0 0 1-4.4-4.4l5.3-5.3a2.1 2.1 0 0 1 3 3l-5.2 5.2a1 1 0 0 1-1.5-1.5l4.6-4.6"/>',
  file: '<path d="M5.5 3.5h5.5l3.5 3.5v8.5a1 1 0 0 1-1 1h-8a1 1 0 0 1-1-1v-11a1 1 0 0 1 1-1z"/><path d="M11 3.5v3.5h3.5"/>',
};

export function glyph(name: keyof typeof paths, size = 20): string {
  return `<svg class="glyph" width="${size}" height="${size}" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${paths[name]}</svg>`;
}
