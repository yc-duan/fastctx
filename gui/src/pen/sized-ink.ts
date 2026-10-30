/** Retains a drawing while only its canvas changes size. The shape key must
 * include every point and state used by draw; dimensions only set its viewBox. */
export function sizedInk(element: HTMLElement, shape: string, width: number, height: number, draw: () => string): void {
  const box = `0 0 ${width} ${height}`;
  const previous = drawings.get(element);
  if (previous?.shape === shape && previous.svg.parentElement === element) {
    if (previous.box !== box) {
      previous.svg.setAttribute("viewBox", box);
      previous.box = box;
    }
    return;
  }
  element.innerHTML = draw();
  const svg = element.querySelector<SVGSVGElement>(":scope > svg");
  if (svg) drawings.set(element, { shape, box, svg });
}

const drawings = new WeakMap<HTMLElement, { shape: string; box: string; svg: SVGSVGElement }>();
