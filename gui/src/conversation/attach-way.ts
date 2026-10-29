// The paperclip under the words (`world-gui.md`, 界面的结构): pressed, it
// picks files. On a phone it opens a short menu over the input instead, the
// camera first and files after, so the row under the words keeps its room
// for the model's name and the send button. Where no camera is to hand, an
// input that asks for a capture is only another file picker, so the menu is
// a phone's alone.

import type { AttachWords } from "./attach";
import { glyph } from "./glyphs";
import { escape } from "./prose";

/** Makes `button` the paperclip: files at once, or on a phone a menu put in
 *  `menus` offering the camera and files. A press anywhere else in `page`
 *  puts the menu away. */
export function wireAttach(
  page: HTMLElement,
  button: HTMLButtonElement,
  menus: HTMLElement,
  words: AttachWords,
  ways: { files(): void; photo(): void },
): void {
  button.hidden = false;
  button.innerHTML = glyph("attach", 18);
  button.setAttribute("aria-label", words.attach);
  button.title = words.attach;
  if (!matchMedia("(pointer: coarse)").matches) {
    button.addEventListener("click", () => ways.files());
    return;
  }
  const menu = document.createElement("div");
  menu.className = "composer-menu attach-pop";
  menu.setAttribute("role", "menu");
  menu.hidden = true;
  menu.innerHTML =
    `<button class="attach-row" type="button" role="menuitem" data-way="photo">${glyph("camera", 18)}<span>${escape(words.photo)}</span></button>` +
    `<button class="attach-row" type="button" role="menuitem" data-way="files">${glyph("attach", 18)}<span>${escape(words.attach)}</span></button>`;
  menus.append(menu);
  button.setAttribute("aria-haspopup", "menu");
  button.setAttribute("aria-expanded", "false");
  const show = (open: boolean) => {
    menu.hidden = !open;
    button.setAttribute("aria-expanded", String(open));
  };
  button.addEventListener("click", () => show(menu.hidden));
  menu.addEventListener("click", (event) => {
    const way = (event.target as HTMLElement).closest<HTMLElement>("[data-way]")?.dataset.way;
    if (!way) return;
    show(false);
    if (way === "photo") ways.photo();
    else ways.files();
  });
  page.addEventListener("pointerdown", (event) => {
    const target = event.target as Node;
    if (!menu.hidden && !menu.contains(target) && !button.contains(target)) show(false);
  });
  page.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !menu.hidden) {
      show(false);
      button.focus();
    }
  });
}
