// Words just sent come into their place as the sheet beside a conversation
// opens (`app/room.ts`): over one glide, the view is brought down to its foot,
// what was there pushed up before them, and the words, their attachments with
// them as one block, rise the rest of the way from below. One animation keeps
// the time of both, so the scroll and the rise never part.

import { msOf, reducedMotion } from "../motion";

/** A glide under way. */
export interface Glide {
  /** Lets it go where it stands, as the reader takes the view. */
  stop(): void;
}

/** Glides `scroller` from where it stands to its foot, `block`, the words
 *  just sent, rising whatever part of its height the scroll leaves; calls
 *  `done` once it stands at the foot or was let go. None where nothing is to
 *  move, the view put at its foot at once. */
export function glideSent(scroller: HTMLElement, block: HTMLElement, done: () => void): Glide | null {
  const style = getComputedStyle(block);
  const whole = reducedMotion() ? 0 : msOf(style.getPropertyValue("--glide"));
  const from = scroller.scrollTop;
  const foot = () => Math.max(0, scroller.scrollHeight - scroller.clientHeight);
  const rise = Math.max(0, Math.round(block.getBoundingClientRect().height - (foot() - from)));
  if (!whole || typeof block.animate !== "function" || (foot() - from < 1 && rise < 1)) {
    scroller.scrollTop = scroller.scrollHeight;
    done();
    return null;
  }
  // As a transform, and its end heard as an event, as `room.ts` has it: an
  // older system browser knows neither `translate` nor an animation's promise.
  const animation = block.animate([{ transform: `translateY(${rise}px)` }, { transform: "none" }], {
    duration: whole,
    easing: style.getPropertyValue("--glide-easing").trim() || "ease",
  });
  let frame = 0;
  let over = false;
  const step = () => {
    const progress = animation.playState === "finished" ? 1 : (animation.effect?.getComputedTiming().progress ?? 0);
    // To the foot as it is now: the answer may be growing beneath already.
    const at = Math.round(from + (foot() - from) * progress);
    if (scroller.scrollTop !== at) scroller.scrollTop = at;
    frame = requestAnimationFrame(step);
  };
  const end = () => {
    if (over) return;
    over = true;
    cancelAnimationFrame(frame);
    done();
  };
  frame = requestAnimationFrame(step);
  animation.onfinish = () => {
    scroller.scrollTop = scroller.scrollHeight;
    end();
  };
  return {
    stop: () => {
      animation.cancel();
      end();
    },
  };
}
