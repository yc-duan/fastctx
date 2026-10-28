// A row of the pages that list what is the user's own (`world-gui.md`, 「我」):
// its mark, its name with a quieter line, and at its end what is said of
// it and what can be done; one solid box, no line round it. What is too
// long to say on it is said when it is pointed at.

import { escape } from "../conversation/prose";

export function row(mark: string, name: string, sub: string, verb: string, end: string, state = "", title = ""): string {
  // A row that ends in one button keeps it on its line, even on a phone.
  const act = !verb && end.startsWith("<button") ? " act" : "";
  return `<li class="me-row${state}${act}"${title ? ` title="${escape(title)}"` : ""}><span class="me-row-mark">${mark}</span><span class="me-row-name">${name}${sub ? `<span class="me-row-sub">${sub}</span>` : ""}</span><span class="me-row-end">${verb ? `<span class="me-row-verb">${escape(verb)}</span>` : ""}${end}</span></li>`;
}
