// What Rust's compiler said as cargo printed it: each error and warning,
// where it points, and the source it showed under it. The lines that only
// total them up (`generated 2 warnings`, `could not compile`) are not
// diagnostics of their own.

import type { ChecksDeed, Diagnostic, Found, OutputLines } from "./types";

const HEAD = /^(error|warning)(?:\[([A-Za-z0-9_:-]+)\])?: (.+)$/;
const TOTALS = /^(?:warning: `[^`]+` \(.+\) generated \d+|error: could not compile|error: aborting due to|warning: build failed|error: test failed|warning: \d+ warnings? emitted|error: \d+ errors?|warning: unused manifest key)/;
const PLACE = /^\s*--> (.+?):(\d+):(\d+)$/;
const FINISHED = /^\s*Finished\b/;

/** The diagnostics among `lines`, all of them one deed; `built` says the
 *  command was one that builds, so that building with nothing to say is
 *  a deed too. */
export function rustChecks(lines: OutputLines, built: boolean): Found<ChecksDeed> | null {
  const items: Diagnostic[] = [];
  let at = -1;
  let finished = false;
  let open: Diagnostic | null = null;
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line === null || line === undefined) {
      open = null;
      continue;
    }
    if (FINISHED.test(line)) {
      finished = true;
      if (at < 0) at = index;
    }
    if (TOTALS.test(line)) {
      open = null;
      continue;
    }
    const head = HEAD.exec(line);
    if (head) {
      if (at < 0) at = index;
      open = { level: head[1] as Diagnostic["level"], code: head[2] ?? null, message: head[3]!, place: null, snippet: [] };
      items.push(open);
      continue;
    }
    if (!open) continue;
    if (!line.trim()) {
      open = null;
      continue;
    }
    const place = PLACE.exec(line);
    if (place && !open.place) {
      open.place = { path: place[1]!, line: Number(place[2]), column: Number(place[3]) };
      continue;
    }
    open.snippet.push(line);
  }
  if (at < 0 || (!items.length && !(built && finished))) return null;
  return { at, deed: { kind: "checks", command: null, items, clean: !items.length } };
}
