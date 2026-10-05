// The app's own version, and a newer one from the project's releases when
// there is one. It is looked for when this row is shown; downloading it
// checks it against the app's signing key, and installing it closes World
// and opens it again, after which the World's version row offers the
// fastctx the new app carries. Every step is said as it happens.

import { escape } from "../conversation/prose";
import type { SettingsWords } from "./words";

export interface Available {
  version: string;
  /** What the release says of itself. */
  notes: string | null;
  /** Downloads, checks the signature and installs; World closes then. */
  install(progress: (received: number, total: number | null) => void): Promise<void>;
}

export interface Updating {
  current(): Promise<string>;
  /** A newer version, or null when this is the latest. */
  check(): Promise<Available | null>;
}

type State =
  | { kind: "checking" }
  | { kind: "latest" }
  | { kind: "unknown"; why: string }
  | { kind: "available"; update: Available }
  | { kind: "downloading"; update: Available; received: number; total: number | null }
  | { kind: "installing"; update: Available }
  | { kind: "failed"; update: Available; why: string };

export class AppUpdateRow {
  private version = "";
  private state: State = { kind: "checking" };
  private left = false;

  constructor(
    private readonly element: HTMLElement,
    private readonly words: SettingsWords,
    private readonly updating: Updating,
    /** Told what was found, so the rest of the interface can mark it. */
    private readonly found: (version: string | null) => void,
  ) {
    element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-update]");
      if (!button || button.disabled) return;
      if (button.dataset.update === "check") this.check();
      else if (this.state.kind === "available" || this.state.kind === "failed") this.install(this.state.update);
    });
    updating.current().then(
      (version) => {
        this.version = version;
        this.draw();
      },
      () => {},
    );
    this.check();
  }

  leave(): void {
    this.left = true;
  }

  private check(): void {
    this.set({ kind: "checking" });
    this.updating.check().then(
      (update) => {
        this.found(update?.version ?? null);
        this.set(update ? { kind: "available", update } : { kind: "latest" });
      },
      (error: unknown) => this.set({ kind: "unknown", why: String(error) }),
    );
  }

  private install(update: Available): void {
    this.set({ kind: "downloading", update, received: 0, total: null });
    update
      .install((received, total) => {
        // The last piece is followed by checking the signature, then the
        // installer takes over and World closes.
        this.set(total !== null && received >= total ? { kind: "installing", update } : { kind: "downloading", update, received, total });
      })
      .catch((error: unknown) => this.set({ kind: "failed", update, why: String(error) }));
  }

  private set(state: State): void {
    if (this.left) return;
    this.state = state;
    this.draw();
  }

  private draw(): void {
    const { words, state } = this;
    const button = (what: "check" | "install", label: string) =>
      `<button class="text-button quiet-button" type="button" data-update="${what}">${escape(label)}</button>`;
    let said = "";
    let act = "";
    let notes = "";
    switch (state.kind) {
      case "checking":
        said = words.appChecking;
        break;
      case "latest":
        said = words.appLatest;
        act = button("check", words.appCheckAgain);
        break;
      case "unknown":
        said = words.appUnknown(state.why);
        act = button("check", words.appCheckAgain);
        break;
      case "available":
        said = words.appAvailable(state.update.version);
        act = button("install", words.appInstall);
        notes = state.update.notes ?? "";
        break;
      case "downloading":
        said = words.appDownloading(state.total ? Math.floor((state.received / state.total) * 100) : null);
        break;
      case "installing":
        said = words.appInstalling;
        break;
      case "failed":
        said = words.appFailed(state.why);
        act = button("install", words.appInstall);
        break;
    }
    this.element.hidden = false;
    this.element.innerHTML = `<dt>${escape(words.app)}</dt><dd><span>${escape(this.version ? `World ${this.version}` : "World")}</span>${act}<p class="settings-said" role="status">${escape(said)}</p>${
      notes ? `<p class="settings-notes">${escape(notes)}</p>` : ""
    }</dd>`;
  }
}
