// The marks a list of steps is ticked with as they go, drawn in the film's
// hand: shared by every dialog that does something in steps.

import { Sketch } from "./sketch";

export type StepResult = "done" | "failed" | "skipped";

/** A pencil tick for a step done, a cross for one that failed, a dash for
 *  one there was nothing to do for, a faint dot for one still to come. */
export function stepMark(result: StepResult | null): string {
  const sheet = new Sketch(`removal-${result ?? "coming"}`);
  if (result === "done")
    sheet.stroke(
      [
        [3, 8.6],
        [6.6, 12],
        [13, 4.2],
      ],
      1.9,
      "current",
      0.18,
    );
  else if (result === "failed")
    sheet
      .stroke(
        [
          [4, 4],
          [12, 12],
        ],
        1.9,
        "current",
      )
      .stroke(
        [
          [12, 4],
          [4, 12],
        ],
        1.9,
        "current",
      );
  else if (result === "skipped") sheet.rule([4, 8.5], [12, 8], 1.8, "current");
  else sheet.blot([8, 8], 1.6, "current");
  return sheet.svg(16, 16, "removal-mark");
}
