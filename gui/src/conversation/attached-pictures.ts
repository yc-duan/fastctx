// How a picture attached and not yet sent opens when pressed. The interface
// says once how it shows pictures (`viewPictures`); the trays of attached
// files only ask, so they need nothing of the viewer.

type Viewer = (images: HTMLImageElement[], index: number) => void;

let viewer: Viewer | null = null;

/** How attached pictures are shown from now on. */
export function showAttachedWith(view: Viewer): void {
  viewer = view;
}

/** Opens the picture pressed in `tray`, among the others it holds; false
 *  when nothing pressed was a picture. */
export function viewAttached(tray: HTMLElement, pressed: Element): boolean {
  const image = pressed.closest<HTMLImageElement>("img.attach-thumb");
  if (!image || !viewer) return false;
  const all = [...tray.querySelectorAll<HTMLImageElement>("img.attach-thumb")];
  viewer(all, Math.max(0, all.indexOf(image)));
  return true;
}
