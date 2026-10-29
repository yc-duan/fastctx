// Before a link in a conversation runs a program on this computer: opening
// a program is running it, so the sheet shows the file in its folder as the
// plain way, and running it is said in the colour of what cannot be taken
// back. The shell says which files are programs (`programs.rs`).

import { Sheet } from "./confirm";
import { escape } from "../conversation/prose";
import type { ProgramWords } from "./program-words";

export interface ProgramWays {
  show(path: string): Promise<void>;
  run(path: string): Promise<void>;
}

/** Asks what to do with the program at `path`. */
export function askProgram(words: ProgramWords, path: string, ways: ProgramWays): void {
  const sheet = new Sheet("device-sheet confirm-sheet program-sheet");
  const name = path.split(/[\\/]/).pop() || path;
  sheet.body.innerHTML = `
    ${sheet.head(words.title(name), words.close)}
    <p class="confirm-what">${escape(words.what)}</p>
    <p class="sheet-note" role="alert" hidden></p>
    <footer class="sheet-foot"><button class="text-button danger-text" type="button" data-run>${escape(words.run)}</button><button class="sheet-go" type="submit">${escape(words.show)}</button></footer>`;
  const note = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
  const go = (way: (path: string) => Promise<void>) => {
    if (sheet.busy) return;
    sheet.busy = true;
    way(path).then(
      () => {
        sheet.busy = false;
        sheet.close();
      },
      (error: unknown) => {
        sheet.busy = false;
        note.textContent = words.failed;
        note.title = error instanceof Error ? error.message : String(error);
        note.hidden = false;
      },
    );
  };
  sheet.body.onsubmit = (event) => {
    event.preventDefault();
    go(ways.show);
  };
  sheet.body.querySelector<HTMLButtonElement>("[data-run]")!.onclick = () => go(ways.run);
  sheet.show();
}
