// A hand drawing drawn in with the pen the first time it shows (the stroke
// animation of `.welcome-way-mark` in `pages.css`). A page drawn again on a
// change shows the drawings it showed before as they are, drawn already,
// rather than drawing them in anew each time.

export class DrawnOnce {
  private readonly drawn = new Set<string>();

  /** The `.welcome-way-mark` drawing `svg`, named `key`: drawn already when
   *  it showed before. */
  mark(key: string, svg: string): string {
    return svg.replace(/class="welcome-way-mark([^"]*)"/, (_, more: string) => `class="welcome-way-mark${more}${this.drawn.has(key) ? " drawn" : ""}" data-drawing="${key}"`);
  }

  /** Takes note of the drawings `root` shows now. */
  seen(root: ParentNode): void {
    for (const drawing of root.querySelectorAll<SVGElement>("[data-drawing]")) this.drawn.add(drawing.dataset.drawing!);
  }
}
