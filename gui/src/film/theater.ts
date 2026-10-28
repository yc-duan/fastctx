// A film watched again over the whole page (`world-gui.md`, 功能): it plays
// until the page behind is touched, Escape is pressed or its close is,
// which sits under the film, away from the window's own buttons.

import { leave } from "../app/leave";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { Film } from "./film";
import type { Built, Shape } from "./reel";

/** Plays the film `make` draws over the page; answers how to close it. */
export function watchFilm(make: (shape: Shape) => Built, label: string, closeWord: string): () => void {
  const theater = document.createElement("div");
  theater.className = "film-theater";
  theater.innerHTML = `<div class="film-theater-box"></div><button class="text-button film-theater-close" type="button">${glyph("close", 16)}<span>${escape(closeWord)}</span></button>`;
  const film = new Film(make, label);
  theater.firstElementChild!.append(film.element);
  const key = (event: KeyboardEvent) => {
    if (event.key === "Escape") close();
  };
  let open = true;
  const close = () => {
    if (!open) return;
    open = false;
    document.removeEventListener("keydown", key);
    leave(theater, () => {
      film.destroy();
      theater.remove();
    });
  };
  theater.addEventListener("click", (event) => {
    const target = event.target as HTMLElement;
    if (!target.closest(".film-stage") || target.closest(".film-theater-close")) close();
  });
  document.addEventListener("keydown", key);
  document.body.append(theater);
  return close;
}
