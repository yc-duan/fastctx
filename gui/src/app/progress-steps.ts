// A flow's steps as one line of marks: those done ticked, the one
// this is filled in clay with its number, those to come faint, joined by a
// line drawn as far as the flow has gone. It says where one is; nothing in
// it is pressed.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";

/** The steps named `labels`, the one at `current` being this one; at
 *  `labels.length` all are done. */
export function progressSteps(labels: readonly string[], current: number, label: string): string {
  const steps = labels
    .map((name, index) => {
      const state = index < current ? "done" : index === current ? "current" : "ahead";
      const shown = state === "done" ? glyph("check", 13) : String(index + 1);
      return `<li class="progress-step ${state}"${state === "current" ? ' aria-current="step"' : ""}><span class="progress-mark" aria-hidden="true">${shown}</span><span class="progress-label">${escape(name)}</span></li>`;
    })
    .join("");
  return `<ol class="progress-steps" aria-label="${escape(label)}">${steps}</ol>`;
}
