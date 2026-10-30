import { onMotion, reducedMotion } from "../motion";

/** Owns the conversation's scroll anchoring during an explicit user action:
 * the pressed element stays where it was on the screen as far as the content
 * lets the view go. Nothing is added past the content's ends to hold it
 * there: at the end of a short or scrolled-down conversation the content
 * settles against the edge as a box closes, so the reader can never scroll
 * into empty room that a fold left behind.
 *
 * What it makes up for is only what the motion moves: where the pressed
 * element is in the content, not on the screen. The reader may scroll while
 * a box opens or closes, and their scroll is theirs; a box growing below
 * the element moves nothing above it, so then the view is never touched. */
export class AnchorLayout {
  private locks = 0;
  private previousAnchor = "";
  private previousBehavior = "";

  constructor(readonly scroller: HTMLElement) {}

  /** Where `element` is in the scrolled content: what layout moves, and
   *  scrolling does not. */
  private inContent(element: HTMLElement): number {
    return element.getBoundingClientRect().top - this.scroller.getBoundingClientRect().top + this.scroller.scrollTop;
  }

  hold(anchor: HTMLElement): { correct: () => number; release: () => void } {
    let last = this.inContent(anchor);
    // What is owed in whole pixels: Chromium rounds scrollTop to them.
    let owed = 0;
    if (this.locks++ === 0) {
      this.previousAnchor = this.scroller.style.overflowAnchor;
      this.previousBehavior = this.scroller.style.scrollBehavior;
      this.scroller.style.overflowAnchor = "none";
      this.scroller.style.scrollBehavior = "auto";
    }
    return {
      correct: () => {
        // An element drawn anew meanwhile is no longer where anything is.
        if (!anchor.isConnected) return 0;
        const now = this.inContent(anchor);
        owed += now - last;
        last = now;
        const step = Math.round(owed);
        if (step) {
          const before = this.scroller.scrollTop;
          this.scroller.scrollTop = before + step;
          owed -= this.scroller.scrollTop - before;
          // Against an end of the content nothing more can be made up.
          if (this.scroller.scrollTop === before) owed = 0;
        }
        return owed;
      },
      release: () => {
        if (--this.locks === 0) {
          this.scroller.style.overflowAnchor = this.previousAnchor;
          this.scroller.style.scrollBehavior = this.previousBehavior;
        }
      },
    };
  }

  destroy(): void {}
}

export interface FoldFrame { time: number; height: number; anchorError: number; }
export interface FoldMotion { cancelled: boolean; reduced: boolean; }

/** Animate only the clipping box. Keep the clicked element alive throughout
 * the transition. A reversal begins at the currently painted height. */
export class AnchoredHeight {
  private active: { animation: Animation; finish: (cancelled: boolean) => void; retarget: (height: number) => void } | null = null;
  private readonly unfollow: () => void;

  constructor(readonly box: HTMLElement, private layout: AnchorLayout, private frame?: (frame: FoldFrame) => void) {
    this.unfollow = onMotion(this.motionChanged);
  }

  private motionChanged = (): void => { if (reducedMotion()) this.active?.finish(false); };

  /** Whether the box is on its way to a height now. */
  get moving(): boolean {
    return this.active !== null;
  }

  /** A source or viewport changed while opening. Keep the original completion
   * promise and anchor, and continue from the painted height to the new target. */
  resize(height: number, anchor: HTMLElement): void {
    if (this.active) { this.active.retarget(height); return; }
    const held = this.layout.hold(anchor);
    this.box.style.height = `${Math.round(height)}px`;
    held.correct(); held.release();
  }

  to(height: number, anchor: HTMLElement): Promise<FoldMotion> {
    this.active?.finish(true);
    // Chromium rounds scrollTop to CSS pixels even at high device scales:
    // the box moves in whole pixels, which leave no residual jump under the
    // pointer, never past where it goes, and ends at the exact height, the
    // one it keeps once nothing holds it.
    let from = this.box.getBoundingClientRect().height;
    const held = this.layout.hold(anchor);
    const style = getComputedStyle(this.box);
    const token = style.getPropertyValue("--expand").trim();
    const duration = parseFloat(token) * (token.endsWith("ms") ? 1 : 1000);
    const reduced = reducedMotion();
    if (!Number.isFinite(duration)) throw new Error("--expand must be a CSS time");
    if (reduced || duration === 0 || Math.abs(height - from) < 0.01) {
      this.box.style.height = `${height}px`;
      this.frame?.({ time: performance.now(), height, anchorError: held.correct() });
      held.release();
      return Promise.resolve({ cancelled: false, reduced });
    }
    const animation = this.box.animate([], {
      duration, easing: style.getPropertyValue("--expand-easing").trim(), fill: "both",
    });
    // The motion's clock starts with its first frame: what opening the box
    // took (its rows made, laid out) is not spent from the motion.
    animation.pause();
    return new Promise((resolve) => {
      let request = 0;
      const finish = (cancelled: boolean): void => {
        if (this.active?.animation !== animation) return;
        cancelAnimationFrame(request);
        const final = cancelled ? this.box.getBoundingClientRect().height : height;
        this.box.style.height = `${final}px`;
        animation.cancel();
        this.frame?.({ time: performance.now(), height: final, anchorError: held.correct() });
        held.release();
        this.active = null;
        resolve({ cancelled, reduced: reduced || reducedMotion() });
      };
      const retarget = (target: number): void => {
        if (Math.abs(target - height) < 0.01) return;
        from = this.box.getBoundingClientRect().height;
        height = target;
        animation.currentTime = 0;
      };
      this.active = { animation, finish, retarget };
      const tick = (time: number): void => {
        if (animation.playState === "paused") animation.play();
        const progress = animation.effect?.getComputedTiming().progress ?? 0;
        const step = progress >= 1 ? height : Math.round(from + (height - from) * progress);
        this.box.style.height = `${Math.min(Math.max(step, Math.min(from, height)), Math.max(from, height))}px`;
        this.frame?.({ time, height: this.box.getBoundingClientRect().height, anchorError: held.correct() });
        // Correct even when no instrumentation is installed.
        if (!this.frame) held.correct();
        request = requestAnimationFrame(tick);
      };
      request = requestAnimationFrame(tick);
      animation.finished.then(() => finish(false), () => {});
    });
  }

  destroy(): void {
    this.active?.finish(true);
    this.unfollow();
  }
}
