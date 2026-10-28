// A line between two parts of the window, dragged to make the part beside it
// wider or narrower (`world-gui.md`, 界面的结构): the sidebar's edge, and the
// edge of the panel beside a conversation. Pressed twice, the part goes back
// to the width it has at first; the arrow keys move the line a step at a
// time. Where the width is kept is the caller's to say.

export interface GripOptions {
  /** Which side of the line the part is on: dragging away from it widens it. */
  part: "left" | "right";
  label: string;
  /** The part's width now, and the least and most it may have. */
  width(): number;
  least(): number;
  most(): number;
  /** Takes a width, or none to go back to the first one. */
  set(width: number | null): void;
  /** The line is being dragged, or no longer. */
  moving?(moving: boolean): void;
}

const STEP = 16;

export function makeGrip(className: string, options: GripOptions): HTMLElement {
  const grip = document.createElement("div");
  grip.className = className;
  grip.tabIndex = 0;
  grip.setAttribute("role", "separator");
  grip.setAttribute("aria-orientation", "vertical");
  grip.setAttribute("aria-label", options.label);
  const away = options.part === "left" ? 1 : -1;
  const say = () => {
    grip.setAttribute("aria-valuenow", String(Math.round(options.width())));
    grip.setAttribute("aria-valuemin", String(Math.round(options.least())));
    grip.setAttribute("aria-valuemax", String(Math.round(options.most())));
  };
  const take = (width: number) => {
    options.set(Math.round(Math.min(options.most(), Math.max(options.least(), width))));
    say();
  };

  grip.addEventListener("pointerdown", (event) => {
    if (event.button !== 0 || event.pointerType === "touch") return;
    event.preventDefault();
    const from = event.clientX;
    const began = options.width();
    grip.setPointerCapture(event.pointerId);
    document.documentElement.classList.add("gripping");
    options.moving?.(true);
    const move = (moved: PointerEvent) => take(began + away * (moved.clientX - from));
    const end = () => {
      grip.removeEventListener("pointermove", move);
      grip.removeEventListener("pointerup", end);
      grip.removeEventListener("pointercancel", end);
      document.documentElement.classList.remove("gripping");
      options.moving?.(false);
    };
    grip.addEventListener("pointermove", move);
    grip.addEventListener("pointerup", end);
    grip.addEventListener("pointercancel", end);
  });
  grip.addEventListener("dblclick", () => {
    options.set(null);
    say();
  });
  grip.addEventListener("keydown", (event) => {
    const step = event.key === "ArrowRight" ? STEP : event.key === "ArrowLeft" ? -STEP : 0;
    if (step) take(options.width() + away * step);
    else if (event.key === "Home") take(options.least());
    else if (event.key === "End") take(options.most());
    else return;
    event.preventDefault();
  });
  grip.addEventListener("focus", say);
  return grip;
}
