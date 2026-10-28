// A sheet over the page, and the one question asked before something that
// is not easily taken back, or that the user should read first: removing a
// machine or a device, logging out, removing an agent, trusting a machine.
// It says what will happen, waits for the one button, and stays open with
// what went wrong if the World refuses.

import { leave, stay } from "./leave";
import { onShade, watchShade } from "./shade-press";
import { Steady } from "./steady";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";

export class Sheet {
  readonly dialog: HTMLDialogElement;
  readonly body: HTMLElement;
  busy = false;
  /** Open, it keeps the size it opened at: what changes in it may make it
   *  taller, as far as the screen allows, and never shorter, so nothing in
   *  it moves under the reader (`steady.ts`). */
  private readonly steady: Steady;

  /** `fixed`: its size is its own, as a folder picker's, and not the one
   *  it opened at. */
  constructor(
    className: string,
    private readonly fixed = false,
  ) {
    // It stands in the document only while it is open, so a page let go
    // of takes the sheets it made with it.
    this.dialog = document.createElement("dialog");
    this.dialog.className = `sheet ${className}`;
    this.dialog.innerHTML = `<form class="sheet-body" method="dialog"></form>`;
    this.body = this.dialog.querySelector("form")!;
    this.steady = new Steady(this.body, true);
    watchShade(this.dialog);
    this.dialog.addEventListener("click", (event) => {
      if (onShade(this.dialog, event) || (event.target as HTMLElement).closest(".sheet-close")) this.close();
    });
    // Escape closes it as the close button does, going as it came.
    this.dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      this.close();
    });
    this.dialog.addEventListener("close", () => {
      this.steady.free();
      this.dialog.remove();
    });
  }

  head(title: string, close: string): string {
    return `<header class="sheet-head"><h2 class="sheet-title">${escape(title)}</h2><button class="icon-button sheet-close" type="button" aria-label="${escape(close)}">${glyph("close", 18)}</button></header>`;
  }

  show(): void {
    stay(this.dialog);
    if (this.dialog.open) return;
    if (!this.dialog.isConnected) document.body.append(this.dialog);
    this.dialog.showModal();
    if (this.fixed) return;
    this.steady.hold();
    this.steady.watch();
  }

  close(): void {
    if (this.busy || !this.dialog.open) return;
    leave(this.dialog, () => this.dialog.close(), { as: "sheet-leaving", moving: this.body });
  }

  /** What it holds became another stage, as a sheet opened anew: it takes
   *  its size from what is drawn now. */
  anew(): void {
    if (this.dialog.open && !this.fixed) this.steady.hold();
  }
}

export interface ConfirmWords {
  close: string;
  /** On the button while the World does it. */
  working: string;
  failed(why: string): string;
}

export interface Asked {
  title: string;
  /** What will happen, said plainly. */
  what: string;
  /** Fields asked along with it, as markup inside the form. */
  fields?: string;
  go: string;
  /** The button is said in the quiet colour, for what is not lost by
   *  doing it; otherwise in the colour of what cannot be undone. */
  gentle?: boolean;
  act(): Promise<void>;
  /** It was done; the sheet has closed. */
  done(): void;
}

export class Confirm {
  private readonly sheet = new Sheet("device-sheet confirm-sheet");

  constructor(private readonly words: ConfirmWords) {}

  /** The form, for reading what was filled in. */
  get body(): HTMLElement {
    return this.sheet.body;
  }

  ask(asked: Asked): void {
    const { words, sheet } = this;
    sheet.body.innerHTML = `
      ${sheet.head(asked.title, words.close)}
      <p class="confirm-what">${escape(asked.what)}</p>
      ${asked.fields ?? ""}
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go${asked.gentle ? "" : " danger"}" type="submit">${escape(asked.go)}</button></footer>`;
    sheet.body.onsubmit = (event) => {
      event.preventDefault();
      if (sheet.busy) return;
      const button = sheet.body.querySelector<HTMLButtonElement>(".sheet-go")!;
      const note = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
      sheet.busy = true;
      button.disabled = true;
      button.textContent = words.working;
      asked
        .act()
        .then(() => {
          sheet.busy = false;
          sheet.close();
          asked.done();
        })
        .catch((error: unknown) => {
          sheet.busy = false;
          button.disabled = false;
          button.textContent = asked.go;
          note.textContent = words.failed(error instanceof Error ? error.message : String(error));
          note.title = error instanceof Error ? error.message : String(error);
          note.hidden = false;
        });
    };
    sheet.show();
  }
}
