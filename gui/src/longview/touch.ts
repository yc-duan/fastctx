import { reducedMotion } from "../motion";
import type { LineViewport } from "./viewport";

/** Chromium latches a touch gesture to its initial scrolling element. Route
 * vertical distance explicitly so the same finger continues into the page at
 * either edge. Horizontal panning and pinch zoom remain browser gestures. */
export function chainTouch(viewport: LineViewport): () => void {
  const element = viewport.element;
  let pointer: number | null = null;
  let x = 0;
  let y = 0;
  let time = 0;
  let velocity = 0;
  let vertical = false;
  let request = 0;
  const move = (delta: number): void => viewport.chain(viewport.scrollBy(delta));
  const down = (event: PointerEvent): void => {
    if (event.pointerType !== "touch" || !event.isPrimary) return;
    cancelAnimationFrame(request);
    pointer = event.pointerId;
    x = event.clientX; y = event.clientY; time = event.timeStamp;
    velocity = 0; vertical = false;
    element.setPointerCapture(pointer);
  };
  const drag = (event: PointerEvent): void => {
    if (pointer !== event.pointerId) return;
    const delta = y - event.clientY;
    if (!vertical && Math.abs(event.clientX - x) > Math.abs(delta)) return;
    vertical = true;
    event.preventDefault();
    if (event.timeStamp > time) velocity = delta / (event.timeStamp - time);
    move(delta);
    y = event.clientY; time = event.timeStamp;
  };
  const up = (event: PointerEvent): void => {
    if (pointer !== event.pointerId) return;
    pointer = null;
    if (!vertical || reducedMotion() || event.type === "pointercancel") return;
    const token = getComputedStyle(element).getPropertyValue("--fold").trim();
    const duration = parseFloat(token) * (token.endsWith("ms") ? 1 : 1000);
    const distance = velocity * duration / 3;
    const started = performance.now();
    let last = 0;
    const tick = (now: number): void => {
      const progress = Math.min(1, (now - started) / duration);
      const offset = distance * (1 - (1 - progress) ** 3);
      move(offset - last); last = offset;
      if (progress < 1) request = requestAnimationFrame(tick);
    };
    request = requestAnimationFrame(tick);
  };
  element.addEventListener("pointerdown", down);
  element.addEventListener("pointermove", drag);
  element.addEventListener("pointerup", up);
  element.addEventListener("pointercancel", up);
  return () => {
    cancelAnimationFrame(request);
    element.removeEventListener("pointerdown", down);
    element.removeEventListener("pointermove", drag);
    element.removeEventListener("pointerup", up);
    element.removeEventListener("pointercancel", up);
  };
}
