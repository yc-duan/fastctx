// The requested position stays visible while a switch is being saved.

export interface SwitchState {
  on: boolean;
  busy: boolean;
}

export class SwitchChanges {
  private readonly pending = new Map<string, boolean>();
  private readonly moving = new Set<Promise<void>>();
  private redraw: (() => void) | null = null;

  /** A surrounding redraw waits only for the knob's current animation,
   *  never for the remote save. Replacing its DOM would cut the slide short. */
  deferDraw(redraw: () => void): boolean {
    if (!this.moving.size) return false;
    this.redraw = redraw;
    return true;
  }

  private motion(button: HTMLElement): Promise<void> {
    const moving = motion(button).finally(() => {
      this.moving.delete(moving);
      if (!this.moving.size) {
        const redraw = this.redraw;
        this.redraw = null;
        redraw?.();
      }
    });
    this.moving.add(moving);
    return moving;
  }

  state(key: string, on: boolean): SwitchState {
    return { on: this.pending.get(key) ?? on, busy: this.pending.has(key) };
  }

  /** Saves the position the reader chose. False means the caller has shown
   *  why it failed; exceptions also restore the previous position. The key
   *  locks the setting even if its control is redrawn or reopened. */
  async run(key: string, button: HTMLElement, save: (on: boolean) => Promise<boolean>): Promise<boolean> {
    if (this.pending.has(key) || button.getAttribute("aria-disabled") === "true") return false;
    const before = button.getAttribute("aria-checked") === "true";
    const on = !before;
    this.pending.set(key, on);
    paintSwitch(button, { on, busy: true });
    const moved = this.motion(button);
    let saved = false;
    try {
      saved = await save(on);
      await moved;
      return saved;
    } finally {
      if (!saved) {
        this.pending.set(key, before);
        paintSwitch(button, { on: before, busy: true });
        await this.motion(button);
      }
      this.pending.delete(key);
      paintSwitch(button, { on: saved ? on : before, busy: false });
    }
  }
}

/** Updates the existing knob rather than replacing it before it can slide. */
export function paintSwitch(button: HTMLElement, state: SwitchState): void {
  button.setAttribute("aria-checked", String(state.on));
  button.setAttribute("aria-busy", String(state.busy));
  button.setAttribute("aria-disabled", String(state.busy));
  if (button instanceof HTMLButtonElement) button.disabled = state.busy;
}

async function motion(button: HTMLElement): Promise<void> {
  const knob = button.querySelector(".switch-knob");
  if (!knob || !button.isConnected) return;
  getComputedStyle(knob).getPropertyValue("--slide");
  await Promise.allSettled(knob.getAnimations().map((animation) => animation.finished));
}
