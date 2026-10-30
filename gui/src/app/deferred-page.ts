// A page whose code loads as it is first opened, so what every page needs
// comes first and alone. Its place stands empty for the moment the load
// takes; a page left before its code arrived is never made.

import type { Page } from "./shell";

export function deferredPage(title: string, make: () => Promise<Page>): Page {
  const element = document.createElement("div");
  element.className = "deferred-page";
  let page: Page | null = null;
  let left = false;
  make().then(
    (made) => {
      if (left) {
        made.leave?.();
        return;
      }
      page = made;
      element.replaceChildren(made.element);
    },
    (error: unknown) => console.error("a page's code could not be loaded", error),
  );
  return {
    element,
    // The page names itself once it is there, as it may as it changes.
    get title() {
      return page?.title ?? title;
    },
    leave() {
      left = true;
      page?.leave?.();
    },
  };
}
