// Whether a click on a dialog was a press on the shade around it. The click
// of a drag lands on what the press began and ended over have in common, so
// a drag that selects text inside the dialog and is let go over the shade
// clicks the dialog itself: it is a press on the shade only where it also
// began there.

const began = new WeakMap<HTMLElement, EventTarget | null>();

/** Follows where presses on `dialog` begin; call once, as it is made. */
export function watchShade(dialog: HTMLElement): void {
  dialog.addEventListener(
    "pointerdown",
    (event) => {
      began.set(dialog, event.target);
    },
    true,
  );
}

/** A click on `dialog` that began and ended on its shade. */
export function onShade(dialog: HTMLElement, event: Event): boolean {
  return event.target === dialog && began.get(dialog) === dialog;
}
