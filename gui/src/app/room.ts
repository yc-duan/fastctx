// A sheet at the desk's edge comes and goes, and the page beside it gives
// way as it does (the user, 2026-11-21: "正文文字，没有被侧栏温柔推开的动画，
// 而是正文瞬间缩小宽度", "侧栏宽度逐帧增长、正文同步逐帧重排").
//
// The sheet stands at the edge, as large as it is open, and is only moved:
// by the compositor, which waits for no layout. Its room beside the page is
// given a little at every frame, as much as the sheet has come by then, and
// the page is laid out for that room: its lines are set again as the sheet
// pushes them. A page slow to lay out follows in fewer and larger steps,
// under a sheet that has already come; the sheet does not halt for it, as
// it did while its own place was the page's to lay out (5a0df94fe), and the
// page does not take its whole new room before the sheet has moved, its
// lines set anew at once (683f9b09d).

import { msOf, reducedMotion } from "../motion";

/** A glide under way: how much of its room the sheet has now, its end, and
 *  its room given back to the style sheet. */
type Gliding = { share: () => number; stop: () => void; clear: () => void };

const gliding = new WeakMap<HTMLElement, Gliding>();

export type Giving = {
  /** How far from its place the sheet stands when shut, across and down. */
  away: readonly [number, number];
  /** The room it takes open, in pixels. */
  size: number;
  open: boolean;
  /** Gives the sheet `room` pixels beside the page, `share` of all it takes;
   *  none once the glide is over and the room is the style sheet's again. */
  room(room: number | null, share: number): void;
};

/** Slides `sheet` open or shut over one glide, its room given in step. A
 *  glide under way is taken over from where the sheet is. */
export function give(sheet: HTMLElement, { away, size, open, room }: Giving): void {
  const style = getComputedStyle(sheet);
  const whole = reducedMotion() ? 0 : msOf(style.getPropertyValue("--glide"));
  const under = gliding.get(sheet);
  const from = under ? under.share() : open ? 0 : 1;
  const to = open ? 1 : 0;
  under?.stop();
  if (!whole || from === to || typeof sheet.animate !== "function") {
    room(null, to);
    return;
  }
  const at = (share: number) => `${away[0] * (1 - share)}px ${away[1] * (1 - share)}px`;
  const animation = sheet.animate([{ translate: at(from) }, { translate: at(to) }], {
    duration: whole * Math.abs(to - from),
    easing: style.getPropertyValue("--glide-easing").trim() || "ease",
  });
  const share = () => {
    const progress = animation.playState === "finished" ? 1 : (animation.effect?.getComputedTiming().progress ?? 0);
    return from + (to - from) * progress;
  };
  let frame = 0;
  let given = -1;
  const step = () => {
    const now = share();
    const pixels = Math.round(size * now);
    // A frame the sheet did not move a pixel in lays nothing out.
    if (pixels !== given) {
      given = pixels;
      room(pixels, now);
    }
    frame = requestAnimationFrame(step);
  };
  const state: Gliding = {
    share,
    stop: () => {
      cancelAnimationFrame(frame);
      gliding.delete(sheet);
      animation.cancel();
    },
    clear: () => room(null, to),
  };
  gliding.set(sheet, state);
  // Its room as the glide sets out, before anything is painted: the style
  // sheet has given it all, or none, already.
  given = Math.round(size * from);
  room(given, from);
  frame = requestAnimationFrame(step);
  animation.finished.then(
    () => {
      if (gliding.get(sheet) !== state) return;
      cancelAnimationFrame(frame);
      gliding.delete(sheet);
      room(null, to);
    },
    () => {},
  );
}

/** Ends the glide of `sheet` where the style sheet has it, as the desk is
 *  laid out otherwise under it. */
export function settle(sheet: HTMLElement): void {
  const under = gliding.get(sheet);
  if (!under) return;
  under.stop();
  under.clear();
}
